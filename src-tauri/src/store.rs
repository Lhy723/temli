//! 存储层：任务 / 运行 / 证据 / 审查 / 决策 / 事件，以及任务状态机。
//!
//! 表结构由 `db.rs` 的迁移创建（迁移 v2 增加 `tasks.prompt`）；状态机与不变量
//! 见 `docs/data-model.md` §3、§4。`tasks` 表没有状态列，任务状态单点存放在关联
//! issue item 的 `items.status` 上，因此所有迁移都必须经由 [`transition`]。

use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use uuid::Uuid;

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
// 任务
// ---------------------------------------------------------------------------

/// 新建任务：一个事务内写入 `items(kind='issue', status='draft')` 与 `tasks` 行。
pub fn create_task(conn: &Connection, input: NewTask) -> Result<TaskRow, String> {
    let NewTask {
        project_id,
        title,
        prompt,
        criteria,
    } = input;
    validate_criteria(&criteria)?;
    ensure_project(conn, &project_id)?;

    let item_id = Uuid::new_v4().to_string();
    let task_id = Uuid::new_v4().to_string();
    let acceptance_json =
        serde_json::to_string(&criteria).map_err(|e| format!("验收标准序列化失败：{e}"))?;

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO items (id, project_id, kind, title, status)
         VALUES (?1, ?2, 'issue', ?3, 'draft')",
        params![item_id, project_id, title],
    )
    .map_err(db_err)?;
    tx.execute(
        "INSERT INTO tasks (id, item_id, acceptance_json, prompt)
         VALUES (?1, ?2, ?3, ?4)",
        params![task_id, item_id, acceptance_json, prompt],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;

    conn.query_row(
        "SELECT t.id, i.project_id, i.title, i.status, t.prompt, t.attempts, t.max_minutes,
                t.created_at, t.developer_adapter, t.reviewer_adapter, t.model
         FROM tasks t JOIN items i ON i.id = t.item_id
         WHERE t.id = ?1",
        params![task_id],
        read_task_row,
    )
    .map_err(db_err)
}

/// 把已有的 issue item 转换为任务：item 行保持原样，只把 `status` 置为
/// `draft`，并在同一事务内写入 `tasks` 行。
///
/// 与 [`create_task`] 不同，这里复用同步进来的 item（`github_number` / `body`
/// 等字段不受影响），因此要求 item 存在、`kind='issue'` 且尚未关联任务。
pub fn create_task_for_item(
    conn: &Connection,
    item_id: &str,
    prompt: &str,
    criteria: Vec<Criterion>,
) -> Result<TaskRow, String> {
    validate_criteria(&criteria)?;

    let (project_id, kind) = conn
        .query_row(
            "SELECT project_id, kind FROM items WHERE id = ?1",
            params![item_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("条目不存在：{item_id}"))?;
    if kind != "issue" {
        return Err(format!("只能将 issue 条目转换为任务：{item_id}"));
    }
    ensure_project(conn, &project_id)?;

    let existing: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE item_id = ?1)",
            params![item_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if existing {
        return Err(format!("条目已有任务：{item_id}"));
    }

    let task_id = Uuid::new_v4().to_string();
    let acceptance_json =
        serde_json::to_string(&criteria).map_err(|e| format!("验收标准序列化失败：{e}"))?;

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO tasks (id, item_id, acceptance_json, prompt)
         VALUES (?1, ?2, ?3, ?4)",
        params![task_id, item_id, acceptance_json, prompt],
    )
    .map_err(db_err)?;
    tx.execute(
        "UPDATE items
         SET status = 'draft', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        params![item_id],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;

    conn.query_row(
        "SELECT t.id, i.project_id, i.title, i.status, t.prompt, t.attempts, t.max_minutes,
                t.created_at, t.developer_adapter, t.reviewer_adapter, t.model
         FROM tasks t JOIN items i ON i.id = t.item_id
         WHERE t.id = ?1",
        params![task_id],
        read_task_row,
    )
    .map_err(db_err)
}

/// 为已合并任务排队一个自动文档任务。
///
/// 与普通任务不同：新 item 直接以 `queued` 落库（无需经过 draft/ready），任务行
/// 标记 `auto_docs=1`（合并后不再递归调度），并通过 `derived_from`
/// （父任务 item → 新 item）记录来源。父任务、项目或非空验收标准任一缺失都拒绝。
pub fn create_docs_task(
    conn: &Connection,
    project_id: &str,
    parent_task_id: &str,
    title: &str,
    prompt: &str,
    criteria: Vec<Criterion>,
) -> Result<TaskDetail, String> {
    if criteria.is_empty() {
        return Err("自动文档任务必须包含验收标准".to_string());
    }
    validate_criteria(&criteria)?;
    ensure_project(conn, project_id)?;

    let (parent_item, parent_project) = conn
        .query_row(
            "SELECT t.item_id, i.project_id
             FROM tasks t JOIN items i ON i.id = t.item_id
             WHERE t.id = ?1",
            params![parent_task_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("任务不存在：{parent_task_id}"))?;
    if parent_project != project_id {
        return Err(format!("父任务不属于该项目：{parent_task_id}"));
    }

    let item_id = Uuid::new_v4().to_string();
    let task_id = Uuid::new_v4().to_string();
    let acceptance_json =
        serde_json::to_string(&criteria).map_err(|e| format!("验收标准序列化失败：{e}"))?;

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO items (id, project_id, kind, title, status)
         VALUES (?1, ?2, 'issue', ?3, 'queued')",
        params![item_id, project_id, title],
    )
    .map_err(db_err)?;
    tx.execute(
        "INSERT INTO tasks (id, item_id, acceptance_json, prompt, auto_docs)
         VALUES (?1, ?2, ?3, ?4, 1)",
        params![task_id, item_id, acceptance_json, prompt],
    )
    .map_err(db_err)?;
    tx.execute(
        "INSERT INTO links (id, from_item, to_item, rel)
         VALUES (?1, ?2, ?3, 'derived_from')",
        params![Uuid::new_v4().to_string(), parent_item, item_id],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;

    get_task(conn, &task_id)
}

/// 全项目任务汇总；仅测试使用（生产走 [`list_tasks_for_project`]）。
#[cfg(test)]
pub fn list_tasks(conn: &Connection) -> Result<Vec<TaskSummary>, String> {
    query_task_summaries(conn, None)
}

pub fn list_tasks_for_project(
    conn: &Connection,
    project_id: &str,
) -> Result<Vec<TaskSummary>, String> {
    query_task_summaries(conn, Some(project_id))
}

/// 取最旧的排队任务：`items.status='queued'`，按创建时间、任务 id 升序。
///
/// 排序键与调度器的 FIFO 语义一致；同一毫秒创建时用 `t.id` 兜底，保证结果稳定。
/// 仅测试使用（调度器走 [`next_ready_tasks`]）。
#[cfg(test)]
pub fn next_queued_task(conn: &Connection) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT t.id
         FROM tasks t JOIN items i ON i.id = t.item_id
         WHERE i.status = 'queued'
         ORDER BY i.created_at ASC, t.id ASC
         LIMIT 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)
}

/// 读取任务当前状态，供调度器做轻量轮询（避免拉取完整 [`TaskDetail`]）。
pub fn task_status(conn: &Connection, task_id: &str) -> Result<TaskStatus, String> {
    let raw = conn
        .query_row(
            "SELECT i.status FROM tasks t JOIN items i ON i.id = t.item_id WHERE t.id = ?1",
            params![task_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("任务不存在：{task_id}"))?;
    TaskStatus::parse(&raw)
}

/// 任务对应的 issue item id；依赖边存放在 `items` 之间，写入前需先做映射。
pub fn task_item_id(conn: &Connection, task_id: &str) -> Result<String, String> {
    conn.query_row(
        "SELECT item_id FROM tasks WHERE id = ?1",
        params![task_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("任务不存在：{task_id}"))
}

/// 任务是否为自动文档任务；默认 `false`。
///
/// 自动文档任务合并后绝不能再调度新的文档任务，否则会形成无限递归。
pub fn task_auto_docs(conn: &Connection, task_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT COALESCE(auto_docs, 0) FROM tasks WHERE id = ?1",
        params![task_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("任务不存在：{task_id}"))
}

/// 读取任务级 agent 指派：`(developer_adapter, reviewer_adapter)`。
///
/// `None` 表示该角色未指派，由运行器回退到全局默认（审查角色回退到开发者）。
pub fn task_agents(
    conn: &Connection,
    task_id: &str,
) -> Result<(Option<String>, Option<String>), String> {
    conn.query_row(
        "SELECT developer_adapter, reviewer_adapter FROM tasks WHERE id = ?1",
        params![task_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("任务不存在：{task_id}"))
}

/// 写入任务级 agent 指派；`None` 清除指派并恢复默认。
///
/// 只校验 id 非空——适配器是否注册由运行器按注册表判断，存储层不持有注册表。
pub fn set_task_agents(
    conn: &Connection,
    task_id: &str,
    developer: Option<&str>,
    reviewer: Option<&str>,
) -> Result<(), String> {
    let developer = normalize_agent_id("开发者", developer)?;
    let reviewer = normalize_agent_id("审查者", reviewer)?;
    ensure_task(conn, task_id)?;
    conn.execute(
        "UPDATE tasks SET developer_adapter = ?2, reviewer_adapter = ?3 WHERE id = ?1",
        params![task_id, developer, reviewer],
    )
    .map_err(db_err)?;
    Ok(())
}

/// 写入任务级模型覆盖；`None` 或空白串清除覆盖，回退到适配器默认模型。
///
/// 只校验任务存在——模型名是否被具体适配器接受由运行器/适配器判断。
pub fn set_task_model(conn: &Connection, task_id: &str, model: Option<&str>) -> Result<(), String> {
    let model = match model {
        None => None,
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
    };
    ensure_task(conn, task_id)?;
    conn.execute(
        "UPDATE tasks SET model = ?2 WHERE id = ?1",
        params![task_id, model],
    )
    .map_err(db_err)?;
    Ok(())
}

fn normalize_agent_id(role: &str, id: Option<&str>) -> Result<Option<String>, String> {
    match id {
        None => Ok(None),
        Some(raw) => {
            let id = raw.trim();
            if id.is_empty() {
                Err(format!("{role}适配器 id 不能为空"))
            } else {
                Ok(Some(id.to_string()))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 依赖与调度
// ---------------------------------------------------------------------------

/// 登记 `task_id` 依赖 `depends_on_task_id`。
///
/// 依赖只存一条 `links(rel='blocks')` 边：`from_item` 为前置任务，
/// `to_item` 为被阻塞任务；不复制状态，读取一律走递归闭包。
pub fn add_dependency(
    conn: &Connection,
    task_id: &str,
    depends_on_task_id: &str,
) -> Result<(), String> {
    if task_id == depends_on_task_id {
        return Err("任务不能依赖自身".to_string());
    }

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let item_id = task_item_id(&tx, task_id)?;
    let depends_on_item_id = task_item_id(&tx, depends_on_task_id)?;

    let (project_id, depends_on_project_id) = tx
        .query_row(
            "SELECT task.project_id, blocker.project_id
             FROM items task, items blocker
             WHERE task.id = ?1 AND blocker.id = ?2",
            params![item_id, depends_on_item_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(db_err)?;
    if project_id != depends_on_project_id {
        return Err("依赖只能建立在同一项目的任务之间".to_string());
    }

    // 沿依赖方向（`to_item` -> `from_item`）求闭包：若前置任务已经（传递地）
    // 依赖当前任务，再加入这条边就会成环。
    let creates_cycle: bool = tx
        .query_row(
            "WITH RECURSIVE closure(item_id) AS (
                 SELECT ?1
                 UNION
                 SELECT l.from_item
                 FROM links l JOIN closure c ON l.to_item = c.item_id
                 WHERE l.rel = 'blocks'
             )
             SELECT EXISTS(SELECT 1 FROM closure WHERE item_id = ?2)",
            params![depends_on_item_id, item_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if creates_cycle {
        return Err("依赖关系会形成环".to_string());
    }

    tx.execute(
        "INSERT INTO links (id, from_item, to_item, rel)
         VALUES (?1, ?2, ?3, 'blocks')
         ON CONFLICT (from_item, to_item, rel) DO NOTHING",
        params![Uuid::new_v4().to_string(), depends_on_item_id, item_id],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)
}

/// 删除依赖边；边不存在时视为已经删除（幂等）。
pub fn remove_dependency(
    conn: &Connection,
    task_id: &str,
    depends_on_task_id: &str,
) -> Result<(), String> {
    let item_id = task_item_id(conn, task_id)?;
    let depends_on_item_id = task_item_id(conn, depends_on_task_id)?;
    conn.execute(
        "DELETE FROM links
         WHERE from_item = ?1 AND to_item = ?2 AND rel = 'blocks'",
        params![depends_on_item_id, item_id],
    )
    .map_err(db_err)?;
    Ok(())
}

/// 返回 `(blockers, blocks)`：前者是当前任务的直接前置，后者是直接依赖当前任务的任务。
pub fn task_dependencies(
    conn: &Connection,
    task_id: &str,
) -> Result<(Vec<TaskSummary>, Vec<TaskSummary>), String> {
    let item_id = task_item_id(conn, task_id)?;
    let blockers = query_related_tasks(
        conn,
        "SELECT t.id, i.title, i.status, t.attempts, t.acceptance_json, t.created_at,
                t.developer_adapter, t.reviewer_adapter, t.model, t.auto_docs
         FROM links l
         JOIN tasks t ON t.item_id = l.from_item
         JOIN items i ON i.id = t.item_id
         WHERE l.to_item = ?1 AND l.rel = 'blocks'
         ORDER BY i.created_at ASC, t.id ASC",
        &item_id,
    )?;
    let blocks = query_related_tasks(
        conn,
        "SELECT t.id, i.title, i.status, t.attempts, t.acceptance_json, t.created_at,
                t.developer_adapter, t.reviewer_adapter, t.model, t.auto_docs
         FROM links l
         JOIN tasks t ON t.item_id = l.to_item
         JOIN items i ON i.id = t.item_id
         WHERE l.from_item = ?1 AND l.rel = 'blocks'
         ORDER BY i.created_at ASC, t.id ASC",
        &item_id,
    )?;
    Ok((blockers, blocks))
}

fn query_related_tasks(
    conn: &Connection,
    sql: &str,
    item_id: &str,
) -> Result<Vec<TaskSummary>, String> {
    let mut stmt = conn.prepare(sql).map_err(db_err)?;
    let rows = stmt
        .query_map(params![item_id], read_task_summary)
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

// ---------------------------------------------------------------------------
// 知识上下文
// ---------------------------------------------------------------------------

/// `links.rel` 的合法取值，与数据库 CHECK 约束保持一致。
const LINK_RELS: &[&str] = &["derived_from", "implements", "blocks", "relates"];

fn item_project(conn: &Connection, item_id: &str) -> Result<String, String> {
    conn.query_row(
        "SELECT project_id FROM items WHERE id = ?1",
        params![item_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("条目不存在：{item_id}"))
}

/// 建立条目链接（`derived_from` / `implements` / `blocks` / `relates`）。
///
/// 只允许同一项目内的条目互相链接；重复链接按幂等处理。任务依赖边请走
/// [`add_dependency`]，那里还会做环检测。
pub fn link_item(
    conn: &Connection,
    from_item: &str,
    to_item: &str,
    rel: &str,
) -> Result<(), String> {
    if !LINK_RELS.contains(&rel) {
        return Err(format!("未知链接类型：{rel}"));
    }

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let from_project = item_project(&tx, from_item)?;
    let to_project = item_project(&tx, to_item)?;
    if from_project != to_project {
        return Err("链接只能建立在同一项目的条目之间".to_string());
    }
    tx.execute(
        "INSERT INTO links (id, from_item, to_item, rel)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (from_item, to_item, rel) DO NOTHING",
        params![Uuid::new_v4().to_string(), from_item, to_item, rel],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)
}

/// 删除条目链接；边不存在时视为已经删除（幂等）。
pub fn unlink_item(
    conn: &Connection,
    from_item: &str,
    to_item: &str,
    rel: &str,
) -> Result<(), String> {
    if !LINK_RELS.contains(&rel) {
        return Err(format!("未知链接类型：{rel}"));
    }
    conn.execute(
        "DELETE FROM links
         WHERE from_item = ?1 AND to_item = ?2 AND rel = ?3",
        params![from_item, to_item, rel],
    )
    .map_err(db_err)?;
    Ok(())
}

/// 任务条目通过 `links(rel='relates')` 关联的文档条目，按创建时间稳定排序。
///
/// 只读取数据库缓存正文，只返回与任务同项目且 `kind='doc'` 的条目；笔记已迁往
/// 独立 vault，不再注入任务上下文。派发任务时由 orchestrator 截断后注入 prompt。
pub fn task_context_meta(conn: &Connection, task_id: &str) -> Result<Vec<ContextMeta>, String> {
    let item_id = task_item_id(conn, task_id)?;
    let mut stmt = conn
        .prepare(
            "SELECT i.id, i.kind, i.title
             FROM links l
             JOIN items i ON i.id = l.to_item
             JOIN items task_item ON task_item.id = l.from_item
             WHERE l.from_item = ?1 AND l.rel = 'relates'
               AND i.project_id = task_item.project_id
               AND i.kind = 'doc'
             ORDER BY i.created_at ASC, i.id ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![item_id], |row| {
            Ok(ContextMeta {
                id: row.get(0)?,
                kind: row.get(1)?,
                title: row.get(2)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

pub fn task_context(conn: &Connection, task_id: &str) -> Result<Vec<ContextItem>, String> {
    let item_id = task_item_id(conn, task_id)?;
    let mut stmt = conn
        .prepare(
            "SELECT i.id, i.kind, i.title, i.body
             FROM links l
             JOIN items i ON i.id = l.to_item
             JOIN items task_item ON task_item.id = l.from_item
             WHERE l.from_item = ?1 AND l.rel = 'relates'
               AND i.project_id = task_item.project_id
               AND i.kind = 'doc'
             ORDER BY i.created_at ASC, i.id ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![item_id], |row| {
            Ok(ContextItem {
                id: row.get(0)?,
                kind: row.get(1)?,
                title: row.get(2)?,
                body: row.get(3)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

/// 就绪任务的共享查询：先按 `queued` 状态和可选的项目/任务范围收窄，再对
/// `links(rel='blocks')` 求传递闭包并排除仍被未 `merged` 任务阻塞的候选。
///
/// `project_id` 与 `task_id` 都是可选的收窄条件；两者都不传时返回全部就绪任务。
/// 依赖闭包直接递归，不读取任何物化副本；没有任何依赖的任务天然满足条件。
/// 排序：创建时间、任务 id 升序，与调度器的 FIFO 语义一致。
fn query_ready_tasks(
    conn: &Connection,
    project_id: Option<&str>,
    task_id: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(
            "WITH RECURSIVE queued_tasks(task_id, item_id, created_at) AS (
                 SELECT t.id, t.item_id, i.created_at
                 FROM tasks t JOIN items i ON i.id = t.item_id
                 WHERE i.status = 'queued'
                   AND (?1 IS NULL OR i.project_id = ?1)
                   AND (?2 IS NULL OR t.id = ?2)
             ),
             blockers(task_id, blocker_item) AS (
                 SELECT q.task_id, l.from_item
                 FROM queued_tasks q
                 JOIN links l ON l.to_item = q.item_id AND l.rel = 'blocks'
                 UNION
                 SELECT b.task_id, l.from_item
                 FROM blockers b
                 JOIN tasks bt ON bt.item_id = b.blocker_item
                 JOIN links l ON l.to_item = bt.item_id AND l.rel = 'blocks'
             )
             SELECT q.task_id
             FROM queued_tasks q
             WHERE NOT EXISTS (
                 SELECT 1
                 FROM blockers b
                 JOIN tasks bt ON bt.item_id = b.blocker_item
                 JOIN items bi ON bi.id = bt.item_id
                 WHERE b.task_id = q.task_id AND bi.status <> 'merged'
             )
             ORDER BY q.created_at ASC, q.task_id ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![project_id, task_id], |row| row.get(0))
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

/// 项目内最旧的已就绪任务；仅测试使用（生产走 [`next_ready_tasks`]）。
#[cfg(test)]
pub fn next_ready_task(conn: &Connection, project_id: &str) -> Result<Option<String>, String> {
    Ok(next_ready_tasks(conn, project_id)?.into_iter().next())
}

/// 项目内全部就绪的排队任务，按创建时间、任务 id 升序。
///
/// 就绪判定：关联 item 状态为 `queued`，且传递依赖闭包中不存在状态不是
/// `merged` 的任务。
pub fn next_ready_tasks(conn: &Connection, project_id: &str) -> Result<Vec<String>, String> {
    query_ready_tasks(conn, Some(project_id), None)
}

/// 判断单个任务当前是否就绪：状态为 `queued` 且传递依赖全部 `merged`。
///
/// 与 [`next_ready_task`] 复用同一谓词；任务不存在或不在 `queued` 状态时返回
/// `false`。
pub fn task_ready(conn: &Connection, task_id: &str) -> Result<bool, String> {
    Ok(!query_ready_tasks(conn, None, Some(task_id))?.is_empty())
}

/// 读取全局设置；键不存在时返回 `None`。
pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT value FROM app_settings WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)
}

/// 写入全局设置（存在则覆盖并刷新 `updated_at`）。
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET
             value = excluded.value,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        params![key, value],
    )
    .map_err(db_err)?;
    Ok(())
}

/// 删除全局设置；键不存在时视为成功。
pub fn delete_setting(conn: &Connection, key: &str) -> Result<(), String> {
    conn.execute("DELETE FROM app_settings WHERE key = ?1", params![key])
        .map_err(db_err)?;
    Ok(())
}

/// 孤儿 worktree 的保留天数：读不到或非法时回退 7 天，并收敛到 `1..=90`。
pub fn worktree_retention_days(conn: &Connection) -> u32 {
    get_setting(conn, SETTING_WORKTREE_RETENTION_DAYS)
        .ok()
        .flatten()
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .unwrap_or(DEFAULT_WORKTREE_RETENTION_DAYS)
        .clamp(1, 90)
}

/// 开发者模式是否开启；只有精确的 `"1"` 视为开启。
pub fn dev_mode_enabled(conn: &Connection) -> bool {
    get_setting(conn, SETTING_DEV_MODE)
        .ok()
        .flatten()
        .is_some_and(|raw| raw.trim() == "1")
}

/// 开发者 API 端口：缺失或非法时回退默认端口。
pub fn dev_port(conn: &Connection) -> u16 {
    get_setting(conn, SETTING_DEV_PORT)
        .ok()
        .flatten()
        .and_then(|raw| raw.trim().parse::<u16>().ok())
        .filter(|port| *port >= MIN_DEV_PORT)
        .unwrap_or(DEFAULT_DEV_PORT)
}

/// 读取开发者 API token；不存在时生成 UUID v4 并持久化。
pub fn ensure_dev_api_token(conn: &Connection) -> Result<String, String> {
    if let Some(token) = get_setting(conn, SETTING_DEV_API_TOKEN)?
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
    {
        return Ok(token);
    }
    let token = Uuid::new_v4().to_string();
    set_setting(conn, SETTING_DEV_API_TOKEN, &token)?;
    Ok(token)
}

/// 全库任务按状态计数，按状态名排序；没有任务时返回空列表。
pub fn task_status_counts(conn: &Connection) -> Result<Vec<(String, u32)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT i.status, COUNT(*)
             FROM tasks t JOIN items i ON i.id = t.item_id
             GROUP BY i.status
             ORDER BY i.status ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

/// 项目的并发上限；列为 NULL 时回落为 1。
pub fn project_concurrency(conn: &Connection, project_id: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(concurrency_limit, 1) FROM projects WHERE id = ?1",
        params![project_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("项目不存在：{project_id}"))
}

/// 设置项目并发上限，越界值收敛到 `1..=16`。
pub fn set_project_concurrency(
    conn: &Connection,
    project_id: &str,
    limit: i64,
) -> Result<(), String> {
    let limit = limit.clamp(1, 16);
    let changed = conn
        .execute(
            "UPDATE projects SET concurrency_limit = ?2 WHERE id = ?1",
            params![project_id, limit],
        )
        .map_err(db_err)?;
    if changed == 0 {
        return Err(format!("项目不存在：{project_id}"));
    }
    Ok(())
}

/// 项目是否启用 GitHub 自动合并；默认关闭。
pub fn project_auto_merge(conn: &Connection, project_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT COALESCE(auto_merge, 0) FROM projects WHERE id = ?1",
        params![project_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("项目不存在：{project_id}"))
}

pub fn set_project_auto_merge(
    conn: &Connection,
    project_id: &str,
    enabled: bool,
) -> Result<(), String> {
    let changed = conn
        .execute(
            "UPDATE projects SET auto_merge = ?2 WHERE id = ?1",
            params![project_id, enabled],
        )
        .map_err(db_err)?;
    if changed == 0 {
        return Err(format!("项目不存在：{project_id}"));
    }
    Ok(())
}

/// 项目是否在任务合并后自动排队文档更新任务；默认关闭。
pub fn project_auto_docs(conn: &Connection, project_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT COALESCE(auto_docs, 0) FROM projects WHERE id = ?1",
        params![project_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("项目不存在：{project_id}"))
}

pub fn set_project_auto_docs(
    conn: &Connection,
    project_id: &str,
    enabled: bool,
) -> Result<(), String> {
    let changed = conn
        .execute(
            "UPDATE projects SET auto_docs = ?2 WHERE id = ?1",
            params![project_id, enabled],
        )
        .map_err(db_err)?;
    if changed == 0 {
        return Err(format!("项目不存在：{project_id}"));
    }
    Ok(())
}

/// 项目是否强制要求 PR 检查全部通过；默认关闭，此时空检查列表视为通过。
pub fn project_require_checks(conn: &Connection, project_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT COALESCE(require_checks, 0) FROM projects WHERE id = ?1",
        params![project_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("项目不存在：{project_id}"))
}

pub fn set_project_require_checks(
    conn: &Connection,
    project_id: &str,
    enabled: bool,
) -> Result<(), String> {
    let changed = conn
        .execute(
            "UPDATE projects SET require_checks = ?2 WHERE id = ?1",
            params![project_id, enabled],
        )
        .map_err(db_err)?;
    if changed == 0 {
        return Err(format!("项目不存在：{project_id}"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 计划
// ---------------------------------------------------------------------------

/// 创建计划：一个事务内写入计划、阶段与任务引用。
///
/// 校验：至少一个阶段、每阶段至少一个任务、任务存在且属于同一项目、
/// 任务尚未属于任何计划（`plan_tasks.task_id` 全局唯一）。
pub fn create_plan(conn: &Connection, input: NewPlan) -> Result<Plan, String> {
    let NewPlan {
        project_id,
        title,
        goal,
        stages,
    } = input;
    ensure_project(conn, &project_id)?;
    if stages.is_empty() {
        return Err("计划至少需要一个阶段".to_string());
    }

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let plan_id = Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO plans (id, project_id, title, goal, status)
         VALUES (?1, ?2, ?3, ?4, 'draft')",
        params![plan_id, project_id, title, goal],
    )
    .map_err(db_err)?;

    let mut seen: HashSet<&str> = HashSet::new();
    for (index, stage) in stages.iter().enumerate() {
        if stage.task_ids.is_empty() {
            return Err(format!("阶段至少需要一个任务：{}", stage.title));
        }
        let stage_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO plan_stages (id, plan_id, seq, title) VALUES (?1, ?2, ?3, ?4)",
            params![stage_id, plan_id, index as i64 + 1, stage.title],
        )
        .map_err(db_err)?;

        for (position, task_id) in stage.task_ids.iter().enumerate() {
            if !seen.insert(task_id.as_str()) {
                return Err(format!("任务重复出现在计划中：{task_id}"));
            }
            let task_project: Option<String> = tx
                .query_row(
                    "SELECT i.project_id
                     FROM tasks t JOIN items i ON i.id = t.item_id
                     WHERE t.id = ?1",
                    params![task_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(db_err)?;
            let Some(task_project) = task_project else {
                return Err(format!("任务不存在：{task_id}"));
            };
            if task_project != project_id {
                return Err(format!("任务不属于该项目：{task_id}"));
            }
            let assigned: Option<String> = tx
                .query_row(
                    "SELECT stage_id FROM plan_tasks WHERE task_id = ?1",
                    params![task_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(db_err)?;
            if assigned.is_some() {
                return Err(format!("任务已属于其他计划：{task_id}"));
            }
            tx.execute(
                "INSERT INTO plan_tasks (stage_id, task_id, position) VALUES (?1, ?2, ?3)",
                params![stage_id, task_id, position as i64 + 1],
            )
            .map_err(db_err)?;
        }
    }

    tx.commit().map_err(db_err)?;
    get_plan(conn, &plan_id)
}

/// 项目下的计划列表（按创建时间升序），阶段状态按当前任务状态实时推导。
pub fn list_plans(conn: &Connection, project_id: &str) -> Result<Vec<Plan>, String> {
    let mut stmt = conn
        .prepare("SELECT id FROM plans WHERE project_id = ?1 ORDER BY created_at ASC, id ASC")
        .map_err(db_err)?;
    let ids = stmt
        .query_map(params![project_id], |row| row.get::<_, String>(0))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    ids.iter().map(|id| read_plan(conn, id)).collect()
}

pub fn get_plan(conn: &Connection, plan_id: &str) -> Result<Plan, String> {
    read_plan(conn, plan_id)
}

/// 开始计划：仅 `draft` 可开始，置为 `active`。
pub fn start_plan(conn: &Connection, plan_id: &str) -> Result<Plan, String> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT status FROM plans WHERE id = ?1",
            params![plan_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    let Some(raw) = raw else {
        return Err(format!("计划不存在：{plan_id}"));
    };
    if PlanStatus::parse(&raw)? != PlanStatus::Draft {
        return Err(format!("只有草稿计划可以开始：{plan_id}"));
    }
    conn.execute(
        "UPDATE plans
         SET status = 'active', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        params![plan_id],
    )
    .map_err(db_err)?;
    read_plan(conn, plan_id)
}

/// 验收阶段：仅 `active` 计划的 `review` 阶段可验收（任务全部 `merged`），
/// 写入 `passed_at`；最后一个阶段通过后计划置为 `done`，否则保持 `active`。
/// 草稿计划必须先 `start_plan`，已完成或草稿计划一律拒绝。
pub fn accept_stage(conn: &Connection, stage_id: &str) -> Result<Plan, String> {
    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let plan_id: Option<String> = tx
        .query_row(
            "SELECT plan_id FROM plan_stages WHERE id = ?1",
            params![stage_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    let Some(plan_id) = plan_id else {
        return Err(format!("阶段不存在：{stage_id}"));
    };

    let plan = read_plan(&tx, &plan_id)?;
    match plan.status {
        PlanStatus::Active => {}
        PlanStatus::Draft => return Err(format!("计划尚未启动：{plan_id}")),
        PlanStatus::Done => return Err(format!("计划已完成：{plan_id}")),
    }
    let stage = plan
        .stages
        .iter()
        .find(|stage| stage.id == stage_id)
        .ok_or_else(|| format!("阶段不存在：{stage_id}"))?;
    if stage.status != StageStatus::Review {
        return Err(format!("阶段未处于待验收状态：{stage_id}"));
    }

    tx.execute(
        "UPDATE plan_stages
         SET passed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        params![stage_id],
    )
    .map_err(db_err)?;

    let remaining: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM plan_stages WHERE plan_id = ?1 AND passed_at IS NULL",
            params![plan_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    let status = if remaining == 0 {
        PlanStatus::Done.as_str()
    } else {
        PlanStatus::Active.as_str()
    };
    tx.execute(
        "UPDATE plans
         SET status = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        params![plan_id, status],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;

    read_plan(conn, &plan_id)
}

fn read_plan(conn: &Connection, plan_id: &str) -> Result<Plan, String> {
    let (id, project_id, title, goal, raw_status, created_at, updated_at) = conn
        .query_row(
            "SELECT id, project_id, title, goal, status, created_at, updated_at
             FROM plans WHERE id = ?1",
            params![plan_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("计划不存在：{plan_id}"))?;

    let status = PlanStatus::parse(&raw_status)?;
    Ok(Plan {
        id,
        project_id,
        title,
        goal,
        status,
        stages: read_plan_stages(conn, plan_id, status)?,
        created_at,
        updated_at,
    })
}

/// 按 `seq` 读取阶段并推导状态：草稿计划一律 `locked`；开始后前序阶段
/// 全部 `passed` 才解锁当前阶段。
fn read_plan_stages(
    conn: &Connection,
    plan_id: &str,
    plan_status: PlanStatus,
) -> Result<Vec<PlanStage>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, seq, title, passed_at
             FROM plan_stages WHERE plan_id = ?1 ORDER BY seq ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![plan_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    let mut previous_passed = true;
    let mut stages = Vec::with_capacity(rows.len());
    for (id, seq, title, passed_at) in rows {
        let tasks = read_stage_tasks(conn, &id)?;
        let all_merged =
            !tasks.is_empty() && tasks.iter().all(|task| task.status == TaskStatus::Merged);
        let status = if passed_at.is_some() {
            StageStatus::Passed
        } else if plan_status == PlanStatus::Draft || !previous_passed {
            StageStatus::Locked
        } else if all_merged {
            StageStatus::Review
        } else {
            StageStatus::Active
        };
        previous_passed &= passed_at.is_some();
        stages.push(PlanStage {
            id,
            seq,
            title,
            status,
            tasks,
            passed_at,
        });
    }
    Ok(stages)
}

fn read_stage_tasks(conn: &Connection, stage_id: &str) -> Result<Vec<PlanTask>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT t.id, i.title, i.status
             FROM plan_tasks pt
             JOIN tasks t ON t.id = pt.task_id
             JOIN items i ON i.id = t.item_id
             WHERE pt.stage_id = ?1
             ORDER BY pt.position ASC",
        )
        .map_err(db_err)?;
    let tasks = stmt
        .query_map(params![stage_id], |row| {
            Ok(PlanTask {
                id: row.get(0)?,
                title: row.get(1)?,
                status: read_status(row, 2)?,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(tasks)
}

/// 项目下的 issue item 列表（含本地尚未发布的 issue）。
pub fn list_project_issues(
    conn: &Connection,
    project_id: &str,
) -> Result<Vec<ProjectIssue>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT i.id, i.title, i.status, i.github_state, i.github_number, i.ghost, t.id
             FROM items i LEFT JOIN tasks t ON t.item_id = i.id
             WHERE i.project_id = ?1 AND i.kind = 'issue'
             ORDER BY CASE WHEN i.github_number IS NULL THEN 1 ELSE 0 END,
                      i.github_number ASC, i.created_at ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![project_id], |row| {
            Ok(ProjectIssue {
                id: row.get(0)?,
                title: row.get(1)?,
                status: row.get(2)?,
                github_state: row.get(3)?,
                github_number: row.get(4)?,
                ghost: row.get(5)?,
                task_id: row.get(6)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

/// 项目下最近的审查列表，按审查时间倒序，最多 100 条。
///
/// 证据统计只统计该次 run 的验收证据（`criterion_id IS NOT NULL`）：artifact /
/// review 这类辅助证据不挂在验收标准上，不计入分母。通过数在同一集合上按
/// `exit_code = 0` 计，未取到 exit code 的证据不算通过。
pub fn list_reviews(conn: &Connection, project_id: &str) -> Result<Vec<ReviewListItem>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT rev.run_id, t.id, i.title, rev.verdict, rev.summary,
                    rev.risk_flags_json, rev.test_diff_json, rev.created_at,
                    (SELECT COUNT(*) FROM evidence e
                     WHERE e.run_id = rev.run_id AND e.criterion_id IS NOT NULL),
                    (SELECT COALESCE(SUM(e.exit_code = 0), 0)
                     FROM evidence e
                     WHERE e.run_id = rev.run_id AND e.criterion_id IS NOT NULL),
                    r.pr_url
             FROM reviews rev
             JOIN runs r ON r.id = rev.run_id
             JOIN tasks t ON t.id = r.task_id
             JOIN items i ON i.id = t.item_id
             WHERE i.project_id = ?1
             ORDER BY rev.created_at DESC, rev.rowid DESC
             LIMIT 100",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![project_id], read_review_list_item)
        .map_err(db_err)?;

    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

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

pub fn get_task(conn: &Connection, task_id: &str) -> Result<TaskDetail, String> {
    let (task, acceptance_json) = conn
        .query_row(
            "SELECT t.id, i.project_id, i.title, i.status, t.prompt, t.attempts, t.max_minutes,
                    t.created_at, t.developer_adapter, t.reviewer_adapter, t.model,
                    t.acceptance_json
             FROM tasks t JOIN items i ON i.id = t.item_id
             WHERE t.id = ?1",
            params![task_id],
            |row| Ok((read_task_row(row)?, row.get::<_, String>(11)?)),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("任务不存在：{task_id}"))?;

    let criteria = parse_criteria(&acceptance_json)?;

    let mut run_stmt = conn
        .prepare(
            "SELECT id, status, adapter_id, branch, worktree_path, started_at, ended_at, error,
                    pr_number, pr_url, pr_state
             FROM runs WHERE task_id = ?1 ORDER BY attempt ASC",
        )
        .map_err(db_err)?;
    let runs = run_stmt
        .query_map(params![task_id], read_run_row)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    let mut evidence_stmt = conn
        .prepare(
            "SELECT e.id, e.criterion_id, e.kind, e.cmd, e.exit_code, e.duration_ms,
                    e.output_path, e.collected_at
             FROM evidence e JOIN runs r ON r.id = e.run_id
             WHERE r.task_id = ?1
             ORDER BY e.collected_at ASC, e.rowid ASC",
        )
        .map_err(db_err)?;
    let evidence = evidence_stmt
        .query_map(params![task_id], read_evidence_row)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    let review = conn
        .query_row(
            "SELECT rev.verdict, rev.summary, rev.risk_flags_json, rev.test_diff_json
             FROM reviews rev JOIN runs r ON r.id = rev.run_id
             WHERE r.task_id = ?1
               AND r.attempt = (SELECT MAX(attempt) FROM runs WHERE task_id = ?1)
             LIMIT 1",
            params![task_id],
            read_review_row,
        )
        .optional()
        .map_err(db_err)?;

    let mut decision_stmt = conn
        .prepare(
            "SELECT d.id, d.task_id, d.kind, d.prompt, d.status, d.created_at, d.resolution
             FROM decisions d
             WHERE d.task_id = ?1 OR d.run_id IN (SELECT id FROM runs WHERE task_id = ?1)
             ORDER BY d.created_at ASC, d.rowid ASC",
        )
        .map_err(db_err)?;
    let decisions = decision_stmt
        .query_map(params![task_id], read_decision_row)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    let mut event_stmt = conn
        .prepare(
            "SELECT id, type, payload_json, at FROM events
             WHERE entity_id = ?1
                OR entity_id IN (SELECT id FROM runs WHERE task_id = ?1)
             ORDER BY at ASC, rowid ASC",
        )
        .map_err(db_err)?;
    let events = event_stmt
        .query_map(params![task_id], read_event_row)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    Ok(TaskDetail {
        task,
        criteria,
        runs,
        evidence,
        review,
        decisions,
        events,
    })
}

/// 任务状态迁移，唯一写入口；非法迁移返回 `Err`。
///
/// 边集合即 `docs/data-model.md` §3：`merged` 无出边；`needs_human` / `interrupted`
/// 只能迁回 `queued` / `draft`，`queued` 可取消回 `draft`（由人工触发，但走同一函数）。
/// 进入 `queued` 必须已有验收标准；`failed -> queued` 要求尝试次数未用尽。`merged`
/// 的证据门由 [`merge_ready`] 负责，调用方须先确认再迁移。
pub fn transition(conn: &Connection, task_id: &str, to: TaskStatus) -> Result<TaskStatus, String> {
    let (item_id, raw_status, attempts, acceptance_json) = conn
        .query_row(
            "SELECT t.item_id, i.status, t.attempts, t.acceptance_json
             FROM tasks t JOIN items i ON i.id = t.item_id
             WHERE t.id = ?1",
            params![task_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("任务不存在：{task_id}"))?;

    let from = TaskStatus::parse(&raw_status)?;
    if !allowed_transition(from, to) {
        return Err(format!("非法状态迁移：{from} → {to}"));
    }

    if to == TaskStatus::Queued {
        let criteria = parse_criteria(&acceptance_json)?;
        if criteria.is_empty() {
            return Err("任务缺少验收标准，不能进入 queued".to_string());
        }
    }

    if from == TaskStatus::Failed && to == TaskStatus::Queued && attempts >= MAX_ATTEMPTS {
        return Err(format!(
            "尝试次数已用尽（{attempts}/{MAX_ATTEMPTS}），需要人工处理"
        ));
    }

    let changed = conn
        .execute(
            "UPDATE items
             SET status = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?1",
            params![item_id, to.as_str()],
        )
        .map_err(db_err)?;
    if changed == 0 {
        return Err(format!("任务项不存在：{task_id}"));
    }

    Ok(to)
}

/// §3 的迁移表；`Merged` 是终态，没有出边。
fn allowed_transition(from: TaskStatus, to: TaskStatus) -> bool {
    use TaskStatus::*;
    matches!(
        (from, to),
        (Draft, Ready)
            | (Ready, Queued)
            | (Queued, Running)
            | (Queued, Draft)
            | (Running, Verifying)
            | (Running, Failed)
            | (Running, Interrupted)
            | (Verifying, Merged)
            | (Verifying, NeedsHuman)
            | (Verifying, Failed)
            | (Verifying, Interrupted)
            | (Failed, Queued)
            | (Failed, NeedsHuman)
            | (Interrupted, Queued)
            | (Interrupted, Draft)
            | (NeedsHuman, Queued)
            | (NeedsHuman, Draft)
    )
}

// ---------------------------------------------------------------------------
// 运行
// ---------------------------------------------------------------------------

/// 建立一次运行：attempt 自增，`tasks.attempts` 同步为本次 attempt。
///
/// `NewRun.id` 可传入调用方预先生成的 id（worktree、分支名都以它为准）；
/// 未提供时由存储层生成。
pub fn start_run(conn: &Connection, input: NewRun) -> Result<RunRow, String> {
    let NewRun {
        id,
        task_id,
        adapter_id,
        adapter_version,
        model,
        branch,
        worktree_path,
        base_sha,
    } = input;
    ensure_task(conn, &task_id)?;

    let run_id = match id {
        Some(id) => {
            if id.trim().is_empty() {
                return Err("运行 id 不能为空".to_string());
            }
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM runs WHERE id = ?1)",
                    params![id],
                    |row| row.get(0),
                )
                .map_err(db_err)?;
            if exists {
                return Err(format!("运行 id 已存在：{id}"));
            }
            id
        }
        None => Uuid::new_v4().to_string(),
    };

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let attempt: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(attempt), 0) + 1 FROM runs WHERE task_id = ?1",
            params![task_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    tx.execute(
        "INSERT INTO runs (id, task_id, attempt, adapter_id, adapter_version, model,
                           worktree_path, branch, base_sha, status, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'running',
                 strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
        params![
            run_id,
            task_id,
            attempt,
            adapter_id,
            adapter_version,
            model,
            worktree_path,
            branch,
            base_sha
        ],
    )
    .map_err(db_err)?;
    tx.execute(
        "UPDATE tasks SET attempts = ?2 WHERE id = ?1",
        params![task_id, attempt],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;

    read_run(conn, &run_id)
}

/// 结束一次运行；`status` 仅接受 `completed` / `failed` / `cancelled`。
pub fn finish_run(
    conn: &Connection,
    run_id: &str,
    status: &str,
    error: Option<&str>,
) -> Result<RunRow, String> {
    if !matches!(status, "completed" | "failed" | "cancelled") {
        return Err(format!(
            "未知的运行状态：{status}（可选 completed / failed / cancelled）"
        ));
    }
    let changed = conn
        .execute(
            "UPDATE runs
             SET status = ?2,
                 ended_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                 error = ?3
             WHERE id = ?1",
            params![run_id, status, error],
        )
        .map_err(db_err)?;
    if changed == 0 {
        return Err(format!("运行不存在：{run_id}"));
    }
    read_run(conn, run_id)
}

/// 记录适配器会话 id（`runs.session_id`）。
pub fn set_run_session(conn: &Connection, run_id: &str, session_id: &str) -> Result<(), String> {
    let changed = conn
        .execute(
            "UPDATE runs SET session_id = ?2 WHERE id = ?1",
            params![run_id, session_id],
        )
        .map_err(db_err)?;
    if changed == 0 {
        return Err(format!("运行不存在：{run_id}"));
    }
    Ok(())
}

/// 记录运行关联的 GitHub PR（自动合并路径）。
pub fn set_run_pr(
    conn: &Connection,
    run_id: &str,
    number: u64,
    url: &str,
    state: &str,
) -> Result<(), String> {
    let changed = conn
        .execute(
            "UPDATE runs SET pr_number = ?2, pr_url = ?3, pr_state = ?4 WHERE id = ?1",
            params![run_id, number as i64, url, state],
        )
        .map_err(db_err)?;
    if changed == 0 {
        return Err(format!("运行不存在：{run_id}"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 证据 / 审查
// ---------------------------------------------------------------------------

pub fn add_evidence(conn: &Connection, input: NewEvidence) -> Result<EvidenceRow, String> {
    if !matches!(
        input.kind.as_str(),
        "test" | "build" | "lint" | "review" | "artifact"
    ) {
        return Err(format!("未知的证据类型：{}", input.kind));
    }
    ensure_run(conn, &input.run_id)?;

    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO evidence (id, run_id, criterion_id, kind, cmd, exit_code, duration_ms,
                               output_path)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id,
            input.run_id,
            input.criterion_id,
            input.kind,
            input.cmd,
            input.exit_code,
            input.duration_ms,
            input.output_path
        ],
    )
    .map_err(db_err)?;

    conn.query_row(
        "SELECT id, criterion_id, kind, cmd, exit_code, duration_ms, output_path, collected_at
         FROM evidence WHERE id = ?1",
        params![id],
        read_evidence_row,
    )
    .map_err(db_err)
}

/// 写入或覆盖 run 的审查结论（`reviews.run_id` 唯一）。
pub fn set_review(conn: &Connection, input: NewReview) -> Result<ReviewRow, String> {
    if !matches!(input.verdict.as_str(), "pass" | "fail" | "needs_human") {
        return Err(format!("未知的审查结论：{}", input.verdict));
    }
    ensure_run(conn, &input.run_id)?;

    let risk_flags_json =
        serde_json::to_string(&input.risk_flags).map_err(|e| format!("风险标记序列化失败：{e}"))?;
    let test_diff_json =
        serde_json::to_string(&input.test_diff).map_err(|e| format!("测试改动序列化失败：{e}"))?;

    conn.execute(
        "INSERT INTO reviews (id, run_id, verdict, summary, risk_flags_json, test_diff_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(run_id) DO UPDATE SET
             verdict = excluded.verdict,
             summary = excluded.summary,
             risk_flags_json = excluded.risk_flags_json,
             test_diff_json = excluded.test_diff_json,
             created_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        params![
            Uuid::new_v4().to_string(),
            input.run_id,
            input.verdict,
            input.summary,
            risk_flags_json,
            test_diff_json
        ],
    )
    .map_err(db_err)?;

    conn.query_row(
        "SELECT verdict, summary, risk_flags_json, test_diff_json
         FROM reviews WHERE run_id = ?1",
        params![input.run_id],
        read_review_row,
    )
    .map_err(db_err)
}

// ---------------------------------------------------------------------------
// 决策 / 事件
// ---------------------------------------------------------------------------

pub fn open_decision(conn: &Connection, input: NewDecision) -> Result<DecisionRow, String> {
    ensure_project(conn, &input.project_id)?;
    if let Some(task_id) = input.task_id.as_deref() {
        ensure_task(conn, task_id)?;
    }
    if let Some(run_id) = input.run_id.as_deref() {
        ensure_run(conn, run_id)?;
    }

    let id = Uuid::new_v4().to_string();
    let options_json =
        serde_json::to_string(&input.options).map_err(|e| format!("选项序列化失败：{e}"))?;

    conn.execute(
        "INSERT INTO decisions (id, project_id, task_id, run_id, kind, prompt, options_json,
                                status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'open')",
        params![
            id,
            input.project_id,
            input.task_id,
            input.run_id,
            input.kind,
            input.prompt,
            options_json
        ],
    )
    .map_err(db_err)?;

    read_decision(conn, &id)
}

/// 解决一条未决决策；已解决/已过期再调用返回 `Err`。
pub fn resolve_decision(
    conn: &Connection,
    decision_id: &str,
    resolution: &str,
) -> Result<DecisionRow, String> {
    let status: Option<String> = conn
        .query_row(
            "SELECT status FROM decisions WHERE id = ?1",
            params![decision_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    match status.as_deref() {
        None => return Err(format!("决策不存在：{decision_id}")),
        Some("open") => {}
        Some(other) => {
            return Err(format!("决策已处理（{other}），不能再解决：{decision_id}"));
        }
    }

    conn.execute(
        "UPDATE decisions
         SET status = 'resolved',
             resolution = ?2,
             resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        params![decision_id, resolution],
    )
    .map_err(db_err)?;

    read_decision(conn, decision_id)
}

/// 追加事件（只追加，不改写）；id 自动生成，payload 存为 JSON 文本。
pub fn append_event(conn: &Connection, input: NewEvent) -> Result<EventRow, String> {
    ensure_project(conn, &input.project_id)?;

    let id = Uuid::new_v4().to_string();
    let payload_json = input.payload.to_string();
    conn.execute(
        "INSERT INTO events (id, project_id, entity, entity_id, type, payload_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            id,
            input.project_id,
            input.entity,
            input.entity_id,
            input.kind,
            payload_json
        ],
    )
    .map_err(db_err)?;

    conn.query_row(
        "SELECT id, type, payload_json, at FROM events WHERE id = ?1",
        params![id],
        read_event_row,
    )
    .map_err(db_err)
}

// ---------------------------------------------------------------------------
// 证据门 / 崩溃恢复
// ---------------------------------------------------------------------------

/// 证据门（§4.2）：最新 run 的每条验收标准都要有最新 evidence 满足
/// `exit_code == criterion.expect_exit`，且（当 `expect_match` 存在时）其
/// `output_path` 文件内容包含该片段；文件缺失或不可读视为失败。
/// 此外该 run 的审查必须为 `pass`，且没有未决决策时，才允许
/// `verifying -> merged`。
pub fn merge_ready(conn: &Connection, task_id: &str) -> Result<bool, String> {
    let acceptance_json: String = conn
        .query_row(
            "SELECT acceptance_json FROM tasks WHERE id = ?1",
            params![task_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("任务不存在：{task_id}"))?;
    let criteria = parse_criteria(&acceptance_json)?;
    if criteria.is_empty() {
        return Ok(false);
    }

    let run_id: Option<String> = conn
        .query_row(
            "SELECT id FROM runs WHERE task_id = ?1 ORDER BY attempt DESC LIMIT 1",
            params![task_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    let Some(run_id) = run_id else {
        return Ok(false);
    };

    for criterion in &criteria {
        if !criterion_satisfied_by_latest_evidence(conn, &run_id, criterion)? {
            return Ok(false);
        }
    }

    let review_passed: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM reviews WHERE run_id = ?1 AND verdict = 'pass')",
            params![run_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if !review_passed {
        return Ok(false);
    }

    let open_decisions: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM decisions
             WHERE status = 'open'
               AND (task_id = ?1 OR run_id IN (SELECT id FROM runs WHERE task_id = ?1))",
            params![task_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;

    Ok(open_decisions == 0)
}

/// 判定某条验收标准是否被 run 内最新一条证据满足：退出码必须等于
/// `expect_exit`；存在 `expect_match` 时，证据输出文件必须可读且包含该片段。
fn criterion_satisfied_by_latest_evidence(
    conn: &Connection,
    run_id: &str,
    criterion: &Criterion,
) -> Result<bool, String> {
    let latest: Option<(Option<i32>, Option<String>)> = conn
        .query_row(
            "SELECT exit_code, output_path FROM evidence
             WHERE run_id = ?1 AND criterion_id = ?2
             ORDER BY collected_at DESC, rowid DESC
             LIMIT 1",
            params![run_id, criterion.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(db_err)?;

    let Some((exit_code, output_path)) = latest else {
        return Ok(false);
    };
    if exit_code != Some(criterion.expect_exit) {
        return Ok(false);
    }

    if let Some(fragment) = criterion.expect_match.as_deref() {
        let Some(path) = output_path else {
            return Ok(false);
        };
        let Ok(output) = std::fs::read_to_string(&path) else {
            return Ok(false);
        };
        if !output.contains(fragment) {
            return Ok(false);
        }
    }

    Ok(true)
}

/// 启动恢复（§4.8）：把残留的 `running` / `verifying` 任务标记为 `interrupted`，
/// 同时把其未结束的 run 收尾为 `cancelled`；返回被中断的任务数。
pub fn interrupt_running(conn: &Connection) -> Result<usize, String> {
    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let interrupted = tx
        .execute(
            "UPDATE items
             SET status = 'interrupted',
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE status IN ('running', 'verifying')
               AND id IN (SELECT item_id FROM tasks)",
            [],
        )
        .map_err(db_err)?;
    tx.execute(
        "UPDATE runs
         SET status = 'cancelled',
             ended_at = COALESCE(ended_at, strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             error = COALESCE(error, '应用重启，运行中断')
         WHERE status = 'running'
           AND task_id IN (
               SELECT t.id FROM tasks t JOIN items i ON i.id = t.item_id
               WHERE i.status = 'interrupted'
           )",
        [],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;
    Ok(interrupted)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Db, RegisterProjectInput};
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("temli-store-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn open_db() -> Db {
        Db::open(&temp_dir().join("temli.db")).unwrap()
    }

    fn project(db: &Db) -> String {
        let repo = temp_dir();
        db.insert_project(
            RegisterProjectInput {
                repo_path: repo.to_string_lossy().into_owned(),
                name: Some("demo".into()),
                github_owner: None,
                github_repo: None,
                default_branch: Some("main".into()),
            },
            repo.canonicalize().unwrap(),
        )
        .unwrap()
        .id
    }

    fn criteria() -> Vec<Criterion> {
        vec![
            Criterion {
                id: "c1".into(),
                cmd: "cargo test".into(),
                expect_exit: 0,
                expect_match: None,
            },
            Criterion {
                id: "c2".into(),
                cmd: "cargo clippy -- -D warnings".into(),
                expect_exit: 0,
                expect_match: Some("Finished".into()),
            },
        ]
    }

    fn new_task(project_id: &str, criteria: Vec<Criterion>) -> NewTask {
        NewTask {
            project_id: project_id.to_string(),
            title: "实现证据门".into(),
            prompt: "实现证据门并跑通验收".into(),
            criteria,
        }
    }

    fn new_run(task_id: &str) -> NewRun {
        NewRun {
            id: None,
            task_id: task_id.to_string(),
            adapter_id: "fake".into(),
            adapter_version: Some("0.1.0".into()),
            model: None,
            branch: Some("temli/1-run".into()),
            worktree_path: Some("/tmp/temli-wt".into()),
            base_sha: None,
        }
    }

    fn start_task_run(db: &Db, task_id: &str) -> RunRow {
        start_run(db.conn(), new_run(task_id)).unwrap()
    }

    fn ready_to_running(conn: &Connection, task_id: &str) {
        transition(conn, task_id, TaskStatus::Ready).unwrap();
        transition(conn, task_id, TaskStatus::Queued).unwrap();
        transition(conn, task_id, TaskStatus::Running).unwrap();
    }

    fn create_queued_task(conn: &Connection, project_id: &str, title: &str) -> TaskRow {
        let mut input = new_task(project_id, criteria());
        input.title = title.to_string();
        let task = create_task(conn, input).unwrap();
        transition(conn, &task.id, TaskStatus::Ready).unwrap();
        transition(conn, &task.id, TaskStatus::Queued).unwrap();
        task
    }

    fn insert_item(
        conn: &Connection,
        project_id: &str,
        kind: &str,
        title: &str,
        body: &str,
    ) -> String {
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO items (id, project_id, kind, title, body)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, project_id, kind, title, body],
        )
        .unwrap();
        id
    }

    fn merge_task(conn: &Connection, task_id: &str) {
        transition(conn, task_id, TaskStatus::Running).unwrap();
        transition(conn, task_id, TaskStatus::Verifying).unwrap();
        transition(conn, task_id, TaskStatus::Merged).unwrap();
    }

    fn set_item_created_at(conn: &Connection, task_id: &str, created_at: &str) {
        conn.execute(
            "UPDATE items SET created_at = ?2
             WHERE id = (SELECT item_id FROM tasks WHERE id = ?1)",
            rusqlite::params![task_id, created_at],
        )
        .unwrap();
    }

    fn write_evidence_output(contents: &str) -> String {
        let path = temp_dir().join("evidence.log");
        fs::write(&path, contents).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn evidence(run_id: &str, criterion_id: &str, exit_code: i32) -> NewEvidence {
        let output_path = write_evidence_output("Finished\n");
        evidence_with_output(run_id, criterion_id, exit_code, Some(output_path))
    }

    fn evidence_with_output(
        run_id: &str,
        criterion_id: &str,
        exit_code: i32,
        output_path: Option<String>,
    ) -> NewEvidence {
        NewEvidence {
            run_id: run_id.to_string(),
            criterion_id: Some(criterion_id.to_string()),
            kind: "test".into(),
            cmd: Some("cargo test".into()),
            exit_code: Some(exit_code),
            duration_ms: Some(42),
            output_path,
        }
    }

    /// 不挂验收标准的辅助证据：artifact / review 两类，退出码为 0 也不该计入
    /// 验收证据的通过数。
    fn auxiliary_evidence(run_id: &str, kind: &str) -> NewEvidence {
        NewEvidence {
            run_id: run_id.to_string(),
            criterion_id: None,
            kind: kind.into(),
            cmd: None,
            exit_code: Some(0),
            duration_ms: Some(7),
            output_path: None,
        }
    }

    #[test]
    fn create_task_inserts_issue_item_and_criteria() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        assert_eq!(task.status, TaskStatus::Draft);
        assert_eq!(task.attempts, 0);
        assert_eq!(task.max_minutes, 30);
        assert_eq!(task.project_id, project_id);
        assert_eq!(task.prompt, "实现证据门并跑通验收");

        let detail = get_task(conn, &task.id).unwrap();
        assert_eq!(detail.criteria.len(), 2);
        assert_eq!(detail.criteria[0].id, "c1");
        assert_eq!(detail.criteria[1].expect_match.as_deref(), Some("Finished"));
        assert!(detail.runs.is_empty());
        assert!(detail.evidence.is_empty());
        assert!(detail.review.is_none());
        assert!(detail.decisions.is_empty());
        assert!(detail.events.is_empty());

        let summaries = list_tasks(conn).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, task.id);
        assert_eq!(summaries[0].criterion_count, 2);
        assert_eq!(list_tasks_for_project(conn, &project_id).unwrap().len(), 1);
        assert!(
            list_tasks_for_project(conn, "missing-project")
                .unwrap()
                .is_empty()
        );

        let kind: String = conn
            .query_row(
                "SELECT kind FROM items WHERE id = (SELECT item_id FROM tasks WHERE id = ?1)",
                [task.id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(kind, "issue");
    }

    #[test]
    fn create_task_writes_no_partial_rows_for_missing_project() {
        let db = open_db();
        let conn = db.conn();

        let err = create_task(conn, new_task("missing-project", criteria())).unwrap_err();
        assert!(err.contains("项目不存在"), "{err}");

        let items: i64 = conn
            .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
            .unwrap();
        let tasks: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
            .unwrap();
        assert_eq!((items, tasks), (0, 0));
    }

    #[test]
    fn create_task_for_item_reuses_issue_row() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();
        let item_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO items (id, project_id, kind, title, body, status, github_number)
             VALUES (?1, ?2, 'issue', '同步问题', '复现步骤', 'open', 42)",
            params![item_id, project_id],
        )
        .unwrap();

        let task = create_task_for_item(conn, &item_id, "处理同步问题", criteria()).unwrap();
        assert_eq!(task.status, TaskStatus::Draft);
        assert_eq!(task.title, "同步问题");
        assert_eq!(task.project_id, project_id);
        assert_eq!(task.prompt, "处理同步问题");
        assert_eq!(get_task(conn, &task.id).unwrap().criteria.len(), 2);

        let (status, body, github_number): (String, String, i64) = conn
            .query_row(
                "SELECT status, body, github_number FROM items WHERE id = ?1",
                params![item_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "draft");
        assert_eq!(body, "复现步骤");
        assert_eq!(github_number, 42);

        // 条目已有关联任务时必须报错，而不是再插入一条 tasks 行。
        let err = create_task_for_item(conn, &item_id, "再次转换", criteria()).unwrap_err();
        assert!(err.contains("已有任务"), "{err}");
        let tasks: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tasks, 1);
    }

    #[test]
    fn auto_docs_settings_default_off_and_roundtrip() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        assert!(!project_auto_docs(conn, &project_id).unwrap());
        set_project_auto_docs(conn, &project_id, true).unwrap();
        assert!(project_auto_docs(conn, &project_id).unwrap());
        set_project_auto_docs(conn, &project_id, false).unwrap();
        assert!(!project_auto_docs(conn, &project_id).unwrap());

        assert!(project_auto_docs(conn, "missing-project").is_err());
        assert!(set_project_auto_docs(conn, "missing-project", true).is_err());

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        assert!(!task_auto_docs(conn, &task.id).unwrap());
        assert!(task_auto_docs(conn, "missing-task").is_err());
    }

    #[test]
    fn create_docs_task_queues_linked_auto_docs_task() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();
        let parent = create_task(conn, new_task(&project_id, criteria())).unwrap();

        let docs = create_docs_task(
            conn,
            &project_id,
            &parent.id,
            "文档：实现证据门",
            "更新文档",
            criteria(),
        )
        .unwrap();

        assert_eq!(docs.task.status, TaskStatus::Queued);
        assert_eq!(docs.task.project_id, project_id);
        assert_eq!(docs.task.title, "文档：实现证据门");
        assert_eq!(docs.task.prompt, "更新文档");
        assert_eq!(docs.criteria.len(), 2);
        assert!(task_auto_docs(conn, &docs.task.id).unwrap());

        let (kind, status): (String, String) = conn
            .query_row(
                "SELECT kind, status FROM items
                 WHERE id = (SELECT item_id FROM tasks WHERE id = ?1)",
                params![docs.task.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((kind.as_str(), status.as_str()), ("issue", "queued"));

        let linked: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM links l
                     JOIN tasks parent ON parent.item_id = l.from_item
                     JOIN tasks docs ON docs.item_id = l.to_item
                     WHERE parent.id = ?1 AND docs.id = ?2 AND l.rel = 'derived_from'
                 )",
                params![parent.id, docs.task.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            linked,
            "父任务 item 到文档任务 item 必须有 derived_from 链接"
        );
        assert!(docs.events.is_empty());
    }

    #[test]
    fn list_tasks_carries_auto_docs_flag() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();
        let parent = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let docs = create_docs_task(
            conn,
            &project_id,
            &parent.id,
            "文档：实现证据门",
            "更新文档",
            criteria(),
        )
        .unwrap();

        let summaries = list_tasks_for_project(conn, &project_id).unwrap();
        assert_eq!(summaries.len(), 2);
        let parent_summary = summaries
            .iter()
            .find(|summary| summary.id == parent.id)
            .unwrap();
        assert!(!parent_summary.auto_docs);
        let docs_summary = summaries
            .iter()
            .find(|summary| summary.id == docs.task.id)
            .unwrap();
        assert!(docs_summary.auto_docs);

        // 依赖视图与列表共用行读取器，同样必须读到 auto_docs 列。
        add_dependency(conn, &docs.task.id, &parent.id).unwrap();
        let (blockers, _) = task_dependencies(conn, &docs.task.id).unwrap();
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].id, parent.id);
        assert!(!blockers[0].auto_docs);
        let (_, blocks) = task_dependencies(conn, &parent.id).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].id, docs.task.id);
        assert!(blocks[0].auto_docs);
    }

    #[test]
    fn create_docs_task_rejects_missing_inputs_and_empty_criteria() {
        let db = open_db();
        let project_id = project(&db);
        let other_project = project(&db);
        let conn = db.conn();
        let parent = create_task(conn, new_task(&project_id, criteria())).unwrap();

        let missing_project =
            create_docs_task(conn, "missing-project", &parent.id, "文档", "p", criteria())
                .unwrap_err();
        assert!(missing_project.contains("项目不存在"), "{missing_project}");

        let missing_parent =
            create_docs_task(conn, &project_id, "missing-task", "文档", "p", criteria())
                .unwrap_err();
        assert!(missing_parent.contains("任务不存在"), "{missing_parent}");

        let mismatched =
            create_docs_task(conn, &other_project, &parent.id, "文档", "p", criteria())
                .unwrap_err();
        assert!(mismatched.contains("父任务不属于该项目"), "{mismatched}");

        let empty =
            create_docs_task(conn, &project_id, &parent.id, "文档", "p", Vec::new()).unwrap_err();
        assert!(empty.contains("验收标准"), "{empty}");

        let mut invalid = criteria();
        invalid[0].id = "bad id".into();
        let invalid =
            create_docs_task(conn, &project_id, &parent.id, "文档", "p", invalid).unwrap_err();
        assert!(invalid.contains("id 非法"), "{invalid}");

        let (items, tasks, links): (i64, i64, i64) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM items),
                        (SELECT COUNT(*) FROM tasks),
                        (SELECT COUNT(*) FROM links)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((items, tasks, links), (1, 1, 0), "失败时不得写入任何行");
    }

    #[test]
    fn create_task_for_item_rejects_non_issue_and_invalid_criteria() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let note = insert_item(conn, &project_id, "note", "笔记", "正文");
        let err = create_task_for_item(conn, &note, "p", criteria()).unwrap_err();
        assert!(err.contains("issue"), "{err}");

        let issue = insert_item(conn, &project_id, "issue", "问题", "正文");
        let mut invalid = criteria();
        invalid[0].id = "bad id".into();
        let err = create_task_for_item(conn, &issue, "p", invalid).unwrap_err();
        assert!(err.contains("id 非法"), "{err}");

        let missing = create_task_for_item(conn, "missing-item", "p", criteria()).unwrap_err();
        assert!(missing.contains("条目不存在"), "{missing}");

        let (tasks, status): (i64, String) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM tasks), status FROM items WHERE id = ?1",
                params![issue],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(tasks, 0);
        assert_eq!(status, "open", "校验失败时不应改写 item 状态");
    }

    #[test]
    fn queued_requires_acceptance_criteria() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let empty = create_task(conn, new_task(&project_id, vec![])).unwrap();
        // draft -> queued 本身非法
        let err = transition(conn, &empty.id, TaskStatus::Queued).unwrap_err();
        assert!(err.contains("非法状态迁移"), "{err}");

        transition(conn, &empty.id, TaskStatus::Ready).unwrap();
        let err = transition(conn, &empty.id, TaskStatus::Queued).unwrap_err();
        assert!(err.contains("验收标准"), "{err}");
        assert_eq!(
            get_task(conn, &empty.id).unwrap().task.status,
            TaskStatus::Ready
        );

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        transition(conn, &task.id, TaskStatus::Ready).unwrap();
        assert_eq!(
            transition(conn, &task.id, TaskStatus::Queued).unwrap(),
            TaskStatus::Queued
        );
    }

    #[test]
    fn queued_can_cancel_to_draft_but_merged_stays_terminal() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let id = task.id.as_str();
        transition(conn, id, TaskStatus::Ready).unwrap();
        transition(conn, id, TaskStatus::Queued).unwrap();
        assert_eq!(
            transition(conn, id, TaskStatus::Draft).unwrap(),
            TaskStatus::Draft
        );
        assert_eq!(get_task(conn, id).unwrap().task.status, TaskStatus::Draft);
        assert_eq!(next_queued_task(conn).unwrap(), None);

        // 取消后可重新入队并走到终态
        ready_to_running(conn, id);
        transition(conn, id, TaskStatus::Verifying).unwrap();
        transition(conn, id, TaskStatus::Merged).unwrap();
        let err = transition(conn, id, TaskStatus::Queued).unwrap_err();
        assert!(err.contains("非法状态迁移"), "{err}");
        assert!(transition(conn, id, TaskStatus::Draft).is_err());
    }

    #[test]
    fn transition_edges_match_documented_state_machine() {
        use TaskStatus::*;
        const ALL: [TaskStatus; 9] = [
            Draft,
            Ready,
            Queued,
            Running,
            Verifying,
            Merged,
            Failed,
            NeedsHuman,
            Interrupted,
        ];
        let allowed = [
            (Draft, Ready),
            (Ready, Queued),
            (Queued, Running),
            (Queued, Draft),
            (Running, Verifying),
            (Running, Failed),
            (Running, Interrupted),
            (Verifying, Merged),
            (Verifying, NeedsHuman),
            (Verifying, Failed),
            (Verifying, Interrupted),
            (Failed, Queued),
            (Failed, NeedsHuman),
            (Interrupted, Queued),
            (Interrupted, Draft),
            (NeedsHuman, Queued),
            (NeedsHuman, Draft),
        ];
        for from in ALL {
            for to in ALL {
                assert_eq!(
                    allowed_transition(from, to),
                    allowed.contains(&(from, to)),
                    "{from} -> {to}"
                );
            }
        }
        // merged 是终态：没有任何出边
        assert!(ALL.iter().all(|to| !allowed_transition(Merged, *to)));
    }

    #[test]
    fn next_queued_task_returns_oldest_queued_task() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        assert_eq!(next_queued_task(conn).unwrap(), None);

        let older = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let newer = create_task(conn, new_task(&project_id, criteria())).unwrap();

        // draft 不参与调度
        assert_eq!(next_queued_task(conn).unwrap(), None);

        for id in [&older.id, &newer.id] {
            transition(conn, id, TaskStatus::Ready).unwrap();
            transition(conn, id, TaskStatus::Queued).unwrap();
        }
        // 固定创建时间，避免毫秒级抖动影响断言
        conn.execute(
            "UPDATE items SET created_at = '2026-01-02T00:00:00.000Z'
             WHERE id = (SELECT item_id FROM tasks WHERE id = ?1)",
            [newer.id.as_str()],
        )
        .unwrap();
        conn.execute(
            "UPDATE items SET created_at = '2026-01-01T00:00:00.000Z'
             WHERE id = (SELECT item_id FROM tasks WHERE id = ?1)",
            [older.id.as_str()],
        )
        .unwrap();

        assert_eq!(
            next_queued_task(conn).unwrap().as_deref(),
            Some(older.id.as_str())
        );
        assert_eq!(task_status(conn, &older.id).unwrap(), TaskStatus::Queued);
        assert_eq!(task_status(conn, &newer.id).unwrap(), TaskStatus::Queued);
        assert!(task_status(conn, "missing-task").is_err());

        // 同一时刻按任务 id 升序兜底
        conn.execute(
            "UPDATE items SET created_at = '2026-01-01T00:00:00.000Z'",
            [],
        )
        .unwrap();
        let (first, second) = if older.id < newer.id {
            (&older, &newer)
        } else {
            (&newer, &older)
        };
        assert_eq!(
            next_queued_task(conn).unwrap().as_deref(),
            Some(first.id.as_str())
        );

        // 取消首个任务后调度下一个
        transition(conn, &first.id, TaskStatus::Draft).unwrap();
        assert_eq!(
            next_queued_task(conn).unwrap().as_deref(),
            Some(second.id.as_str())
        );
    }

    #[test]
    fn valid_transitions_pass_and_merged_is_terminal() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let id = task.id.as_str();
        assert_eq!(
            transition(conn, id, TaskStatus::Ready).unwrap(),
            TaskStatus::Ready
        );
        assert_eq!(
            transition(conn, id, TaskStatus::Queued).unwrap(),
            TaskStatus::Queued
        );
        assert_eq!(
            transition(conn, id, TaskStatus::Running).unwrap(),
            TaskStatus::Running
        );
        assert_eq!(
            transition(conn, id, TaskStatus::Verifying).unwrap(),
            TaskStatus::Verifying
        );
        assert_eq!(
            transition(conn, id, TaskStatus::Merged).unwrap(),
            TaskStatus::Merged
        );

        let err = transition(conn, id, TaskStatus::Queued).unwrap_err();
        assert!(err.contains("非法状态迁移"), "{err}");
        assert!(transition(conn, id, TaskStatus::Draft).is_err());
        assert_eq!(get_task(conn, id).unwrap().task.status, TaskStatus::Merged);
    }

    #[test]
    fn needs_human_and_interrupted_only_leave_for_queued_or_draft() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let id = task.id.as_str();
        ready_to_running(conn, id);
        transition(conn, id, TaskStatus::Interrupted).unwrap();
        assert!(transition(conn, id, TaskStatus::Running).is_err());
        assert_eq!(
            transition(conn, id, TaskStatus::Draft).unwrap(),
            TaskStatus::Draft
        );

        ready_to_running(conn, id);
        transition(conn, id, TaskStatus::Failed).unwrap();
        transition(conn, id, TaskStatus::NeedsHuman).unwrap();
        assert!(transition(conn, id, TaskStatus::Failed).is_err());
        assert_eq!(
            transition(conn, id, TaskStatus::Queued).unwrap(),
            TaskStatus::Queued
        );
    }

    #[test]
    fn failed_retry_is_limited_to_three_attempts() {
        assert_eq!(MAX_ATTEMPTS, 3);
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let id = task.id.as_str();

        // 第一次尝试后可重试
        transition(conn, id, TaskStatus::Ready).unwrap();
        transition(conn, id, TaskStatus::Queued).unwrap();
        transition(conn, id, TaskStatus::Running).unwrap();
        start_task_run(&db, id);
        transition(conn, id, TaskStatus::Failed).unwrap();
        assert_eq!(
            transition(conn, id, TaskStatus::Queued).unwrap(),
            TaskStatus::Queued
        );

        // 用完三次尝试后禁止自动重试
        start_task_run(&db, id);
        start_task_run(&db, id);
        assert_eq!(get_task(conn, id).unwrap().task.attempts, MAX_ATTEMPTS);
        transition(conn, id, TaskStatus::Running).unwrap();
        transition(conn, id, TaskStatus::Failed).unwrap();
        let err = transition(conn, id, TaskStatus::Queued).unwrap_err();
        assert!(err.contains("尝试次数"), "{err}");
        assert_eq!(
            transition(conn, id, TaskStatus::NeedsHuman).unwrap(),
            TaskStatus::NeedsHuman
        );
    }

    #[test]
    fn merge_ready_requires_green_evidence_pass_review_and_no_open_decision() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let id = task.id.as_str();
        ready_to_running(conn, id);
        transition(conn, id, TaskStatus::Verifying).unwrap();
        let run = start_task_run(&db, id);
        assert_eq!(run.status, "running");

        assert!(!merge_ready(conn, id).unwrap());

        let first = add_evidence(conn, evidence(&run.id, "c1", 0)).unwrap();
        assert_eq!(first.criterion_id.as_deref(), Some("c1"));
        assert_eq!(first.exit_code, Some(0));
        assert_eq!(first.duration_ms, Some(42));
        assert!(!merge_ready(conn, id).unwrap());

        add_evidence(conn, evidence(&run.id, "c2", 1)).unwrap();
        assert!(!merge_ready(conn, id).unwrap());

        add_evidence(conn, evidence(&run.id, "c2", 0)).unwrap();
        // 证据全绿，但缺审查
        assert!(!merge_ready(conn, id).unwrap());

        let review = set_review(
            conn,
            NewReview {
                run_id: run.id.clone(),
                verdict: "pass".into(),
                summary: Some("证据齐全".into()),
                risk_flags: vec!["依赖清单".into()],
                test_diff: vec!["tests/gate.rs +12/-3".into()],
            },
        )
        .unwrap();
        assert_eq!(review.verdict, "pass");
        assert_eq!(review.risk_flags, vec!["依赖清单".to_string()]);
        assert_eq!(review.test_diff, vec!["tests/gate.rs +12/-3".to_string()]);
        assert!(merge_ready(conn, id).unwrap());

        // 审查结论可覆盖
        let failed_review = set_review(
            conn,
            NewReview {
                run_id: run.id.clone(),
                verdict: "fail".into(),
                summary: Some("存在回归".into()),
                risk_flags: vec![],
                test_diff: vec![],
            },
        )
        .unwrap();
        assert_eq!(failed_review.verdict, "fail");
        assert!(!merge_ready(conn, id).unwrap());
        assert!(
            set_review(
                conn,
                NewReview {
                    run_id: run.id.clone(),
                    verdict: "maybe".into(),
                    summary: None,
                    risk_flags: vec![],
                    test_diff: vec![],
                },
            )
            .is_err()
        );

        set_review(
            conn,
            NewReview {
                run_id: run.id.clone(),
                verdict: "pass".into(),
                summary: Some("证据齐全".into()),
                risk_flags: vec![],
                test_diff: vec!["tests/gate.rs +12/-3".into()],
            },
        )
        .unwrap();
        assert!(merge_ready(conn, id).unwrap());

        // 未决决策阻塞合并
        let decision = open_decision(
            conn,
            NewDecision {
                project_id: project_id.clone(),
                task_id: Some(task.id.clone()),
                run_id: Some(run.id.clone()),
                kind: "approval".into(),
                prompt: "是否允许删除文件？".into(),
                options: vec!["allow".into(), "deny".into()],
            },
        )
        .unwrap();
        assert_eq!(decision.status, "open");
        assert!(decision.resolution.is_none());
        assert!(!merge_ready(conn, id).unwrap());

        let resolved = resolve_decision(conn, &decision.id, "allow").unwrap();
        assert_eq!(resolved.status, "resolved");
        assert_eq!(resolved.resolution.as_deref(), Some("allow"));
        assert!(merge_ready(conn, id).unwrap());
        assert!(resolve_decision(conn, &decision.id, "deny").is_err());

        let detail = get_task(conn, id).unwrap();
        assert_eq!(detail.runs.len(), 1);
        assert_eq!(detail.evidence.len(), 3);
        assert_eq!(detail.decisions.len(), 1);
        assert_eq!(detail.decisions[0].status, "resolved");
        let stored_review = detail.review.expect("review");
        assert_eq!(stored_review.verdict, "pass");
        assert_eq!(
            stored_review.test_diff,
            vec!["tests/gate.rs +12/-3".to_string()]
        );
    }

    #[test]
    fn merge_ready_honors_expected_exit_code() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(
            conn,
            new_task(
                &project_id,
                vec![Criterion {
                    id: "c-1".into(),
                    cmd: "sh -c 'exit 1'".into(),
                    expect_exit: 1,
                    expect_match: None,
                }],
            ),
        )
        .unwrap();
        let id = task.id.as_str();
        ready_to_running(conn, id);
        transition(conn, id, TaskStatus::Verifying).unwrap();
        let run = start_task_run(&db, id);
        set_review(
            conn,
            NewReview {
                run_id: run.id.clone(),
                verdict: "pass".into(),
                summary: None,
                risk_flags: vec![],
                test_diff: vec![],
            },
        )
        .unwrap();

        add_evidence(conn, evidence(&run.id, "c-1", 0)).unwrap();
        assert!(
            !merge_ready(conn, id).unwrap(),
            "exit 0 不满足 expect_exit 1"
        );

        add_evidence(conn, evidence(&run.id, "c-1", 1)).unwrap();
        assert!(
            merge_ready(conn, id).unwrap(),
            "exit 1 应满足 expect_exit 1"
        );
    }

    #[test]
    fn merge_ready_honors_expect_match_against_output_file() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(
            conn,
            new_task(
                &project_id,
                vec![Criterion {
                    id: "c-1".into(),
                    cmd: "echo needle".into(),
                    expect_exit: 0,
                    expect_match: Some("needle".into()),
                }],
            ),
        )
        .unwrap();
        let id = task.id.as_str();
        ready_to_running(conn, id);
        transition(conn, id, TaskStatus::Verifying).unwrap();
        let run = start_task_run(&db, id);
        set_review(
            conn,
            NewReview {
                run_id: run.id.clone(),
                verdict: "pass".into(),
                summary: None,
                risk_flags: vec![],
                test_diff: vec![],
            },
        )
        .unwrap();

        let hit = write_evidence_output("--- stdout ---\nneedle\n");
        add_evidence(conn, evidence_with_output(&run.id, "c-1", 0, Some(hit))).unwrap();
        assert!(merge_ready(conn, id).unwrap(), "输出包含片段应通过");

        let miss = write_evidence_output("--- stdout ---\nhaystack\n");
        add_evidence(conn, evidence_with_output(&run.id, "c-1", 0, Some(miss))).unwrap();
        assert!(!merge_ready(conn, id).unwrap(), "最新证据不含片段应失败");

        let missing = temp_dir().join("missing.log");
        add_evidence(
            conn,
            evidence_with_output(
                &run.id,
                "c-1",
                0,
                Some(missing.to_string_lossy().into_owned()),
            ),
        )
        .unwrap();
        assert!(!merge_ready(conn, id).unwrap(), "输出文件缺失应失败");

        add_evidence(conn, evidence_with_output(&run.id, "c-1", 0, None)).unwrap();
        assert!(!merge_ready(conn, id).unwrap(), "缺少 output_path 应失败");

        let hit_again = write_evidence_output("stdout: needle\n");
        add_evidence(
            conn,
            evidence_with_output(&run.id, "c-1", 0, Some(hit_again)),
        )
        .unwrap();
        assert!(merge_ready(conn, id).unwrap(), "最新证据重新命中应通过");
    }

    #[test]
    fn validate_criteria_rejects_invalid_ids_and_empty_cmd() {
        let valid = Criterion {
            id: "C_2-ok".into(),
            cmd: "cargo test".into(),
            expect_exit: 0,
            expect_match: None,
        };
        assert!(validate_criteria(std::slice::from_ref(&valid)).is_ok());

        let empty_cmd = Criterion {
            cmd: "   ".into(),
            ..valid.clone()
        };
        let err = validate_criteria(&[empty_cmd]).unwrap_err();
        assert!(err.contains("cmd"), "{err}");

        let empty_id = Criterion {
            id: String::new(),
            ..valid.clone()
        };
        assert!(validate_criteria(&[empty_id]).is_err());

        let spaced_id = Criterion {
            id: "bad id".into(),
            ..valid.clone()
        };
        assert!(validate_criteria(&[spaced_id]).is_err());

        let long_id = Criterion {
            id: "a".repeat(41),
            ..valid.clone()
        };
        assert!(validate_criteria(&[long_id]).is_err());

        let duplicate = Criterion {
            id: "dup".into(),
            ..valid.clone()
        };
        let err = validate_criteria(&[duplicate.clone(), duplicate]).unwrap_err();
        assert!(err.contains("重复"), "{err}");
    }

    #[test]
    fn merge_ready_is_false_for_unknown_task() {
        let db = open_db();
        let conn = db.conn();
        let err = merge_ready(conn, "missing-task").unwrap_err();
        assert!(err.contains("任务不存在"), "{err}");
    }

    #[test]
    fn interrupt_running_marks_tasks_and_closes_runs() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        ready_to_running(conn, &task.id);
        let run = start_task_run(&db, &task.id);
        assert_eq!(run.status, "running");

        assert_eq!(interrupt_running(conn).unwrap(), 1);

        let detail = get_task(conn, &task.id).unwrap();
        assert_eq!(detail.task.status, TaskStatus::Interrupted);
        assert_eq!(detail.runs.len(), 1);
        assert_eq!(detail.runs[0].status, "cancelled");
        assert!(detail.runs[0].ended_at.is_some());
        assert_eq!(detail.runs[0].error.as_deref(), Some("应用重启，运行中断"));

        // 已无运行中的任务
        assert_eq!(interrupt_running(conn).unwrap(), 0);
    }

    #[test]
    fn start_run_uses_caller_supplied_id() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();
        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();

        let mut spec = new_run(&task.id);
        spec.id = Some("run-fixed".into());
        let run = start_run(conn, spec).unwrap();
        assert_eq!(run.id, "run-fixed");

        let mut duplicate = new_run(&task.id);
        duplicate.id = Some("run-fixed".into());
        let err = start_run(conn, duplicate).unwrap_err();
        assert!(err.contains("已存在"), "{err}");
    }

    #[test]
    fn finish_run_and_append_event() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let run = start_task_run(&db, &task.id);
        assert_eq!(run.status, "running");
        assert_eq!(get_task(conn, &task.id).unwrap().task.attempts, 1);

        set_run_session(conn, &run.id, "session-1").unwrap();
        let finished = finish_run(conn, &run.id, "completed", None).unwrap();
        assert_eq!(finished.status, "completed");
        assert!(finished.ended_at.is_some());
        assert!(finished.error.is_none());
        assert!(finish_run(conn, &run.id, "bogus", None).is_err());
        assert!(finish_run(conn, "missing-run", "failed", Some("x")).is_err());

        let event = append_event(
            conn,
            NewEvent {
                project_id: project_id.clone(),
                entity: "task".into(),
                entity_id: task.id.clone(),
                kind: "task.created".into(),
                payload: serde_json::json!({ "attempt": 1 }),
            },
        )
        .unwrap();
        assert!(!event.id.is_empty());
        assert_eq!(event.kind, "task.created");
        assert_eq!(event.payload_json, "{\"attempt\":1}");

        let session_id: Option<String> = conn
            .query_row(
                "SELECT session_id FROM runs WHERE id = ?1",
                [run.id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(session_id.as_deref(), Some("session-1"));

        let detail = get_task(conn, &task.id).unwrap();
        assert_eq!(detail.events.len(), 1);
        assert_eq!(detail.events[0].payload_json, "{\"attempt\":1}");
    }

    #[test]
    fn dependency_rejects_self_and_cross_project_edges() {
        let db = open_db();
        let project_id = project(&db);
        let other_project_id = project(&db);
        let conn = db.conn();

        let task = create_queued_task(conn, &project_id, "任务");
        let foreign = create_queued_task(conn, &other_project_id, "外部任务");

        let err = add_dependency(conn, &task.id, &task.id).unwrap_err();
        assert!(err.contains("自身"), "{err}");

        let err = add_dependency(conn, &task.id, &foreign.id).unwrap_err();
        assert!(err.contains("同一项目"), "{err}");

        assert!(add_dependency(conn, &task.id, "missing-task").is_err());
        assert!(add_dependency(conn, "missing-task", &task.id).is_err());
        assert!(remove_dependency(conn, &task.id, "missing-task").is_err());

        // 被拒绝的写入不能留下任何依赖边
        let edge_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM links WHERE rel = 'blocks'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(edge_count, 0);
    }

    #[test]
    fn dependency_cycle_is_rejected_and_duplicate_add_is_noop() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let a = create_queued_task(conn, &project_id, "A");
        let b = create_queued_task(conn, &project_id, "B");

        // A 依赖 B；重复登记同一条边是幂等的。
        add_dependency(conn, &a.id, &b.id).unwrap();
        add_dependency(conn, &a.id, &b.id).unwrap();

        let (blockers, blocks) = task_dependencies(conn, &a.id).unwrap();
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].id, b.id);
        assert_eq!(blockers[0].status, TaskStatus::Queued);
        assert!(blocks.is_empty());

        let (b_blockers, b_blocks) = task_dependencies(conn, &b.id).unwrap();
        assert!(b_blockers.is_empty());
        assert_eq!(b_blocks.len(), 1);
        assert_eq!(b_blocks[0].id, a.id);

        // B 依赖 A 会与已有边形成环。
        let err = add_dependency(conn, &b.id, &a.id).unwrap_err();
        assert!(err.contains("环"), "{err}");

        let edge_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM links WHERE rel = 'blocks'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(edge_count, 1);
    }

    #[test]
    fn next_ready_task_waits_for_dependencies_to_merge() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let dependent = create_queued_task(conn, &project_id, "被阻塞");
        let blocker = create_queued_task(conn, &project_id, "前置");
        add_dependency(conn, &dependent.id, &blocker.id).unwrap();
        // dependent 更旧，但只有 blocker 完成后才可调度。
        set_item_created_at(conn, &dependent.id, "2026-01-01T00:00:00.000Z");
        set_item_created_at(conn, &blocker.id, "2026-01-02T00:00:00.000Z");

        assert_eq!(
            next_ready_task(conn, &project_id).unwrap().as_deref(),
            Some(blocker.id.as_str())
        );

        transition(conn, &blocker.id, TaskStatus::Running).unwrap();
        assert_eq!(next_ready_task(conn, &project_id).unwrap(), None);

        transition(conn, &blocker.id, TaskStatus::Failed).unwrap();
        assert_eq!(next_ready_task(conn, &project_id).unwrap(), None);

        transition(conn, &blocker.id, TaskStatus::Queued).unwrap();
        assert_eq!(
            next_ready_task(conn, &project_id).unwrap().as_deref(),
            Some(blocker.id.as_str())
        );

        transition(conn, &blocker.id, TaskStatus::Running).unwrap();
        transition(conn, &blocker.id, TaskStatus::Verifying).unwrap();
        assert_eq!(next_ready_task(conn, &project_id).unwrap(), None);

        transition(conn, &blocker.id, TaskStatus::Merged).unwrap();
        assert_eq!(
            next_ready_task(conn, &project_id).unwrap().as_deref(),
            Some(dependent.id.as_str())
        );
    }

    #[test]
    fn next_ready_task_waits_for_transitive_chain() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        // C 依赖 B，B 依赖 A：C 必须等 A 和 B 都 merged。
        let c = create_queued_task(conn, &project_id, "C");
        let b = create_queued_task(conn, &project_id, "B");
        let a = create_queued_task(conn, &project_id, "A");
        add_dependency(conn, &c.id, &b.id).unwrap();
        add_dependency(conn, &b.id, &a.id).unwrap();
        set_item_created_at(conn, &c.id, "2026-01-01T00:00:00.000Z");
        set_item_created_at(conn, &b.id, "2026-01-02T00:00:00.000Z");
        set_item_created_at(conn, &a.id, "2026-01-03T00:00:00.000Z");

        assert_eq!(
            next_ready_task(conn, &project_id).unwrap().as_deref(),
            Some(a.id.as_str())
        );

        merge_task(conn, &a.id);
        assert_eq!(
            next_ready_task(conn, &project_id).unwrap().as_deref(),
            Some(b.id.as_str()),
            "C 仍被 B 传递阻塞"
        );

        merge_task(conn, &b.id);
        assert_eq!(
            next_ready_task(conn, &project_id).unwrap().as_deref(),
            Some(c.id.as_str())
        );
    }

    #[test]
    fn task_ready_waits_for_transitive_blockers_to_merge() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let a = create_queued_task(conn, &project_id, "A");
        let b = create_queued_task(conn, &project_id, "B");
        let c = create_queued_task(conn, &project_id, "C");
        add_dependency(conn, &b.id, &a.id).unwrap();
        add_dependency(conn, &c.id, &b.id).unwrap();

        assert!(task_ready(conn, &a.id).unwrap());
        assert!(!task_ready(conn, &b.id).unwrap());
        assert!(!task_ready(conn, &c.id).unwrap(), "C 被 B 传递阻塞");

        merge_task(conn, &a.id);
        assert!(task_ready(conn, &b.id).unwrap());
        assert!(!task_ready(conn, &c.id).unwrap(), "B 尚未合并");

        merge_task(conn, &b.id);
        assert!(task_ready(conn, &c.id).unwrap());

        // 非 queued 状态即使没有依赖也不就绪；不存在的任务同样返回 false。
        let draft = create_task(conn, new_task(&project_id, criteria())).unwrap();
        assert!(!task_ready(conn, &draft.id).unwrap());
        assert!(!task_ready(conn, "missing-task").unwrap());
    }

    #[test]
    fn next_ready_tasks_lists_all_ready_tasks_in_order() {
        let db = open_db();
        let project_id = project(&db);
        let other_project_id = project(&db);
        let conn = db.conn();

        let blocked = create_queued_task(conn, &project_id, "被阻塞");
        let second = create_queued_task(conn, &project_id, "第二");
        let first = create_queued_task(conn, &project_id, "第一");
        create_queued_task(conn, &other_project_id, "外部任务");
        add_dependency(conn, &blocked.id, &second.id).unwrap();
        set_item_created_at(conn, &blocked.id, "2026-01-01T00:00:00.000Z");
        set_item_created_at(conn, &second.id, "2026-01-02T00:00:00.000Z");
        set_item_created_at(conn, &first.id, "2026-01-03T00:00:00.000Z");

        // blocked 最旧但依赖 second，因此只返回两个独立任务，按创建时间排序；
        // 其它项目的任务不参与本项目的就绪列表。
        assert_eq!(
            next_ready_tasks(conn, &project_id).unwrap(),
            vec![second.id.clone(), first.id.clone()]
        );
        assert_eq!(
            next_ready_task(conn, &project_id).unwrap().as_deref(),
            Some(second.id.as_str()),
            "队首与 next_ready_task 保持一致"
        );

        merge_task(conn, &second.id);
        assert_eq!(
            next_ready_tasks(conn, &project_id).unwrap(),
            vec![blocked.id.clone(), first.id.clone()],
            "依赖合并后 blocked 按创建时间回到队首"
        );
    }

    #[test]
    fn remove_dependency_updates_readiness_immediately() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let dependent = create_queued_task(conn, &project_id, "被阻塞");
        let blocker = create_queued_task(conn, &project_id, "前置");
        add_dependency(conn, &dependent.id, &blocker.id).unwrap();
        set_item_created_at(conn, &dependent.id, "2026-01-01T00:00:00.000Z");
        set_item_created_at(conn, &blocker.id, "2026-01-02T00:00:00.000Z");
        assert_eq!(
            next_ready_task(conn, &project_id).unwrap().as_deref(),
            Some(blocker.id.as_str())
        );

        remove_dependency(conn, &dependent.id, &blocker.id).unwrap();
        assert_eq!(
            next_ready_task(conn, &project_id).unwrap().as_deref(),
            Some(dependent.id.as_str())
        );
        let (blockers, blocks) = task_dependencies(conn, &dependent.id).unwrap();
        assert!(blockers.is_empty());
        assert!(blocks.is_empty());

        // 重复删除是幂等的。
        remove_dependency(conn, &dependent.id, &blocker.id).unwrap();
    }

    #[test]
    fn link_and_unlink_roundtrip() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();
        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let task_item = task_item_id(conn, &task.id).unwrap();
        let note = insert_item(conn, &project_id, "note", "设计笔记", "正文");

        link_item(conn, &task_item, &note, "relates").unwrap();
        // 同一条边重复写入是幂等的。
        link_item(conn, &task_item, &note, "relates").unwrap();
        let edge_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM links WHERE from_item = ?1 AND to_item = ?2",
                params![task_item, note],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(edge_count, 1);

        let err = link_item(conn, &task_item, &note, "unknown").unwrap_err();
        assert!(err.contains("链接类型"), "{err}");

        unlink_item(conn, &task_item, &note, "relates").unwrap();
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM links", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 0);

        // 重复删除同样是幂等的。
        unlink_item(conn, &task_item, &note, "relates").unwrap();
    }

    #[test]
    fn link_rejects_cross_project_and_missing_items() {
        let db = open_db();
        let project_id = project(&db);
        let other_project_id = project(&db);
        let conn = db.conn();
        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let task_item = task_item_id(conn, &task.id).unwrap();
        let foreign = insert_item(conn, &other_project_id, "doc", "外部文档", "正文");

        let err = link_item(conn, &task_item, &foreign, "relates").unwrap_err();
        assert!(err.contains("同一项目"), "{err}");
        assert!(link_item(conn, &task_item, "missing-item", "relates").is_err());
        assert!(link_item(conn, "missing-item", &foreign, "relates").is_err());

        let edge_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM links", [], |row| row.get(0))
            .unwrap();
        assert_eq!(edge_count, 0);
    }

    #[test]
    fn task_context_returns_only_linked_docs_with_bodies() {
        let db = open_db();
        let project_id = project(&db);
        let other_project_id = project(&db);
        let conn = db.conn();
        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let task_item = task_item_id(conn, &task.id).unwrap();

        // 笔记已迁往独立 vault：即使仍有关联边，也不能注入任务上下文。
        let note = insert_item(conn, &project_id, "note", "架构笔记", "笔记正文");
        let doc = insert_item(conn, &project_id, "doc", "设计文档", "文档正文");
        let unrelated = insert_item(conn, &project_id, "doc", "无关文档", "不应出现");
        let reverse = insert_item(conn, &project_id, "doc", "反本文档", "不应出现");
        let _foreign = insert_item(conn, &other_project_id, "doc", "外部文档", "不应出现");

        link_item(conn, &task_item, &note, "relates").unwrap();
        link_item(conn, &task_item, &doc, "relates").unwrap();
        // 其他关系类型、反向边和外部项目都不进入上下文。
        link_item(conn, &task_item, &unrelated, "derived_from").unwrap();
        link_item(conn, &reverse, &task_item, "relates").unwrap();

        let context = task_context(conn, &task.id).unwrap();
        assert_eq!(context.len(), 1);
        assert_eq!(context[0].id, doc);
        assert_eq!(context[0].kind, "doc");
        assert_eq!(context[0].title, "设计文档");
        assert_eq!(context[0].body, "文档正文");

        let meta = task_context_meta(conn, &task.id).unwrap();
        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0].id, doc);

        assert!(task_context(conn, "missing-task").is_err());
    }

    #[test]
    fn task_agents_roundtrip_and_validation() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();
        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();

        // 新任务默认未指派，回退到运行器的全局默认。
        assert_eq!(task_agents(conn, &task.id).unwrap(), (None, None));
        assert_eq!(task.developer_adapter, None);
        assert_eq!(task.reviewer_adapter, None);

        set_task_agents(conn, &task.id, Some("omp"), Some("claude")).unwrap();
        assert_eq!(
            task_agents(conn, &task.id).unwrap(),
            (Some("omp".to_string()), Some("claude".to_string()))
        );
        let detail = get_task(conn, &task.id).unwrap();
        assert_eq!(detail.task.developer_adapter.as_deref(), Some("omp"));
        assert_eq!(detail.task.reviewer_adapter.as_deref(), Some("claude"));
        assert_eq!(
            list_tasks(conn).unwrap()[0].developer_adapter.as_deref(),
            Some("omp")
        );

        // 每次写入覆盖两个角色：传入 None 即清除该角色的指派。
        set_task_agents(conn, &task.id, Some("other"), None).unwrap();
        assert_eq!(
            task_agents(conn, &task.id).unwrap(),
            (Some("other".to_string()), None)
        );

        // 显式清除恢复默认。
        set_task_agents(conn, &task.id, None, None).unwrap();
        assert_eq!(task_agents(conn, &task.id).unwrap(), (None, None));

        // 空 id 不能被写入。
        assert!(set_task_agents(conn, &task.id, Some("  "), None).is_err());
        assert!(set_task_agents(conn, &task.id, None, Some("")).is_err());
        assert!(set_task_agents(conn, "missing-task", Some("omp"), None).is_err());
        assert!(task_agents(conn, "missing-task").is_err());
        assert_eq!(task_agents(conn, &task.id).unwrap(), (None, None));
    }

    #[test]
    fn task_model_roundtrip_and_clear() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();
        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let blocker = create_task(conn, new_task(&project_id, criteria())).unwrap();
        add_dependency(conn, &task.id, &blocker.id).unwrap();

        // 新任务默认使用适配器默认模型。
        assert_eq!(task.model, None);
        assert_eq!(get_task(conn, &task.id).unwrap().task.model, None);
        assert_eq!(list_tasks(conn).unwrap()[0].model, None);

        // 写入时去掉首尾空白；任务详情、列表与依赖摘要都能读回。
        set_task_model(conn, &blocker.id, Some("  gpt-5.2-codex  ")).unwrap();
        assert_eq!(
            get_task(conn, &blocker.id).unwrap().task.model.as_deref(),
            Some("gpt-5.2-codex")
        );
        let listed = list_tasks(conn).unwrap();
        let listed_blocker = listed.iter().find(|row| row.id == blocker.id).unwrap();
        assert_eq!(listed_blocker.model.as_deref(), Some("gpt-5.2-codex"));
        let (blockers, blocks) = task_dependencies(conn, &task.id).unwrap();
        assert!(blocks.is_empty());
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].model.as_deref(), Some("gpt-5.2-codex"));

        // 空串与 None 都清除覆盖。
        set_task_model(conn, &blocker.id, Some("   ")).unwrap();
        assert_eq!(get_task(conn, &blocker.id).unwrap().task.model, None);
        set_task_model(conn, &blocker.id, Some("claude-sonnet-4-5")).unwrap();
        set_task_model(conn, &blocker.id, None).unwrap();
        assert_eq!(get_task(conn, &blocker.id).unwrap().task.model, None);
        assert_eq!(
            list_tasks(conn)
                .unwrap()
                .iter()
                .find(|row| row.id == blocker.id)
                .unwrap()
                .model,
            None
        );

        assert!(set_task_model(conn, "missing-task", Some("m")).is_err());
    }

    #[test]
    fn concurrency_get_set_clamps_to_supported_range() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        assert_eq!(project_concurrency(conn, &project_id).unwrap(), 1);

        set_project_concurrency(conn, &project_id, 4).unwrap();
        assert_eq!(project_concurrency(conn, &project_id).unwrap(), 4);

        for invalid in [0, -7] {
            set_project_concurrency(conn, &project_id, invalid).unwrap();
            assert_eq!(project_concurrency(conn, &project_id).unwrap(), 1);
        }

        set_project_concurrency(conn, &project_id, 99).unwrap();
        assert_eq!(project_concurrency(conn, &project_id).unwrap(), 16);

        set_project_concurrency(conn, &project_id, 16).unwrap();
        assert_eq!(project_concurrency(conn, &project_id).unwrap(), 16);

        assert!(set_project_concurrency(conn, "missing-project", 2).is_err());
        assert!(project_concurrency(conn, "missing-project").is_err());
    }

    #[test]
    fn require_checks_defaults_false_and_roundtrips() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        assert!(!project_require_checks(conn, &project_id).unwrap());

        set_project_require_checks(conn, &project_id, true).unwrap();
        assert!(project_require_checks(conn, &project_id).unwrap());

        set_project_require_checks(conn, &project_id, false).unwrap();
        assert!(!project_require_checks(conn, &project_id).unwrap());

        assert!(set_project_require_checks(conn, "missing-project", true).is_err());
        assert!(project_require_checks(conn, "missing-project").is_err());
    }

    #[test]
    fn settings_roundtrip_overwrite_and_delete() {
        let db = open_db();
        let conn = db.conn();

        assert_eq!(get_setting(conn, SETTING_DEFAULT_AGENT).unwrap(), None);

        set_setting(conn, SETTING_DEFAULT_AGENT, "omp").unwrap();
        assert_eq!(
            get_setting(conn, SETTING_DEFAULT_AGENT).unwrap().as_deref(),
            Some("omp")
        );

        // 覆盖写入并刷新 updated_at（先伪造一个旧时间戳，避免同毫秒竞争）。
        conn.execute(
            "UPDATE app_settings SET updated_at = '2000-01-01T00:00:00.000Z' WHERE key = ?1",
            params![SETTING_DEFAULT_AGENT],
        )
        .unwrap();
        set_setting(conn, SETTING_DEFAULT_AGENT, "codex").unwrap();
        assert_eq!(
            get_setting(conn, SETTING_DEFAULT_AGENT).unwrap().as_deref(),
            Some("codex")
        );
        let updated_at: String = conn
            .query_row(
                "SELECT updated_at FROM app_settings WHERE key = ?1",
                params![SETTING_DEFAULT_AGENT],
                |row| row.get(0),
            )
            .unwrap();
        assert_ne!(updated_at, "2000-01-01T00:00:00.000Z");

        delete_setting(conn, SETTING_DEFAULT_AGENT).unwrap();
        assert_eq!(get_setting(conn, SETTING_DEFAULT_AGENT).unwrap(), None);
        // 重复删除不报错。
        delete_setting(conn, SETTING_DEFAULT_AGENT).unwrap();
    }

    #[test]
    fn worktree_retention_days_defaults_and_clamps() {
        let db = open_db();
        let conn = db.conn();

        assert_eq!(worktree_retention_days(conn), 7, "未设置时回退 7 天");

        for raw in ["", "abc", "1.5", "-3"] {
            set_setting(conn, SETTING_WORKTREE_RETENTION_DAYS, raw).unwrap();
            assert_eq!(worktree_retention_days(conn), 7, "非法值回退 7 天：{raw}");
        }

        set_setting(conn, SETTING_WORKTREE_RETENTION_DAYS, "0").unwrap();
        assert_eq!(worktree_retention_days(conn), 1, "下界收敛到 1");

        set_setting(conn, SETTING_WORKTREE_RETENTION_DAYS, "365").unwrap();
        assert_eq!(worktree_retention_days(conn), 90, "上界收敛到 90");

        set_setting(conn, SETTING_WORKTREE_RETENTION_DAYS, "30").unwrap();
        assert_eq!(worktree_retention_days(conn), 30);
    }

    #[test]
    fn list_reviews_orders_by_created_at_and_counts_evidence() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();

        // 第一次运行：审查通过、无风险，1 条证据通过。
        let pass_run = start_task_run(&db, &task.id);
        set_review(
            conn,
            NewReview {
                run_id: pass_run.id.clone(),
                verdict: "pass".into(),
                summary: Some("证据齐全".into()),
                risk_flags: vec![],
                test_diff: vec![],
            },
        )
        .unwrap();
        add_evidence(conn, evidence(&pass_run.id, "c1", 0)).unwrap();

        // 第二次运行：待人工、带风险标记，2 条证据其中 1 条失败。
        let human_run = start_task_run(&db, &task.id);
        set_review(
            conn,
            NewReview {
                run_id: human_run.id.clone(),
                verdict: "needs_human".into(),
                summary: Some("测试断言被放宽".into()),
                risk_flags: vec!["test_weakened".into()],
                test_diff: vec!["tests/gate.rs +1/-1".into()],
            },
        )
        .unwrap();
        add_evidence(conn, evidence(&human_run.id, "c1", 0)).unwrap();
        add_evidence(conn, evidence(&human_run.id, "c2", 1)).unwrap();

        // artifact / review 辅助证据的 criterion_id 为 NULL：退出码为 0 也不计入
        // 验收证据的分母或通过数。
        for (run_id, kind) in [
            (pass_run.id.as_str(), "artifact"),
            (pass_run.id.as_str(), "review"),
            (human_run.id.as_str(), "artifact"),
            (human_run.id.as_str(), "review"),
        ] {
            add_evidence(conn, auxiliary_evidence(run_id, kind)).unwrap();
        }

        // 固定时间戳，避免两条 review 落在同一毫秒导致顺序不稳定。
        for (run_id, created_at) in [
            (pass_run.id.as_str(), "2025-01-01T00:00:00.000Z"),
            (human_run.id.as_str(), "2025-01-02T00:00:00.000Z"),
        ] {
            conn.execute(
                "UPDATE reviews SET created_at = ?2 WHERE run_id = ?1",
                params![run_id, created_at],
            )
            .unwrap();
        }

        let rows = list_reviews(conn, &project_id).unwrap();
        assert_eq!(rows.len(), 2);

        let latest = &rows[0];
        assert_eq!(latest.run_id, human_run.id);
        assert_eq!(latest.task_id, task.id);
        assert_eq!(latest.task_title, "实现证据门");
        assert_eq!(latest.verdict, "needs_human");
        assert_eq!(latest.summary.as_deref(), Some("测试断言被放宽"));
        assert_eq!(latest.risk_flags, vec!["test_weakened".to_string()]);
        assert_eq!(latest.test_diff, vec!["tests/gate.rs +1/-1".to_string()]);
        assert_eq!(latest.created_at, "2025-01-02T00:00:00.000Z");
        // 只统计验收证据：2 条挂验收标准的证据，额外 2 条辅助证据被排除。
        assert_eq!(latest.evidence_total, 2);
        assert_eq!(latest.evidence_passed, 1);
        assert_eq!(latest.pr_url, None);

        let previous = &rows[1];
        assert_eq!(previous.run_id, pass_run.id);
        assert_eq!(previous.verdict, "pass");
        assert!(previous.risk_flags.is_empty());
        assert!(previous.test_diff.is_empty());
        // 全绿 run：辅助证据退出码为 0 也不进入统计，因此 total == passed。
        assert_eq!(previous.evidence_total, 1);
        assert_eq!(previous.evidence_passed, 1);

        assert!(list_reviews(conn, "missing-project").unwrap().is_empty());
    }

    fn plan_input(project_id: &str, stages: Vec<Vec<&str>>) -> NewPlan {
        NewPlan {
            project_id: project_id.to_string(),
            title: "交付 v1".into(),
            goal: "按阶段完成".into(),
            stages: stages
                .into_iter()
                .enumerate()
                .map(|(index, task_ids)| NewPlanStage {
                    title: format!("阶段 {}", index + 1),
                    task_ids: task_ids.into_iter().map(str::to_string).collect(),
                })
                .collect(),
        }
    }

    #[test]
    fn plan_creation_validates_stages_tasks_and_duplicates() {
        let db = open_db();
        let conn = db.conn();
        let project_id = project(&db);
        let other_project = project(&db);
        let first = create_queued_task(conn, &project_id, "阶段一任务");
        let second = create_queued_task(conn, &project_id, "阶段二任务");
        let foreign = create_queued_task(conn, &other_project, "其他项目任务");

        let err = create_plan(conn, plan_input(&project_id, Vec::new())).unwrap_err();
        assert!(err.contains("至少需要一个阶段"), "{err}");

        let err = create_plan(conn, plan_input(&project_id, vec![vec![]])).unwrap_err();
        assert!(err.contains("至少需要一个任务"), "{err}");

        let err = create_plan(conn, plan_input(&project_id, vec![vec!["missing"]])).unwrap_err();
        assert!(err.contains("任务不存在"), "{err}");

        let err = create_plan(
            conn,
            plan_input(&project_id, vec![vec![foreign.id.as_str()]]),
        )
        .unwrap_err();
        assert!(err.contains("不属于该项目"), "{err}");

        let err = create_plan(
            conn,
            plan_input(
                &project_id,
                vec![vec![first.id.as_str()], vec![first.id.as_str()]],
            ),
        )
        .unwrap_err();
        assert!(err.contains("重复"), "{err}");

        let plan = create_plan(
            conn,
            plan_input(
                &project_id,
                vec![vec![first.id.as_str()], vec![second.id.as_str()]],
            ),
        )
        .unwrap();
        assert_eq!(plan.status, PlanStatus::Draft);
        assert_eq!(plan.stages.len(), 2);
        assert_eq!(plan.stages[0].seq, 1);
        assert_eq!(plan.stages[0].tasks.len(), 1);
        assert_eq!(plan.stages[0].tasks[0].id, first.id);
        assert_eq!(plan.stages[1].tasks[0].id, second.id);

        // 同一任务不能同时属于两个计划。
        let err =
            create_plan(conn, plan_input(&project_id, vec![vec![first.id.as_str()]])).unwrap_err();
        assert!(err.contains("已属于其他计划"), "{err}");

        let plans = list_plans(conn, &project_id).unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].id, plan.id);
        assert_eq!(get_plan(conn, &plan.id).unwrap().id, plan.id);
        assert!(list_plans(conn, &other_project).unwrap().is_empty());
        assert!(get_plan(conn, "missing").is_err());
    }

    #[test]
    fn plan_stage_input_accepts_camel_case_task_ids() {
        let stage: NewPlanStage =
            serde_json::from_str(r#"{"title":"阶段","taskIds":["t1","t2"]}"#).unwrap();
        assert_eq!(stage.task_ids, vec!["t1".to_string(), "t2".to_string()]);

        let stage: NewPlanStage =
            serde_json::from_str(r#"{"title":"阶段","task_ids":["t1"]}"#).unwrap();
        assert_eq!(stage.task_ids, vec!["t1".to_string()]);
    }

    #[test]
    fn plan_stage_lifecycle_unlocks_reviews_and_completes() {
        let db = open_db();
        let conn = db.conn();
        let project_id = project(&db);
        let first = create_queued_task(conn, &project_id, "阶段一任务");
        let second = create_queued_task(conn, &project_id, "阶段二任务");
        let plan = create_plan(
            conn,
            plan_input(
                &project_id,
                vec![vec![first.id.as_str()], vec![second.id.as_str()]],
            ),
        )
        .unwrap();

        // 未开始时计划为 draft，所有阶段锁定。
        assert_eq!(plan.stages[0].status, StageStatus::Locked);
        assert_eq!(plan.stages[1].status, StageStatus::Locked);

        let plan = start_plan(conn, &plan.id).unwrap();
        assert_eq!(plan.status, PlanStatus::Active);
        assert!(start_plan(conn, &plan.id).is_err(), "重复开始应被拒绝");

        // 任务未全部合并时阶段不接受验收。
        let err = accept_stage(conn, &plan.stages[0].id).unwrap_err();
        assert!(err.contains("待验收"), "{err}");

        merge_task(conn, &first.id);
        let plan = get_plan(conn, &plan.id).unwrap();
        assert_eq!(plan.stages[0].status, StageStatus::Review);
        assert_eq!(plan.stages[1].status, StageStatus::Locked);

        let plan = accept_stage(conn, &plan.stages[0].id).unwrap();
        assert_eq!(plan.status, PlanStatus::Active);
        assert_eq!(plan.stages[0].status, StageStatus::Passed);
        assert!(plan.stages[0].passed_at.is_some());
        assert_eq!(plan.stages[1].status, StageStatus::Active);
        assert!(
            accept_stage(conn, &plan.stages[0].id).is_err(),
            "重复验收应被拒绝"
        );

        merge_task(conn, &second.id);
        let plan = get_plan(conn, &plan.id).unwrap();
        assert_eq!(plan.stages[1].status, StageStatus::Review);

        let plan = accept_stage(conn, &plan.stages[1].id).unwrap();
        assert_eq!(plan.status, PlanStatus::Done);
        assert_eq!(plan.stages[1].status, StageStatus::Passed);

        assert!(
            start_plan(conn, &plan.id).is_err(),
            "已完成的计划不能重新开始"
        );
        assert!(accept_stage(conn, "missing").is_err());
        assert!(start_plan(conn, "missing").is_err());
    }

    #[test]
    fn plan_draft_cannot_accept_stage_until_started() {
        let db = open_db();
        let conn = db.conn();
        let project_id = project(&db);
        let task = create_queued_task(conn, &project_id, "草稿阶段任务");
        let plan =
            create_plan(conn, plan_input(&project_id, vec![vec![task.id.as_str()]])).unwrap();

        // 任务全部合并，但计划仍是草稿：阶段保持锁定，验收被拒绝。
        merge_task(conn, &task.id);
        let plan = get_plan(conn, &plan.id).unwrap();
        assert_eq!(plan.status, PlanStatus::Draft);
        assert_eq!(plan.stages[0].status, StageStatus::Locked);

        let err = accept_stage(conn, &plan.stages[0].id).unwrap_err();
        assert!(err.contains("计划尚未启动"), "{err}");
        let unchanged = get_plan(conn, &plan.id).unwrap();
        assert_eq!(unchanged.status, PlanStatus::Draft);
        assert!(unchanged.stages[0].passed_at.is_none());

        // 启动后同一阶段进入待验收，可正常验收并完成计划。
        let plan = start_plan(conn, &plan.id).unwrap();
        assert_eq!(plan.stages[0].status, StageStatus::Review);
        let plan = accept_stage(conn, &plan.stages[0].id).unwrap();
        assert_eq!(plan.status, PlanStatus::Done);
        assert_eq!(plan.stages[0].status, StageStatus::Passed);
        assert!(
            accept_stage(conn, &plan.stages[0].id).is_err(),
            "已完成计划的阶段不能再次验收"
        );
    }
}
