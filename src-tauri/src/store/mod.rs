//! 存储层：任务 / 运行 / 证据 / 审查 / 决策 / 事件，以及任务状态机。
//!
//! 表结构由 `db.rs` 的迁移创建（迁移 v2 增加 `tasks.prompt`）；状态机与不变量
//! 见 `docs/data-model.md` §3、§4。`tasks` 表没有状态列，任务状态单点存放在关联
//! issue item 的 `items.status` 上，因此所有迁移都必须经由 [`transition`]。

use rusqlite::types::Type;
use rusqlite::{Connection, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

mod attachments;
mod plans;
mod runs;
mod scheduling;
mod settings;
mod tasks;

pub use attachments::*;
pub use plans::*;
pub use runs::*;
pub use scheduling::*;
pub use settings::*;
pub use tasks::*;

#[cfg(test)]
mod test_support;

/// 单个任务允许的最大尝试次数（`failed -> queued` 的上限，用尽后转人工）。
pub const MAX_ATTEMPTS: u32 = 3;

/// 全局设置键：未在任务上单独指定时使用的默认适配器 id。
pub const SETTING_DEFAULT_AGENT: &str = "default_agent";

/// 全局设置键：孤儿 worktree 的保留天数。
pub const SETTING_WORKTREE_RETENTION_DAYS: &str = "worktree_retention_days";

/// 全局设置键：开发者模式开关，`"1"` 启用，其余值（含缺失）关闭。
pub const SETTING_DEV_MODE: &str = "dev_mode";

/// 全局设置键：开发者 API 监听端口。
pub const SETTING_DEV_PORT: &str = "dev_port";

/// 全局设置键：开发者 API 的 Bearer token。
pub const SETTING_DEV_API_TOKEN: &str = "dev_api_token";

/// 开发者 API 默认端口。
pub const DEFAULT_DEV_PORT: u16 = 3789;

/// 开发者 API 允许的最小端口；1024 以下为特权端口，不监听。
pub const MIN_DEV_PORT: u16 = 1024;

/// 保留天数缺失或非法时的回退值。
pub const DEFAULT_WORKTREE_RETENTION_DAYS: u32 = 7;

// ---------------------------------------------------------------------------
// 公开类型
// ---------------------------------------------------------------------------

/// 一条验收标准；`id` 稳定，证据通过 `criterion_id` 关联。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Criterion {
    pub id: String,
    pub cmd: String,
    #[serde(default)]
    pub expect_exit: i32,
    pub expect_match: Option<String>,
}

/// 任务状态；`merged` 是唯一终态，`needs_human` / `interrupted` 只能人工迁出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Draft,
    Ready,
    Queued,
    Running,
    Verifying,
    Merged,
    Failed,
    NeedsHuman,
    Interrupted,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Draft => "draft",
            TaskStatus::Ready => "ready",
            TaskStatus::Queued => "queued",
            TaskStatus::Running => "running",
            TaskStatus::Verifying => "verifying",
            TaskStatus::Merged => "merged",
            TaskStatus::Failed => "failed",
            TaskStatus::NeedsHuman => "needs_human",
            TaskStatus::Interrupted => "interrupted",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "draft" => Ok(TaskStatus::Draft),
            "ready" => Ok(TaskStatus::Ready),
            "queued" => Ok(TaskStatus::Queued),
            "running" => Ok(TaskStatus::Running),
            "verifying" => Ok(TaskStatus::Verifying),
            "merged" => Ok(TaskStatus::Merged),
            "failed" => Ok(TaskStatus::Failed),
            "needs_human" => Ok(TaskStatus::NeedsHuman),
            "interrupted" => Ok(TaskStatus::Interrupted),
            other => Err(format!("未知任务状态：{other}")),
        }
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for TaskStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        TaskStatus::parse(s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRow {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub status: TaskStatus,
    pub prompt: String,
    pub attempts: u32,
    pub max_minutes: i64,
    pub created_at: String,
    /// 任务级开发者适配器；`None` 表示使用运行上下文里的全局默认。
    #[serde(default)]
    pub developer_adapter: Option<String>,
    /// 任务级审查适配器；`None` 表示回退到开发者适配器。
    #[serde(default)]
    pub reviewer_adapter: Option<String>,
    /// 任务级模型覆盖；`None` 表示使用适配器的默认模型。
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSummary {
    pub id: String,
    pub title: String,
    pub status: TaskStatus,
    pub attempts: u32,
    pub criterion_count: usize,
    pub created_at: String,
    /// 任务级开发者适配器；`None` 表示未指派。
    #[serde(default)]
    pub developer_adapter: Option<String>,
    /// 任务级审查适配器；`None` 表示未指派。
    #[serde(default)]
    pub reviewer_adapter: Option<String>,
    /// 任务级模型覆盖；`None` 表示未指派。
    #[serde(default)]
    pub model: Option<String>,
    /// 是否为合并后自动生成的文档更新任务。
    #[serde(default)]
    pub auto_docs: bool,
}

/// 计划状态：`draft` 尚未开始，`active` 进行中，`done` 全部阶段通过。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Draft,
    Active,
    Done,
}

impl PlanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanStatus::Draft => "draft",
            PlanStatus::Active => "active",
            PlanStatus::Done => "done",
        }
    }

    fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "draft" => Ok(PlanStatus::Draft),
            "active" => Ok(PlanStatus::Active),
            "done" => Ok(PlanStatus::Done),
            other => Err(format!("未知的计划状态：{other}")),
        }
    }
}

/// 阶段状态由读取时推导：已验收 → `passed`；草稿计划或前序阶段未全部通过
/// → `locked`；否则任务全部合并 → `review`，尚有未合并任务 → `active`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    Locked,
    Active,
    Review,
    Passed,
}

/// 计划中的一个任务（`id` 即任务 id）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanTask {
    pub id: String,
    pub title: String,
    pub status: TaskStatus,
}

/// 计划中的一个阶段；`status` 为读取时推导值，不持久化。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStage {
    pub id: String,
    pub seq: i64,
    pub title: String,
    pub status: StageStatus,
    pub tasks: Vec<PlanTask>,
    pub passed_at: Option<String>,
    /// 验收意见；未填写时为 `None`。
    pub summary: Option<String>,
    /// 验收意见写入时间。
    pub summary_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub goal: String,
    pub status: PlanStatus,
    pub stages: Vec<PlanStage>,
    pub created_at: String,
    pub updated_at: String,
}

/// 任务附件元数据；文件本体存放在数据目录的 `attachments/<task_id>/` 下。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub task_id: String,
    pub file_name: String,
    pub mime: String,
    pub size: i64,
    pub created_at: String,
}

/// 创建计划的输入；阶段按数组顺序获得递增 `seq`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewPlan {
    pub project_id: String,
    pub title: String,
    pub goal: String,
    pub stages: Vec<NewPlanStage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewPlanStage {
    pub title: String,
    /// 契约写 `taskIds`，仓库内前端约定 `task_ids`，两种都接受。
    #[serde(alias = "taskIds")]
    pub task_ids: Vec<String>,
}

/// 项目下的一条 issue item（GitHub 同步视图）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectIssue {
    /// item id；本地编辑远端 issue 时使用。
    pub id: String,
    pub title: String,
    pub status: String,
    pub github_state: Option<String>,
    pub github_number: Option<i64>,
    pub ghost: bool,
    /// 关联任务 id；纯 issue（非任务）为 None。
    pub task_id: Option<String>,
}

/// 任务上下文里的一条关联文档条目。
///
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMeta {
    pub id: String,
    pub kind: String,
    pub title: String,
}

/// `body` 来自 `items.body`：文档在保存时同步正文缓存，任务派发时只读这份
/// 缓存，不接触文件系统。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextItem {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRow {
    pub id: String,
    pub status: String,
    pub adapter_id: String,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub pr_number: Option<i64>,
    #[serde(default)]
    pub pr_url: Option<String>,
    #[serde(default)]
    pub pr_state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRow {
    pub id: String,
    pub criterion_id: Option<String>,
    pub kind: String,
    pub cmd: Option<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<i64>,
    pub output_path: Option<String>,
    pub collected_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewRow {
    pub verdict: String,
    pub summary: Option<String>,
    pub risk_flags: Vec<String>,
    pub test_diff: Vec<String>,
}

/// 审查页列表项：review 关联到 run / task，并汇总该 run 的验收证据通过情况
/// （只统计 `criterion_id IS NOT NULL` 的证据，artifact / review 辅助证据不计入，
/// 通过数按 `exit_code = 0` 计）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewListItem {
    pub run_id: String,
    pub task_id: String,
    pub task_title: String,
    pub verdict: String,
    pub summary: Option<String>,
    pub risk_flags: Vec<String>,
    pub test_diff: Vec<String>,
    pub created_at: String,
    pub evidence_total: i64,
    pub evidence_passed: i64,
    pub pr_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRow {
    pub id: String,
    pub task_id: Option<String>,
    pub kind: String,
    pub prompt: String,
    pub status: String,
    pub created_at: String,
    pub resolution: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRow {
    pub id: String,
    pub kind: String,
    pub payload_json: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDetail {
    pub task: TaskRow,
    pub criteria: Vec<Criterion>,
    pub runs: Vec<RunRow>,
    pub evidence: Vec<EvidenceRow>,
    pub review: Option<ReviewRow>,
    pub decisions: Vec<DecisionRow>,
    pub events: Vec<EventRow>,
}

// ---------------------------------------------------------------------------
// 写入入参
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewTask {
    pub project_id: String,
    pub title: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub criteria: Vec<Criterion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewRun {
    /// 调用方预先分配的 run id（与 worktree / 分支名一致）；
    /// 为 `None` 时由存储层生成。
    #[serde(default)]
    pub id: Option<String>,
    pub task_id: String,
    pub adapter_id: String,
    #[serde(default)]
    pub adapter_version: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub worktree_path: Option<String>,
    #[serde(default)]
    pub base_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewEvidence {
    pub run_id: String,
    #[serde(default)]
    pub criterion_id: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub cmd: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub output_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewReview {
    pub run_id: String,
    pub verdict: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub risk_flags: Vec<String>,
    #[serde(default)]
    pub test_diff: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewDecision {
    pub project_id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    pub kind: String,
    pub prompt: String,
    #[serde(default)]
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewEvent {
    pub project_id: String,
    pub entity: String,
    pub entity_id: String,
    pub kind: String,
    #[serde(default = "empty_payload")]
    pub payload: Value,
}

fn empty_payload() -> Value {
    Value::Object(serde_json::Map::new())
}

// ---------------------------------------------------------------------------
// 共享查询
// ---------------------------------------------------------------------------

fn query_task_summaries(
    conn: &Connection,
    project_id: Option<&str>,
) -> Result<Vec<TaskSummary>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT t.id, i.title, i.status, t.attempts, t.acceptance_json, t.created_at,
                    t.developer_adapter, t.reviewer_adapter, t.model, t.auto_docs
             FROM tasks t JOIN items i ON i.id = t.item_id
             WHERE (?1 IS NULL OR i.project_id = ?1)
             ORDER BY t.created_at DESC, t.rowid DESC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![project_id], read_task_summary)
        .map_err(db_err)?;

    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

// ---------------------------------------------------------------------------
// 行读取与辅助
// ---------------------------------------------------------------------------

fn read_task_summary(row: &Row<'_>) -> rusqlite::Result<TaskSummary> {
    let acceptance_json: String = row.get(4)?;
    let criteria: Vec<Criterion> = serde_json::from_str(&acceptance_json)
        .map_err(|e| invalid_value(4, format!("验收标准 JSON 解析失败：{e}")))?;
    Ok(TaskSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        status: read_status(row, 2)?,
        attempts: row.get(3)?,
        criterion_count: criteria.len(),
        created_at: row.get(5)?,
        developer_adapter: row.get(6)?,
        reviewer_adapter: row.get(7)?,
        model: row.get(8)?,
        auto_docs: row.get(9)?,
    })
}

fn read_task_row(row: &Row<'_>) -> rusqlite::Result<TaskRow> {
    Ok(TaskRow {
        id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        status: read_status(row, 3)?,
        prompt: row.get(4)?,
        attempts: row.get(5)?,
        max_minutes: row.get::<_, i64>(6)?,
        created_at: row.get(7)?,
        developer_adapter: row.get(8)?,
        reviewer_adapter: row.get(9)?,
        model: row.get(10)?,
    })
}

fn read_run_row(row: &Row<'_>) -> rusqlite::Result<RunRow> {
    Ok(RunRow {
        id: row.get(0)?,
        status: row.get(1)?,
        adapter_id: row.get(2)?,
        branch: row.get(3)?,
        worktree_path: row.get(4)?,
        started_at: row.get(5)?,
        ended_at: row.get(6)?,
        error: row.get(7)?,
        pr_number: row.get(8)?,
        pr_url: row.get(9)?,
        pr_state: row.get(10)?,
    })
}

fn read_evidence_row(row: &Row<'_>) -> rusqlite::Result<EvidenceRow> {
    Ok(EvidenceRow {
        id: row.get(0)?,
        criterion_id: row.get(1)?,
        kind: row.get(2)?,
        cmd: row.get(3)?,
        exit_code: row.get(4)?,
        duration_ms: row.get(5)?,
        output_path: row.get(6)?,
        collected_at: row.get(7)?,
    })
}

fn read_review_row(row: &Row<'_>) -> rusqlite::Result<ReviewRow> {
    let risk_flags_json: String = row.get(2)?;
    let test_diff_json: String = row.get(3)?;
    Ok(ReviewRow {
        verdict: row.get(0)?,
        summary: row.get(1)?,
        risk_flags: parse_string_list(&risk_flags_json).map_err(|e| invalid_value(2, e))?,
        test_diff: parse_string_list(&test_diff_json).map_err(|e| invalid_value(3, e))?,
    })
}

fn read_review_list_item(row: &Row<'_>) -> rusqlite::Result<ReviewListItem> {
    let risk_flags_json: String = row.get(5)?;
    let test_diff_json: String = row.get(6)?;
    Ok(ReviewListItem {
        run_id: row.get(0)?,
        task_id: row.get(1)?,
        task_title: row.get(2)?,
        verdict: row.get(3)?,
        summary: row.get(4)?,
        risk_flags: parse_string_list(&risk_flags_json).map_err(|e| invalid_value(5, e))?,
        test_diff: parse_string_list(&test_diff_json).map_err(|e| invalid_value(6, e))?,
        created_at: row.get(7)?,
        evidence_total: row.get(8)?,
        evidence_passed: row.get(9)?,
        pr_url: row.get(10)?,
    })
}

fn read_decision_row(row: &Row<'_>) -> rusqlite::Result<DecisionRow> {
    Ok(DecisionRow {
        id: row.get(0)?,
        task_id: row.get(1)?,
        kind: row.get(2)?,
        prompt: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
        resolution: row.get(6)?,
    })
}

fn read_event_row(row: &Row<'_>) -> rusqlite::Result<EventRow> {
    Ok(EventRow {
        id: row.get(0)?,
        kind: row.get(1)?,
        payload_json: row.get(2)?,
        at: row.get(3)?,
    })
}

fn read_status(row: &Row<'_>, idx: usize) -> rusqlite::Result<TaskStatus> {
    let raw: String = row.get(idx)?;
    TaskStatus::parse(&raw).map_err(|message| invalid_value(idx, message))
}

fn parse_criteria(raw: &str) -> Result<Vec<Criterion>, String> {
    serde_json::from_str(raw).map_err(|e| format!("验收标准 JSON 解析失败：{e}"))
}

fn parse_string_list(raw: &str) -> Result<Vec<String>, String> {
    serde_json::from_str(raw).map_err(|e| format!("JSON 数组解析失败：{e}"))
}

fn validate_criteria(criteria: &[Criterion]) -> Result<(), String> {
    let mut seen = HashSet::with_capacity(criteria.len());
    for criterion in criteria {
        if !is_valid_criterion_id(&criterion.id) {
            return Err(format!(
                "验收标准 id 非法：{}（仅允许 A-Za-z0-9_-，长度 1-40）",
                criterion.id
            ));
        }
        if criterion.cmd.trim().is_empty() {
            return Err(format!("验收标准 {} 缺少 cmd", criterion.id));
        }
        if !seen.insert(criterion.id.as_str()) {
            return Err(format!("验收标准 id 重复：{}", criterion.id));
        }
    }
    Ok(())
}

/// 验收标准 id 只允许 `[A-Za-z0-9_-]`，长度 1–40。
fn is_valid_criterion_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn ensure_project(conn: &Connection, project_id: &str) -> Result<(), String> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
            params![project_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if exists {
        Ok(())
    } else {
        Err(format!("项目不存在：{project_id}"))
    }
}

fn ensure_task(conn: &Connection, task_id: &str) -> Result<(), String> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE id = ?1)",
            params![task_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if exists {
        Ok(())
    } else {
        Err(format!("任务不存在：{task_id}"))
    }
}

fn ensure_run(conn: &Connection, run_id: &str) -> Result<(), String> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE id = ?1)",
            params![run_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if exists {
        Ok(())
    } else {
        Err(format!("运行不存在：{run_id}"))
    }
}

fn read_run(conn: &Connection, run_id: &str) -> Result<RunRow, String> {
    conn.query_row(
        "SELECT id, status, adapter_id, branch, worktree_path, started_at, ended_at, error,
                pr_number, pr_url, pr_state
         FROM runs WHERE id = ?1",
        params![run_id],
        read_run_row,
    )
    .map_err(db_err)
}

fn read_decision(conn: &Connection, decision_id: &str) -> Result<DecisionRow, String> {
    conn.query_row(
        "SELECT id, task_id, kind, prompt, status, created_at, resolution
         FROM decisions WHERE id = ?1",
        params![decision_id],
        read_decision_row,
    )
    .map_err(db_err)
}

fn db_err(error: rusqlite::Error) -> String {
    error.to_string()
}

fn invalid_value(idx: usize, message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        idx,
        Type::Text,
        Box::new(std::io::Error::other(message)),
    )
}
