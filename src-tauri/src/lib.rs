mod agent;
mod db;
mod evidence;
mod github;
mod gitops;
mod notify;
mod orchestrator;
mod planner;
mod prompts;
mod scheduler;
mod search;
mod store;
mod sync;
mod vaults;
mod workspace;

use agent::{
    AdapterRegistry, AgentAdapter, AgentEvent, AgentManifest, Decision, EventSink, RunSpec,
    RunStatus,
};
use db::{Db, Project, RegisterProjectInput, canonical_repo};
use evidence::DiscoveredCommand;
use rusqlite::{OptionalExtension, params};
use scheduler::Scheduler;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use store::{Criterion, DecisionRow, EventRow, TaskDetail, TaskSummary};
use tauri::{
    Manager, State, WindowEvent,
    menu::{MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

struct AppState {
    db: Arc<Mutex<Db>>,
    adapters: Arc<AdapterRegistry>,
    /// 未在任务上单独指定适配器时使用的默认适配器 id。
    default_agent: String,
    /// 任务队列的唯一入口；所有执行都在调度线程内串行进行。
    scheduler: Arc<Scheduler>,
    /// GitHub issue 后台同步。
    sync: Arc<sync::SyncEngine>,
}

#[derive(Debug, Deserialize)]
struct CreateTaskInput {
    project_id: String,
    title: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    acceptance: Vec<store::Criterion>,
}

#[derive(Debug, Deserialize)]
struct CreateTaskFromIssueInput {
    item_id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PlanProjectInput {
    project_id: String,
    idea: String,
    /// 缺省时使用全局默认适配器。
    #[serde(default)]
    agent: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateTasksFromPlanInput {
    project_id: String,
    draft: planner::PlanDraft,
}

#[derive(Debug, Deserialize)]
struct ResolveDecisionInput {
    id: String,
    resolution: String,
}

#[derive(Debug, Deserialize)]
struct DependencyInput {
    task_id: String,
    depends_on_task_id: String,
}

#[derive(Debug, Deserialize)]
struct ProjectConcurrencyInput {
    project_id: String,
    limit: i64,
}

#[derive(Debug, Deserialize)]
struct UpdateIssueInput {
    item_id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
    /// 指派人登录名；不传表示不改动。
    #[serde(default)]
    assignee: Option<String>,
    /// `"open"` / `"closed"`；不传表示不改动。
    #[serde(default)]
    state: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AutoMergeInput {
    project_id: String,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct RequireChecksInput {
    project_id: String,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct AutoDocsInput {
    project_id: String,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct AddVaultInput {
    name: String,
    root_path: String,
    #[serde(default)]
    create: bool,
}

#[derive(Debug, Deserialize)]
struct CreateVaultNoteInput {
    vault_id: String,
    path: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SaveVaultNoteInput {
    #[serde(alias = "note_id")]
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GlobalSearchInput {
    query: String,
    /// 缺省 = 跨所有项目检索。
    #[serde(default)]
    project_id: Option<String>,
    /// 缺省 = 20，服务端再夹到 1..=50。
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct SetTaskAgentsInput {
    task_id: String,
    /// 缺省 = 保持不变；null / 空串 = 清除覆盖；字符串 = 指定适配器。
    #[serde(default, deserialize_with = "double_option")]
    developer_adapter: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    reviewer_adapter: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
struct SetTaskModelInput {
    task_id: String,
    /// 缺省 / null / 空串 = 使用适配器默认模型。
    #[serde(default)]
    model: Option<String>,
}

/// 区分 JSON 字段缺省（外层 `None`）与显式 `null`（`Some(None)`）。
fn double_option<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize)]
struct ContextLinkInput {
    task_id: String,
    item_id: String,
}

#[derive(Debug, Serialize)]
struct DependencyList {
    blockers: Vec<TaskSummary>,
    blocks: Vec<TaskSummary>,
}

fn db_err(e: rusqlite::Error) -> String {
    e.to_string()
}

fn list_project_decisions(
    conn: &rusqlite::Connection,
    project_id: &str,
) -> Result<Vec<DecisionRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, task_id, kind, prompt, status, created_at, resolution
             FROM decisions
             WHERE project_id = ?1 AND status = 'open'
             ORDER BY created_at ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![project_id], |row| {
            Ok(DecisionRow {
                id: row.get(0)?,
                task_id: row.get(1)?,
                kind: row.get(2)?,
                prompt: row.get(3)?,
                status: row.get(4)?,
                created_at: row.get(5)?,
                resolution: row.get(6)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

fn list_task_events(conn: &rusqlite::Connection, task_id: &str) -> Result<Vec<EventRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, type, payload_json, at
             FROM events
             WHERE entity = 'task' AND entity_id = ?1
             ORDER BY at ASC, rowid ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![task_id], |row| {
            Ok(EventRow {
                id: row.get(0)?,
                kind: row.get(1)?,
                payload_json: row.get(2)?,
                at: row.get(3)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

#[tauri::command]
fn list_projects(state: State<AppState>) -> Result<Vec<Project>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    db.list_projects()
}

#[tauri::command]
fn register_project(
    state: State<AppState>,
    input: RegisterProjectInput,
) -> Result<Project, String> {
    let canonical = canonical_repo(&input.repo_path)?;
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    db.insert_project(input, canonical)
}

#[tauri::command]
fn list_tasks(state: State<AppState>, project_id: String) -> Result<Vec<TaskSummary>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::list_tasks_for_project(db.conn(), &project_id)
}

#[tauri::command]
fn add_dependency(state: State<AppState>, input: DependencyInput) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::add_dependency(db.conn(), &input.task_id, &input.depends_on_task_id)
}

#[tauri::command]
fn remove_dependency(state: State<AppState>, input: DependencyInput) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::remove_dependency(db.conn(), &input.task_id, &input.depends_on_task_id)
}

#[tauri::command]
fn list_dependencies(state: State<AppState>, task_id: String) -> Result<DependencyList, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let (blockers, blocks) = store::task_dependencies(db.conn(), &task_id)?;
    Ok(DependencyList { blockers, blocks })
}

#[tauri::command]
fn get_project_concurrency(state: State<AppState>, project_id: String) -> Result<i64, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::project_concurrency(db.conn(), &project_id)
}

#[tauri::command]
fn set_project_concurrency(
    state: State<AppState>,
    input: ProjectConcurrencyInput,
) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::set_project_concurrency(db.conn(), &input.project_id, input.limit)
}

/// 规划器单次运行的硬超时（秒）；超时即取消并回退到默认验收标准。
const PLANNER_TIMEOUT_SECS: u64 = 90;

/// 通过 agent 适配器执行一次规划，返回校验通过的验收标准。
///
/// 结果必须非空，且每条标准的 `cmd` 都要与发现列表中的某条命令完全一致；
/// 启动失败、RPC 异常、超时或解析失败一律返回 `None`，绝不阻塞任务创建。
fn plan_acceptance(
    adapter: &dyn AgentAdapter,
    repo: &Path,
    title: &str,
    intent: &str,
    commands: &[DiscoveredCommand],
) -> Option<Vec<Criterion>> {
    if commands.is_empty() {
        return None;
    }

    let prompt = prompts::planner_prompt(title, intent, commands);
    let (tx, rx) = mpsc::channel::<AgentEvent>();
    let sink: EventSink = Arc::new(move |event| {
        let _ = tx.send(event);
    });
    let spec = RunSpec {
        cwd: repo.to_path_buf(),
        prompt,
        model: None,
        max_minutes: 2,
        env_allowlist: Vec::new(),
    };
    let handle = adapter.start(spec, sink).ok()?;

    let deadline = Instant::now() + Duration::from_secs(PLANNER_TIMEOUT_SECS);
    let mut output = String::new();
    let mut ended = false;
    while !ended {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining.min(Duration::from_millis(500))) {
            Ok(AgentEvent::TextDelta { text }) => output.push_str(&text),
            Ok(AgentEvent::ApprovalRequest { request_id, .. }) => {
                let _ = adapter.respond(&handle, &request_id, Decision::Deny);
            }
            Ok(AgentEvent::Ended { .. }) => ended = true,
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    if !ended {
        let _ = adapter.cancel(&handle);
    }
    let _ = adapter.close(&handle);
    if !ended {
        return None;
    }

    let criteria = prompts::parse_criteria(&output).ok()?;
    if criteria.is_empty() {
        return None;
    }
    let grounded = criteria
        .iter()
        .all(|criterion| commands.iter().any(|command| command.cmd == criterion.cmd));
    if grounded { Some(criteria) } else { None }
}

/// 生成任务的验收标准：优先采用规划器输出，失败或未注册适配器时回退到默认标准。
///
/// 空命令列表由 [`plan_acceptance`] 直接返回 `None`，因此这里同样得到空默认列表，
/// 由调用方转换成“未发现命令”的错误。
fn generate_acceptance(
    adapter: Option<&dyn AgentAdapter>,
    repo: &Path,
    title: &str,
    intent: &str,
    commands: &[DiscoveredCommand],
) -> Vec<Criterion> {
    adapter
        .and_then(|adapter| plan_acceptance(adapter, repo, title, intent, commands))
        .unwrap_or_else(|| evidence::default_acceptance(commands))
}

/// 多任务规划的单次硬超时（秒）。
const PLAN_TIMEOUT_SECS: u64 = 180;

/// 通过适配器生成任务草案。
///
/// 收集全部 `TextDelta` 作为输出；审批请求一律拒绝；无论成功、失败、超时还是
/// 连接断开，都会关闭会话，未正常结束时额外取消。输出交给
/// [`planner::parse_plan`] 严格校验后才返回。
fn run_plan(
    adapter: &dyn AgentAdapter,
    project: &Project,
    idea: &str,
    commands: &[DiscoveredCommand],
) -> Result<planner::PlanDraft, String> {
    let prompt = planner::plan_prompt(idea, project, commands);
    let (tx, rx) = mpsc::channel::<AgentEvent>();
    let sink: EventSink = Arc::new(move |event| {
        let _ = tx.send(event);
    });
    let spec = RunSpec {
        cwd: PathBuf::from(&project.repo_path),
        prompt,
        model: None,
        max_minutes: 3,
        env_allowlist: Vec::new(),
    };
    let handle = adapter
        .start(spec, sink)
        .map_err(|error| format!("无法启动规划器：{error}"))?;

    let deadline = Instant::now() + Duration::from_secs(PLAN_TIMEOUT_SECS);
    let mut output = String::new();
    let mut ended: Option<RunStatus> = None;
    let mut last_error: Option<String> = None;
    let mut timed_out = false;
    let mut disconnected = false;

    while ended.is_none() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            timed_out = true;
            break;
        }
        match rx.recv_timeout(remaining.min(Duration::from_millis(500))) {
            Ok(AgentEvent::TextDelta { text }) => output.push_str(&text),
            Ok(AgentEvent::ApprovalRequest { request_id, .. }) => {
                let _ = adapter.respond(&handle, &request_id, Decision::Deny);
            }
            Ok(AgentEvent::Error { message, .. }) => last_error = Some(message),
            Ok(AgentEvent::Ended { status }) => ended = Some(status),
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                disconnected = true;
                break;
            }
        }
    }

    if !matches!(ended, Some(RunStatus::Completed)) {
        let _ = adapter.cancel(&handle);
    }
    let _ = adapter.close(&handle);

    if timed_out {
        return Err(format!("规划器在 {PLAN_TIMEOUT_SECS} 秒内未完成"));
    }
    if disconnected {
        return Err("规划器连接中断".to_string());
    }
    match ended {
        Some(RunStatus::Completed) => {}
        Some(RunStatus::Failed) => {
            return Err(last_error.unwrap_or_else(|| "规划器运行失败".to_string()));
        }
        Some(RunStatus::Cancelled) => return Err("规划器运行被取消".to_string()),
        None => return Err("规划器未正常结束".to_string()),
    }

    planner::parse_plan(&output, commands)
        .map_err(|error| format!("规划器输出无法通过校验：{error}"))
}

#[tauri::command(async)]
fn plan_project(
    state: State<AppState>,
    input: PlanProjectInput,
) -> Result<planner::PlanDraft, String> {
    let idea = input.idea.trim();
    if idea.is_empty() {
        return Err("请提供要规划的想法".to_string());
    }

    let project = {
        let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
        db.get_project(&input.project_id)?
            .ok_or_else(|| "项目不存在".to_string())?
    };

    let requested = input
        .agent
        .as_deref()
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
        .unwrap_or(&state.default_agent);
    let adapter = state
        .adapters
        .get(requested)
        .ok_or_else(|| format!("适配器未注册：{requested}"))?;

    let repo = Path::new(&project.repo_path);
    let commands = evidence::discover_commands(repo);
    if commands.is_empty() {
        return Err("未发现可执行的测试/构建命令，请先配置测试或构建脚本".to_string());
    }

    run_plan(adapter.as_ref(), &project, idea, &commands)
}

#[tauri::command]
fn create_task(state: State<AppState>, input: CreateTaskInput) -> Result<TaskDetail, String> {
    let project = {
        let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
        db.get_project(&input.project_id)?
            .ok_or_else(|| "项目不存在".to_string())?
    };

    let criteria = if input.acceptance.is_empty() {
        let repo = PathBuf::from(&project.repo_path);
        let commands = evidence::discover_commands(&repo);
        let adapter = state.adapters.get(&state.default_agent);
        let generated = generate_acceptance(
            adapter.as_deref(),
            &repo,
            &input.title,
            &input.prompt,
            &commands,
        );
        if generated.is_empty() {
            return Err("未发现可执行的测试/构建命令，请手动提供验收标准".to_string());
        }
        generated
    } else {
        input.acceptance
    };

    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let task = store::create_task(
        db.conn(),
        store::NewTask {
            project_id: input.project_id,
            title: input.title,
            prompt: input.prompt,
            criteria,
        },
    )?;
    store::get_task(db.conn(), &task.id)
}

/// `create_task_from_issue` 的数据库逻辑；拆出来以便直接对 `Connection` 做单测。
fn convert_issue_to_task(
    conn: &rusqlite::Connection,
    input: CreateTaskFromIssueInput,
) -> Result<TaskDetail, String> {
    let CreateTaskFromIssueInput {
        item_id,
        title,
        prompt,
    } = input;

    let (project_id, kind, item_title, item_body) = conn
        .query_row(
            "SELECT project_id, kind, title, body FROM items WHERE id = ?1",
            params![item_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("条目不存在：{item_id}"))?;
    if kind != "issue" {
        return Err(format!("只能将 issue 条目转换为任务：{item_id}"));
    }

    let repo_path: String = conn
        .query_row(
            "SELECT repo_path FROM projects WHERE id = ?1",
            params![project_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("项目不存在：{project_id}"))?;

    // 已转换过：直接返回已有任务，保证重复调用幂等。
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM tasks WHERE item_id = ?1",
            params![item_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    if let Some(task_id) = existing {
        return store::get_task(conn, &task_id);
    }

    let criteria =
        evidence::default_acceptance(&evidence::discover_commands(Path::new(&repo_path)));
    if criteria.is_empty() {
        return Err("未发现可执行的测试/构建命令，请手动提供验收标准".to_string());
    }

    let title = title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if let Some(title) = title.as_deref() {
        conn.execute(
            "UPDATE items
             SET title = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?1",
            params![item_id, title],
        )
        .map_err(db_err)?;
    }

    let prompt = prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let fallback = title.as_deref().unwrap_or(&item_title);
            if item_body.trim().is_empty() {
                fallback.to_string()
            } else {
                item_body
            }
        });

    let task = store::create_task_for_item(conn, &item_id, &prompt, criteria)?;
    store::get_task(conn, &task.id)
}

#[tauri::command]
fn create_task_from_issue(
    state: State<AppState>,
    input: CreateTaskFromIssueInput,
) -> Result<TaskDetail, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    convert_issue_to_task(db.conn(), input)
}

/// 校验草案并把每个任务写入数据库。
///
/// 与 [`plan_project`] 使用同一套 [`planner::validate_plan`] 规则，因此前端回传的
/// 草案在落库前会再次被严格校验（包括命令必须仍存在于发现列表）。
fn create_plan_tasks(
    conn: &rusqlite::Connection,
    project_id: &str,
    repo: &Path,
    draft: planner::PlanDraft,
) -> Result<Vec<TaskDetail>, String> {
    let commands = evidence::discover_commands(repo);
    if commands.is_empty() {
        return Err("未发现可执行的测试/构建命令，请先配置测试或构建脚本".to_string());
    }
    let draft = planner::validate_plan(draft, &commands)?;
    create_tasks_from_draft(conn, project_id, draft)
}

/// 在单个项目内把草案落成 draft 任务及其依赖边。
///
/// 调用方应先用 [`planner::validate_plan`] 校验；本函数仍负责任何中途失败后的
/// 回滚：删除已创建的 issue item，级联清除任务、验收数据与依赖边。
fn create_tasks_from_draft(
    conn: &rusqlite::Connection,
    project_id: &str,
    draft: planner::PlanDraft,
) -> Result<Vec<TaskDetail>, String> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
            params![project_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if !exists {
        return Err(format!("项目不存在：{project_id}"));
    }

    let mut created: Vec<store::TaskRow> = Vec::with_capacity(draft.issues.len());
    let result = (|| -> Result<Vec<TaskDetail>, String> {
        for issue in &draft.issues {
            let task = store::create_task(
                conn,
                store::NewTask {
                    project_id: project_id.to_string(),
                    title: issue.title.clone(),
                    prompt: issue.prompt.clone(),
                    criteria: issue.acceptance.clone(),
                },
            )?;
            created.push(task);
        }

        for (index, issue) in draft.issues.iter().enumerate() {
            for &blocker_index in &issue.depends_on {
                let blocker = created.get(blocker_index).ok_or_else(|| {
                    format!("任务「{}」依赖不存在的索引：{blocker_index}", issue.title)
                })?;
                store::add_dependency(conn, &created[index].id, &blocker.id)?;
            }
        }

        let mut details = Vec::with_capacity(created.len());
        for task in &created {
            details.push(store::get_task(conn, &task.id)?);
        }
        Ok(details)
    })();

    if result.is_err() {
        rollback_plan_tasks(conn, &created);
    }
    result
}

/// 删除已创建任务的 issue item；外键级联清理 tasks / links。
fn rollback_plan_tasks(conn: &rusqlite::Connection, tasks: &[store::TaskRow]) {
    for task in tasks {
        let item_id: Option<String> = conn
            .query_row(
                "SELECT item_id FROM tasks WHERE id = ?1",
                params![task.id],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten();
        if let Some(item_id) = item_id {
            let _ = conn.execute("DELETE FROM items WHERE id = ?1", params![item_id]);
        }
    }
}

#[tauri::command]
fn create_tasks_from_plan(
    state: State<AppState>,
    input: CreateTasksFromPlanInput,
) -> Result<Vec<TaskDetail>, String> {
    let project = {
        let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
        db.get_project(&input.project_id)?
            .ok_or_else(|| "项目不存在".to_string())?
    };
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    create_plan_tasks(
        db.conn(),
        &project.id,
        Path::new(&project.repo_path),
        input.draft,
    )
}

#[tauri::command]
fn get_task(state: State<AppState>, task_id: String) -> Result<TaskDetail, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::get_task(db.conn(), &task_id)
}

#[tauri::command]
fn start_task(state: State<AppState>, task_id: String) -> Result<(), String> {
    state.scheduler.enqueue(&task_id)
}

#[tauri::command]
fn cancel_task(state: State<AppState>, task_id: String) -> Result<(), String> {
    state.scheduler.cancel(&task_id)
}

#[tauri::command]
fn list_decisions(state: State<AppState>, project_id: String) -> Result<Vec<DecisionRow>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    list_project_decisions(db.conn(), &project_id)
}

#[tauri::command]
fn list_reviews(
    state: State<AppState>,
    project_id: String,
) -> Result<Vec<store::ReviewListItem>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::list_reviews(db.conn(), &project_id)
}

#[tauri::command]
fn resolve_decision(state: State<AppState>, input: ResolveDecisionInput) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let decision = store::resolve_decision(db.conn(), &input.id, &input.resolution)?;
    // Agent approvals only unblock the running session; they must not move the task.
    if decision.kind != "agent_approval" {
        if let Some(task_id) = decision.task_id.as_deref() {
            let current = store::get_task(db.conn(), task_id)?.task.status;
            if matches!(
                current,
                store::TaskStatus::NeedsHuman | store::TaskStatus::Interrupted
            ) {
                let allow = matches!(input.resolution.as_str(), "allow" | "approve" | "retry");
                let next = if allow {
                    store::TaskStatus::Queued
                } else {
                    store::TaskStatus::Draft
                };
                store::transition(db.conn(), task_id, next)?;
            }
        }
    }
    Ok(())
}

#[tauri::command]
fn list_events(state: State<AppState>, task_id: String) -> Result<Vec<EventRow>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    list_task_events(db.conn(), &task_id)
}

#[tauri::command]
fn list_adapters(state: State<AppState>) -> Vec<AgentManifest> {
    state.adapters.manifests()
}

#[tauri::command]
fn sync_now(state: State<AppState>, project_id: String) -> Result<(), String> {
    state.sync.sync_project(&project_id)
}

#[tauri::command]
fn update_issue(state: State<AppState>, input: UpdateIssueInput) -> Result<(), String> {
    state.sync.update_remote_issue(
        &input.item_id,
        input.title,
        input.body,
        input.assignee,
        input.state,
    )
}

#[tauri::command]
fn publish_task(state: State<AppState>, task_id: String) -> Result<u64, String> {
    state.sync.publish_task(&task_id)
}

#[tauri::command]
fn get_project_issues(
    state: State<AppState>,
    project_id: String,
) -> Result<Vec<store::ProjectIssue>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::list_project_issues(db.conn(), &project_id)
}

#[tauri::command]
fn get_auto_merge(state: State<AppState>, project_id: String) -> Result<bool, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::project_auto_merge(db.conn(), &project_id)
}

#[tauri::command]
fn set_auto_merge(state: State<AppState>, input: AutoMergeInput) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::set_project_auto_merge(db.conn(), &input.project_id, input.enabled)
}

#[tauri::command]
fn get_require_checks(state: State<AppState>, project_id: String) -> Result<bool, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::project_require_checks(db.conn(), &project_id)
}

#[tauri::command]
fn set_require_checks(state: State<AppState>, input: RequireChecksInput) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::set_project_require_checks(db.conn(), &input.project_id, input.enabled)
}

#[tauri::command]
fn get_auto_docs(state: State<AppState>, project_id: String) -> Result<bool, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::project_auto_docs(db.conn(), &project_id)
}

#[tauri::command]
fn set_auto_docs(state: State<AppState>, input: AutoDocsInput) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::set_project_auto_docs(db.conn(), &input.project_id, input.enabled)
}

#[tauri::command]
fn set_task_agents(state: State<AppState>, input: SetTaskAgentsInput) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let conn = db.conn();
    let (developer, reviewer) = store::task_agents(conn, &input.task_id)?;

    let apply = |label: &str, incoming: Option<Option<String>>, current: Option<String>| {
        match incoming {
            // 字段缺省：保持当前值。
            None => Ok(current),
            // 显式 null / 空串：清除覆盖，回退到默认适配器。
            Some(None) => Ok(None),
            Some(Some(value)) => {
                let value = value.trim();
                if value.is_empty() {
                    return Ok(None);
                }
                if state.adapters.get(value).is_none() {
                    return Err(format!("{label}适配器未注册：{value}"));
                }
                Ok(Some(value.to_string()))
            }
        }
    };

    let developer = apply("开发者", input.developer_adapter, developer)?;
    let reviewer = apply("审查者", input.reviewer_adapter, reviewer)?;
    store::set_task_agents(
        conn,
        &input.task_id,
        developer.as_deref(),
        reviewer.as_deref(),
    )
}

#[tauri::command]
fn set_task_model(state: State<AppState>, input: SetTaskModelInput) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::set_task_model(db.conn(), &input.task_id, input.model.as_deref())
}

// ---------------------------------------------------------------------------
// 笔记库（vault）
// ---------------------------------------------------------------------------

#[tauri::command]
fn add_vault(state: State<AppState>, input: AddVaultInput) -> Result<vaults::Vault, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::add_vault(db.conn(), &input.name, &input.root_path, input.create)
}

#[tauri::command]
fn list_vaults(state: State<AppState>) -> Result<Vec<vaults::Vault>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::list_vaults(db.conn())
}

#[tauri::command]
fn remove_vault(state: State<AppState>, vault_id: String) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::remove_vault(db.conn(), &vault_id)
}

#[tauri::command]
fn index_vault(state: State<AppState>, vault_id: String) -> Result<usize, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let vault = vaults::get_vault(db.conn(), &vault_id)?;
    vaults::index_vault(db.conn(), &vault)
}

#[tauri::command]
fn vault_tree(state: State<AppState>, vault_id: String) -> Result<Vec<vaults::VaultEntry>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::vault_tree(db.conn(), &vault_id)
}

#[tauri::command]
fn get_vault_note(state: State<AppState>, note_id: String) -> Result<vaults::VaultNote, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::get_vault_note(db.conn(), &note_id)
}

#[tauri::command]
fn create_vault_note(
    state: State<AppState>,
    input: CreateVaultNoteInput,
) -> Result<vaults::VaultNote, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let vault = vaults::get_vault(db.conn(), &input.vault_id)?;
    vaults::create_vault_note(
        db.conn(),
        &vault,
        &input.path,
        input.title.as_deref(),
        input.body.as_deref(),
    )
}

#[tauri::command]
fn save_vault_note(
    state: State<AppState>,
    input: SaveVaultNoteInput,
) -> Result<vaults::VaultNote, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::save_vault_note(
        db.conn(),
        &input.id,
        input.title.as_deref(),
        input.body.as_deref(),
        input.path.as_deref(),
    )
}

#[tauri::command]
fn delete_vault_note(state: State<AppState>, note_id: String) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::delete_vault_note(db.conn(), &note_id)
}

#[tauri::command]
fn search_vault(
    state: State<AppState>,
    vault_id: String,
    query: String,
) -> Result<Vec<vaults::VaultHit>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::search_vault(db.conn(), &vault_id, &query)
}

#[tauri::command]
fn note_backlinks(
    state: State<AppState>,
    note_id: String,
) -> Result<Vec<vaults::Backlink>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::note_backlinks(db.conn(), &note_id)
}

#[tauri::command]
fn vault_graph(state: State<AppState>, vault_id: String) -> Result<vaults::Graph, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::vault_graph(db.conn(), &vault_id)
}

#[tauri::command]
fn list_docs(
    state: State<AppState>,
    project_id: String,
) -> Result<Vec<workspace::DocMeta>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    workspace::list_docs(db.conn(), &project_id)
}

#[tauri::command]
fn get_doc(state: State<AppState>, doc_id: String) -> Result<workspace::Doc, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    workspace::get_doc(db.conn(), &doc_id)
}

#[tauri::command]
fn reindex_workspace(state: State<AppState>, project_id: String) -> Result<usize, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let project = db
        .get_project(&project_id)?
        .ok_or_else(|| "项目不存在".to_string())?;
    workspace::scan_docs(db.conn(), &project)
}

#[tauri::command]
fn search_workspace(
    state: State<AppState>,
    project_id: String,
    query: String,
) -> Result<Vec<workspace::SearchHit>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    workspace::search(db.conn(), &project_id, &query)
}

#[tauri::command]
fn global_search(
    state: State<AppState>,
    input: GlobalSearchInput,
) -> Result<Vec<search::GlobalHit>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    search::global_search(
        db.conn(),
        &input.query,
        input.project_id.as_deref(),
        input.limit.unwrap_or(search::DEFAULT_LIMIT),
    )
}

#[tauri::command]
fn link_context(state: State<AppState>, input: ContextLinkInput) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let task_item = store::task_item_id(db.conn(), &input.task_id)?;
    store::link_item(db.conn(), &task_item, &input.item_id, "relates")
}

#[tauri::command]
fn unlink_context(state: State<AppState>, input: ContextLinkInput) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let task_item = store::task_item_id(db.conn(), &input.task_id)?;
    store::unlink_item(db.conn(), &task_item, &input.item_id, "relates")
}

#[tauri::command]
fn list_context(
    state: State<AppState>,
    task_id: String,
) -> Result<Vec<store::ContextMeta>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::task_context_meta(db.conn(), &task_id)
}

/// 登记但 run 行已消失的 worktree（例如任务行被手工清理）的保留窗口；
/// 失败/需决策任务的 run 行仍在库，不受影响。窗口内一律保留，供排查。
const ORPHAN_WORKTREE_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// 启动时回收 worktree：先让 git 清掉失效登记，再删除项目目录下未被 git 登记的
/// 残留目录；登记仍在但 run 行已消失的孤儿按保留窗口回收。
fn cleanup_worktrees(db: &Db, data_dir: &Path) {
    let projects = match db.list_projects() {
        Ok(projects) => projects,
        Err(err) => {
            eprintln!("读取项目列表失败，跳过 worktree 清理：{err}");
            return;
        }
    };
    for project in projects {
        let repo = Path::new(&project.repo_path);
        if let Err(err) = gitops::prune_worktrees(repo) {
            eprintln!("清理项目 {} 的 worktree 登记失败：{err}", project.id);
        }
        let registered = match gitops::list_worktrees(repo) {
            Ok(paths) => paths
                .into_iter()
                .map(|path| path.canonicalize().unwrap_or(path))
                .collect::<HashSet<_>>(),
            Err(err) => {
                eprintln!("列出项目 {} 的 worktree 失败：{err}", project.id);
                continue;
            }
        };
        let root = data_dir.join("worktrees").join(&project.id);
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            // 项目还没有产生过 worktree。
            Err(_) => continue,
        };
        let mut reclaimed = false;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
            if registered.contains(&canonical) {
                if reclaim_orphan_worktree(
                    db.conn(),
                    &path,
                    SystemTime::now() - ORPHAN_WORKTREE_RETENTION,
                ) {
                    reclaimed = true;
                }
                continue;
            }
            if let Err(err) = std::fs::remove_dir_all(&path) {
                eprintln!("删除残留 worktree {} 失败：{err}", path.display());
            }
        }
        if reclaimed {
            if let Err(err) = gitops::prune_worktrees(repo) {
                eprintln!("回收孤儿 worktree 后清理登记失败：{err}");
            }
        }
    }
}

/// 回收孤儿 worktree：git 登记仍在，但对应 `runs` 行已被清理（任务行被手工
/// 删除时不会自动移除 worktree）。修改时间早于 `cutoff` 才回收，返回是否删除。
fn reclaim_orphan_worktree(conn: &rusqlite::Connection, path: &Path, cutoff: SystemTime) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let has_run: i64 = match conn.query_row(
        "SELECT COUNT(*) FROM runs WHERE id = ?1",
        params![name],
        |row| row.get(0),
    ) {
        Ok(count) => count,
        Err(err) => {
            eprintln!("查询 run {name} 失败，保留 worktree：{err}");
            return false;
        }
    };
    if has_run > 0 {
        return false;
    }
    let modified = match std::fs::metadata(path).and_then(|meta| meta.modified()) {
        Ok(modified) => modified,
        Err(_) => return false,
    };
    if modified >= cutoff {
        return false;
    }
    if let Err(err) = std::fs::remove_dir_all(path) {
        eprintln!("回收孤儿 worktree {} 失败：{err}", path.display());
        return false;
    }
    true
}

/// 显示并聚焦主窗口；窗口尚未创建时静默跳过。
pub(crate) fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// 创建菜单栏托盘图标：左键显示并聚焦主窗口，右键菜单提供“显示 / 退出”。
///
/// 关闭主窗口只会隐藏，因此退出应用的入口始终保留在托盘菜单里。
fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItemBuilder::with_id("tray-show", "显示 temli").build(app)?;
    let quit = MenuItemBuilder::with_id("tray-quit", "退出 temli").build(app)?;
    let menu = MenuBuilder::new(app).items(&[&show, &quit]).build()?;

    let mut builder = TrayIconBuilder::with_id("temli")
        .tooltip("temli")
        .menu(&menu)
        // 左键留给“显示主窗口”，菜单通过右键打开。
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "tray-show" => show_main_window(app),
            "tray-quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    // macOS 菜单栏用模板图标（黑色 + alpha），由系统随浅色/深色外观反色；
    // 其它平台沿用窗口图标。模板缺失时退回窗口图标。
    let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray-template.png"))
        .ok()
        .or_else(|| app.default_window_icon().cloned());
    if let Some(icon) = tray_icon {
        builder = builder.icon(icon).icon_as_template(true);
    }
    builder.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            notify::init(app.handle().clone());
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db = Db::open(&dir.join("temli.db")).map_err(std::io::Error::other)?;
            if let Ok(guard) = db.conn().execute_batch("SELECT 1") {
                let _ = guard;
            }
            let _ = store::interrupt_running(db.conn());
            cleanup_worktrees(&db, &dir);

            let mut adapters = AdapterRegistry::new();
            adapters.register(Arc::new(agent::omp::OmpAdapter::new()));
            adapters.register(Arc::new(agent::fake::FakeAdapter::new()));
            adapters.register(Arc::new(agent::codex::CodexAdapter::new()));
            adapters.register(Arc::new(agent::claude::ClaudeAdapter::new()));

            let default_agent = std::env::var("TEMLI_AGENT").unwrap_or_else(|_| "omp".to_string());
            if adapters.get(&default_agent).is_none() {
                return Err(std::io::Error::other(format!("适配器未注册：{default_agent}")).into());
            }
            let adapters = Arc::new(adapters);

            let db = Arc::new(Mutex::new(db));
            let github: Arc<dyn github::GitHubOps> = Arc::new(github::GhOps::new());
            let scheduler = Scheduler::new(orchestrator::RunContext {
                db: Arc::clone(&db),
                adapters: Arc::clone(&adapters),
                default_agent: default_agent.clone(),
                data_dir: dir,
                running: Arc::new(Mutex::new(HashSet::new())),
                cancels: orchestrator::CancelRegistry::new(),
                merge_lock: Arc::new(Mutex::new(())),
                cancelled: Arc::new(Mutex::new(HashSet::new())),
                github: Some(Arc::clone(&github)),
            });
            scheduler.start().map_err(std::io::Error::other)?;

            let sync = sync::SyncEngine::new(Arc::clone(&db), github);
            sync.start().map_err(std::io::Error::other)?;

            app.manage(AppState {
                db,
                adapters,
                default_agent,
                scheduler,
                sync,
            });

            setup_tray(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // 关闭主窗口只隐藏到托盘，调度器与后台同步继续运行。
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_projects,
            register_project,
            list_tasks,
            add_dependency,
            remove_dependency,
            list_dependencies,
            get_project_concurrency,
            set_project_concurrency,
            create_task,
            create_task_from_issue,
            plan_project,
            create_tasks_from_plan,
            get_task,
            start_task,
            cancel_task,
            list_decisions,
            list_reviews,
            resolve_decision,
            list_events,
            list_adapters,
            sync_now,
            update_issue,
            publish_task,
            get_project_issues,
            get_auto_merge,
            set_auto_merge,
            get_require_checks,
            set_require_checks,
            get_auto_docs,
            set_auto_docs,
            set_task_agents,
            set_task_model,
            add_vault,
            list_vaults,
            remove_vault,
            index_vault,
            vault_tree,
            get_vault_note,
            create_vault_note,
            save_vault_note,
            delete_vault_note,
            search_vault,
            note_backlinks,
            vault_graph,
            list_docs,
            get_doc,
            reindex_workspace,
            search_workspace,
            global_search,
            link_context,
            unlink_context,
            list_context
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(on_run_event);
}

/// macOS：窗口全部隐藏后点击 Dock 图标重新显示主窗口。
#[cfg(target_os = "macos")]
fn on_run_event(app: &tauri::AppHandle, event: tauri::RunEvent) {
    if let tauri::RunEvent::Reopen {
        has_visible_windows,
        ..
    } = event
    {
        if !has_visible_windows {
            show_main_window(app);
        }
    }
}

/// 其他平台没有 Reopen 事件，无需额外处理。
#[cfg(not(target_os = "macos"))]
fn on_run_event(_app: &tauri::AppHandle, _event: tauri::RunEvent) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{
        AgentRole, AuthState, Capabilities, DetectResult, RunStatus, SessionHandle,
    };
    use rusqlite::params;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use uuid::Uuid;

    /// 脚本化适配器：`start` 同步发出一条 `TextDelta` 与 `Ended(Completed)`，
    /// 不启动子进程、不调用模型；`starts` 记录被启动的次数。
    struct ScriptedAdapter {
        output: String,
        starts: AtomicUsize,
    }

    impl ScriptedAdapter {
        fn new(output: impl Into<String>) -> Self {
            Self {
                output: output.into(),
                starts: AtomicUsize::new(0),
            }
        }

        fn starts(&self) -> usize {
            self.starts.load(Ordering::SeqCst)
        }
    }

    impl AgentAdapter for ScriptedAdapter {
        fn manifest(&self) -> AgentManifest {
            AgentManifest {
                id: "scripted".to_string(),
                name: "Scripted".to_string(),
                version: "0.0.0".to_string(),
                roles: vec![AgentRole::Developer, AgentRole::Reviewer],
                capabilities: Capabilities::default(),
            }
        }

        fn detect(&self) -> DetectResult {
            DetectResult {
                available: true,
                version: None,
                auth: AuthState::Ok,
                detail: None,
            }
        }

        fn start(&self, _spec: RunSpec, sink: EventSink) -> Result<SessionHandle, String> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            sink(AgentEvent::TextDelta {
                text: self.output.clone(),
            });
            sink(AgentEvent::Ended {
                status: RunStatus::Completed,
            });
            Ok(SessionHandle {
                id: "scripted-session".to_string(),
            })
        }

        fn respond(
            &self,
            _handle: &SessionHandle,
            _request_id: &str,
            _decision: Decision,
        ) -> Result<(), String> {
            Ok(())
        }

        fn cancel(&self, _handle: &SessionHandle) -> Result<(), String> {
            Ok(())
        }
    }

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// 临时仓库夹具：`package.json` 提供 `test`/`build` 脚本。
    /// `discover_commands` 只读取声明文件，`git init` 仅用于让夹具成为真实仓库。
    struct RepoFixture {
        path: PathBuf,
    }

    impl RepoFixture {
        fn new(label: &str) -> Self {
            let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "temli-planner-{}-{unique}-{label}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("创建临时仓库目录");
            fs::write(
                path.join("package.json"),
                r#"{"scripts":{"test":"node --test","build":"tsc"}}"#,
            )
            .expect("写入 package.json");
            let _ = std::process::Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(&path)
                .status();
            Self { path }
        }

        fn commands(&self) -> Vec<DiscoveredCommand> {
            evidence::discover_commands(&self.path)
        }
    }

    impl Drop for RepoFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn criteria_json(cmd: &str) -> String {
        format!(r#"[{{"id":"c-1","cmd":"{cmd}","expect_exit":0}}]"#)
    }

    #[test]
    fn update_issue_input_matches_frontend_contract() {
        // 字段名与 `src/lib/api.ts` 的 `UpdateIssueInput` 一一对应。
        let input: UpdateIssueInput = serde_json::from_value(serde_json::json!({
            "item_id": "i-1",
            "title": "标题",
            "body": "正文",
            "assignee": "octocat",
            "state": "closed",
        }))
        .unwrap();
        assert_eq!(input.item_id, "i-1");
        assert_eq!(input.title.as_deref(), Some("标题"));
        assert_eq!(input.body.as_deref(), Some("正文"));
        assert_eq!(input.assignee.as_deref(), Some("octocat"));
        assert_eq!(input.state.as_deref(), Some("closed"));

        // 省略的可选字段为 None，表示远端保持不变。
        let input: UpdateIssueInput = serde_json::from_value(serde_json::json!({
            "item_id": "i-1",
            "state": "open",
        }))
        .unwrap();
        assert!(input.title.is_none());
        assert!(input.body.is_none());
        assert!(input.assignee.is_none());
        assert_eq!(input.state.as_deref(), Some("open"));
    }

    #[test]
    fn plan_acceptance_returns_grounded_criteria() {
        let repo = RepoFixture::new("grounded");
        let commands = repo.commands();
        assert_eq!(
            commands
                .iter()
                .map(|command| command.cmd.as_str())
                .collect::<Vec<_>>(),
            vec!["npm run test", "npm run build"]
        );
        // 发现到的 npm 命令形如 `npm run <script>`，规划器输出必须逐字匹配。
        let test_cmd = commands
            .iter()
            .find(|command| command.kind == evidence::CmdKind::Test)
            .expect("夹具应发现 test 命令")
            .cmd
            .clone();
        let adapter = ScriptedAdapter::new(criteria_json(&test_cmd));

        let criteria = plan_acceptance(
            &adapter,
            &repo.path,
            "新增解析器",
            "支持输入解析",
            &commands,
        )
        .expect("与发现命令一致的计划应被接受");

        assert_eq!(criteria.len(), 1);
        assert_eq!(criteria[0].id, "c-1");
        assert_eq!(criteria[0].cmd, test_cmd);
        assert_eq!(criteria[0].expect_exit, 0);
        assert_eq!(adapter.starts(), 1);
    }

    #[test]
    fn plan_acceptance_rejects_unparseable_output() {
        let repo = RepoFixture::new("prose");
        let commands = repo.commands();
        let adapter = ScriptedAdapter::new("没有 JSON，直接运行测试即可。");

        assert!(
            plan_acceptance(&adapter, &repo.path, "t", "i", &commands).is_none(),
            "纯文本输出必须回退（调用方使用默认验收标准）"
        );
        assert_eq!(adapter.starts(), 1);
    }

    #[test]
    fn plan_acceptance_rejects_undiscovered_command() {
        let repo = RepoFixture::new("ungrounded");
        let commands = repo.commands();
        let adapter = ScriptedAdapter::new(r#"[{"id":"c-1","cmd":"rm -rf /","expect_exit":0}]"#);

        assert!(
            plan_acceptance(&adapter, &repo.path, "t", "i", &commands).is_none(),
            "未在发现列表中的命令必须被拒绝"
        );
        assert_eq!(adapter.starts(), 1);
    }

    #[test]
    fn plan_acceptance_skips_adapter_without_commands() {
        let adapter = ScriptedAdapter::new(criteria_json("npm run test"));

        assert!(plan_acceptance(&adapter, Path::new("."), "t", "i", &[]).is_none());
        assert_eq!(adapter.starts(), 0, "命令列表为空时不应启动规划器");
    }

    #[test]
    fn acceptance_generation_falls_back_to_defaults() {
        let repo = RepoFixture::new("fallback");
        let commands = repo.commands();
        let defaults = evidence::default_acceptance(&commands);
        assert!(!defaults.is_empty(), "夹具应产生非空默认验收标准");
        assert_eq!(defaults.len(), commands.len());

        // 规划器输出无法解析：`generate_acceptance` 与 `create_task` 走同一路径，
        // 必须回退到默认验收标准，而不是返回空列表。
        let broken = ScriptedAdapter::new("no json here");
        let broken_adapter: &dyn AgentAdapter = &broken;
        let generated = generate_acceptance(Some(broken_adapter), &repo.path, "t", "i", &commands);
        assert_eq!(generated, defaults);
        assert_eq!(broken.starts(), 1);
        assert_eq!(
            generated
                .iter()
                .map(|criterion| criterion.cmd.as_str())
                .collect::<Vec<_>>(),
            vec!["npm run test", "npm run build"]
        );

        // 适配器未注册时同样回退。
        assert_eq!(
            generate_acceptance(None, &repo.path, "t", "i", &commands),
            defaults
        );

        // 规划成功时使用规划器结果，而非默认标准。
        let grounded = ScriptedAdapter::new(criteria_json(&commands[0].cmd));
        let grounded_adapter: &dyn AgentAdapter = &grounded;
        let planned = generate_acceptance(Some(grounded_adapter), &repo.path, "t", "i", &commands);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].cmd, commands[0].cmd);
        assert_ne!(planned, defaults);
    }

    /// 清理用例夹具：真实 git 仓库 + 独立数据目录 + 已打开数据库，随 Drop 清理。
    struct WorktreeFixture {
        root: PathBuf,
        repo: PathBuf,
        data_dir: PathBuf,
        db: Db,
    }

    fn git(repo: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .expect("执行 git");
        assert!(status.success(), "git {:?} 失败", args);
    }

    impl WorktreeFixture {
        fn new(label: &str) -> Self {
            let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "temli-cleanup-{}-{unique}-{label}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            let repo = root.join("repo");
            fs::create_dir_all(&repo).expect("创建临时仓库目录");
            git(&repo, &["init", "-q", "-b", "main"]);
            fs::write(repo.join(".gitignore"), ".temli/\n").expect("写入 .gitignore");
            fs::write(repo.join("README.md"), "demo\n").expect("写入 README");
            git(&repo, &["add", "-A"]);
            git(
                &repo,
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "-q",
                    "-m",
                    "init",
                ],
            );
            let data_dir = root.join("data");
            fs::create_dir_all(&data_dir).expect("创建数据目录");
            let db = Db::open(&root.join("temli.db")).expect("打开临时数据库");
            Self {
                root,
                repo,
                data_dir,
                db,
            }
        }
    }

    impl Drop for WorktreeFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// 在夹具仓库登记项目；`with_commands` 时写入 `package.json` 供命令发现。
    fn repo_project(fixture: &WorktreeFixture, with_commands: bool) -> Project {
        if with_commands {
            fs::write(
                fixture.repo.join("package.json"),
                r#"{"scripts":{"test":"node --test","build":"tsc"}}"#,
            )
            .expect("写入 package.json");
        }
        let repo = fixture.repo.canonicalize().expect("规范化仓库路径");
        fixture
            .db
            .insert_project(
                RegisterProjectInput {
                    repo_path: repo.to_string_lossy().into_owned(),
                    name: Some("demo".to_string()),
                    github_owner: None,
                    github_repo: None,
                    default_branch: Some("main".to_string()),
                },
                repo,
            )
            .expect("登记项目")
    }

    fn insert_issue_item(
        conn: &rusqlite::Connection,
        project_id: &str,
        kind: &str,
        title: &str,
        body: &str,
        github_number: Option<i64>,
    ) -> String {
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO items (id, project_id, kind, title, body, status, github_number)
             VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6)",
            params![id, project_id, kind, title, body, github_number],
        )
        .expect("写入条目");
        id
    }

    fn item_row(
        conn: &rusqlite::Connection,
        item_id: &str,
    ) -> (String, String, Option<i64>, String) {
        conn.query_row(
            "SELECT status, title, github_number, body FROM items WHERE id = ?1",
            params![item_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("读取条目")
    }

    #[test]
    fn convert_issue_creates_draft_task_and_is_idempotent() {
        let fixture = WorktreeFixture::new("issue-task");
        let project = repo_project(&fixture, true);
        let conn = fixture.db.conn();
        let item_id =
            insert_issue_item(conn, &project.id, "issue", "远端问题", "复现步骤", Some(42));

        let detail = convert_issue_to_task(
            conn,
            CreateTaskFromIssueInput {
                item_id: item_id.clone(),
                title: None,
                prompt: None,
            },
        )
        .expect("同步 issue 应能转换为任务");

        assert_eq!(detail.task.status, store::TaskStatus::Draft);
        assert_eq!(detail.task.title, "远端问题");
        assert_eq!(detail.task.prompt, "复现步骤");
        assert_eq!(detail.task.project_id, project.id);
        assert_eq!(detail.criteria.len(), 2);
        assert_eq!(detail.criteria[0].cmd, "npm run test");

        let (status, title, github_number, body) = item_row(conn, &item_id);
        assert_eq!(status, "draft");
        assert_eq!(title, "远端问题");
        assert_eq!(github_number, Some(42));
        assert_eq!(body, "复现步骤");

        // 幂等：第二次调用返回同一任务，不再插入 tasks 行，也不应用标题覆盖。
        let again = convert_issue_to_task(
            conn,
            CreateTaskFromIssueInput {
                item_id: item_id.clone(),
                title: Some("另一个标题".to_string()),
                prompt: Some("另一段提示".to_string()),
            },
        )
        .expect("重复转换应返回已有任务");
        assert_eq!(again.task.id, detail.task.id);
        let tasks: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tasks, 1);
        assert_eq!(item_row(conn, &item_id).1, "远端问题");
    }

    #[test]
    fn convert_issue_applies_title_override_and_default_prompt() {
        let fixture = WorktreeFixture::new("issue-override");
        let project = repo_project(&fixture, true);
        let conn = fixture.db.conn();

        let no_body = insert_issue_item(conn, &project.id, "issue", "旧标题", "", Some(7));
        let detail = convert_issue_to_task(
            conn,
            CreateTaskFromIssueInput {
                item_id: no_body.clone(),
                title: Some("  新标题  ".to_string()),
                prompt: None,
            },
        )
        .expect("标题覆盖应成功");
        assert_eq!(detail.task.title, "新标题");
        assert_eq!(detail.task.prompt, "新标题", "无正文时提示回退到标题");
        assert_eq!(item_row(conn, &no_body).1, "新标题");

        let with_body = insert_issue_item(
            conn,
            &project.id,
            "issue",
            "另一个问题",
            "正文内容",
            Some(8),
        );
        let detail = convert_issue_to_task(
            conn,
            CreateTaskFromIssueInput {
                item_id: with_body,
                title: None,
                prompt: Some("  自定义提示  ".to_string()),
            },
        )
        .expect("显式提示应成功");
        assert_eq!(detail.task.prompt, "自定义提示");
    }

    #[test]
    fn convert_issue_rejects_non_issue_and_repos_without_commands() {
        let fixture = WorktreeFixture::new("issue-reject");
        let project = repo_project(&fixture, false);
        let conn = fixture.db.conn();

        let note = insert_issue_item(conn, &project.id, "note", "笔记", "正文", None);
        let err = convert_issue_to_task(
            conn,
            CreateTaskFromIssueInput {
                item_id: note,
                title: None,
                prompt: None,
            },
        )
        .unwrap_err();
        assert!(err.contains("issue"), "{err}");

        let issue = insert_issue_item(conn, &project.id, "issue", "没有命令", "正文", None);
        let err = convert_issue_to_task(
            conn,
            CreateTaskFromIssueInput {
                item_id: issue.clone(),
                title: None,
                prompt: None,
            },
        )
        .unwrap_err();
        assert!(err.contains("未发现可执行的测试/构建命令"), "{err}");

        let tasks: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tasks, 0);
        assert_eq!(item_row(conn, &issue).0, "open");
    }

    #[test]
    fn cleanup_worktrees_removes_stale_registration_and_orphans() {
        let fixture = WorktreeFixture::new("orphan");
        let repo = fixture.repo.canonicalize().expect("规范化仓库路径");
        let project = fixture
            .db
            .insert_project(
                RegisterProjectInput {
                    repo_path: repo.to_string_lossy().into_owned(),
                    name: Some("demo".to_string()),
                    github_owner: None,
                    github_repo: None,
                    default_branch: Some("main".to_string()),
                },
                repo.clone(),
            )
            .expect("登记项目");

        let base = gitops::rev_parse(&repo, "main").expect("读取初始提交");
        let project_root = fixture.data_dir.join("worktrees").join(&project.id);
        fs::create_dir_all(&project_root).expect("创建 worktree 根目录");
        let worktree = project_root.join("run-1");
        gitops::create_worktree(&repo, &worktree, "temli/cleanup-run-1", &base)
            .expect("创建 worktree");
        assert!(
            worktree.join("README.md").exists(),
            "worktree 应包含仓库内容"
        );

        // 模拟崩溃后残留：worktree 目录被手动删除，git 仍保留登记。
        fs::remove_dir_all(&worktree).expect("删除 worktree 目录");
        // 另一个从未登记过的孤儿目录（例如数据库被重置后遗留）。
        let orphan = project_root.join("run-orphan");
        fs::create_dir_all(&orphan).expect("创建孤儿目录");
        fs::write(orphan.join("leftover.txt"), "stale\n").expect("写入残留文件");

        cleanup_worktrees(&fixture.db, &fixture.data_dir);

        assert!(!orphan.exists(), "未登记的孤儿目录应被删除");
        assert!(!worktree.exists(), "残留 worktree 目录应被删除");
        let registered = gitops::list_worktrees(&repo).expect("列出 worktree");
        assert_eq!(
            registered.len(),
            1,
            "清理后只应保留主 worktree：{registered:?}"
        );
        assert!(registered.iter().all(|path| path != &worktree));
    }

    #[test]
    fn cleanup_worktrees_keeps_fresh_orphans_and_reclaims_expired_ones() {
        let fixture = WorktreeFixture::new("orphan-window");
        let repo = fixture.repo.canonicalize().expect("规范化仓库路径");
        let project = fixture
            .db
            .insert_project(
                RegisterProjectInput {
                    repo_path: repo.to_string_lossy().into_owned(),
                    name: Some("demo".to_string()),
                    github_owner: None,
                    github_repo: None,
                    default_branch: Some("main".to_string()),
                },
                repo.clone(),
            )
            .expect("登记项目");

        let base = gitops::rev_parse(&repo, "main").expect("读取初始提交");
        let project_root = fixture.data_dir.join("worktrees").join(&project.id);
        fs::create_dir_all(&project_root).expect("创建 worktree 根目录");
        // 登记仍在、但 run 行已被清理的孤儿（手工 DB 清理的产物）。
        let orphan = project_root.join("run-dead");
        gitops::create_worktree(&repo, &orphan, "temli/cleanup-run-dead", &base)
            .expect("创建孤儿 worktree");
        // run 行仍在的登记 worktree：失败现场按 PRD 保留调试。
        let conn = fixture.db.conn();
        conn.execute(
            "INSERT INTO items (id, project_id, kind, title, status, created_at)
             VALUES ('item-alive', ?1, 'issue', 'alive', 'queued',
                     strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
            rusqlite::params![project.id],
        )
        .expect("插入 item");
        conn.execute(
            "INSERT INTO tasks (id, item_id, acceptance_json, prompt, attempts)
             VALUES ('task-alive', 'item-alive', '[]', 'p', 0)",
            [],
        )
        .expect("插入 task");
        conn.execute(
            "INSERT INTO runs (id, task_id, attempt, adapter_id) VALUES ('run-alive', 'task-alive', 1, 'fake')",
            [],
        )
        .expect("插入 run");
        let alive = project_root.join("run-alive");
        gitops::create_worktree(&repo, &alive, "temli/cleanup-run-alive", &base)
            .expect("创建在跑 run 的 worktree");

        // 保留窗口内：孤儿新鲜，不回收；run 行仍在的更不回收。
        cleanup_worktrees(&fixture.db, &fixture.data_dir);
        assert!(orphan.exists(), "窗口内的孤儿 worktree 应保留调试");
        assert!(alive.exists(), "run 行仍在的 worktree 应保留");

        // 超过保留窗口：孤儿回收并清掉登记；run 行仍在的原样保留。
        let cutoff = std::time::SystemTime::now() + Duration::from_secs(3600);
        assert!(
            reclaim_orphan_worktree(fixture.db.conn(), &orphan, cutoff),
            "超窗孤儿应被回收"
        );
        assert!(!orphan.exists(), "超窗孤儿目录应被删除");
        assert!(
            !reclaim_orphan_worktree(fixture.db.conn(), &alive, cutoff),
            "run 行仍在的 worktree 不应回收"
        );
        assert!(alive.exists(), "run 行仍在的 worktree 目录应保留");
        gitops::prune_worktrees(&repo).expect("清理登记");
        let registered = gitops::list_worktrees(&repo).expect("列出 worktree");
        assert!(registered.iter().all(|path| path != &orphan));
    }

    fn planned_issue(title: &str, cmd: &str, depends_on: Vec<usize>) -> planner::PlanIssue {
        planner::PlanIssue {
            title: title.to_string(),
            prompt: format!("实现{title}并补充测试"),
            acceptance: vec![store::Criterion {
                id: "c-1".to_string(),
                cmd: cmd.to_string(),
                expect_exit: 0,
                expect_match: None,
            }],
            depends_on,
        }
    }

    fn plan_issue_json(title: &str, cmd: &str, depends_on: Vec<usize>) -> serde_json::Value {
        serde_json::json!({
            "title": title,
            "prompt": format!("实现{title}并补充测试"),
            "acceptance": [{
                "id": "c-1",
                "cmd": cmd,
                "expect_exit": 0,
                "expect_match": null
            }],
            "depends_on": depends_on
        })
    }

    #[test]
    fn plan_pipeline_returns_strictly_validated_draft() {
        let fixture = WorktreeFixture::new("plan-run");
        let project = repo_project(&fixture, true);
        let commands = evidence::discover_commands(&fixture.repo);
        let test_cmd = commands
            .iter()
            .find(|command| command.kind == evidence::CmdKind::Test)
            .expect("夹具应发现 test 命令")
            .cmd
            .clone();
        let build_cmd = commands
            .iter()
            .find(|command| command.kind == evidence::CmdKind::Build)
            .expect("夹具应发现 build 命令")
            .cmd
            .clone();
        let output = serde_json::json!({
            "issues": [
                plan_issue_json("解析输入", &test_cmd, vec![]),
                plan_issue_json("接入界面", &build_cmd, vec![0])
            ]
        })
        .to_string();
        let adapter = ScriptedAdapter::new(output);

        let draft =
            run_plan(&adapter, &project, "实现搜索面板", &commands).expect("合法输出应生成草案");

        assert_eq!(adapter.starts(), 1);
        assert_eq!(draft.issues.len(), 2);
        assert_eq!(draft.issues[0].title, "解析输入");
        assert_eq!(draft.issues[1].depends_on, vec![0]);
    }

    #[test]
    fn plan_pipeline_rejects_invalid_agent_output() {
        let fixture = WorktreeFixture::new("plan-invalid");
        let project = repo_project(&fixture, true);
        let commands = evidence::discover_commands(&fixture.repo);
        let adapter = ScriptedAdapter::new("这不是 JSON");

        let err = run_plan(&adapter, &project, "想法", &commands).unwrap_err();

        assert!(err.contains("无法通过校验"), "{err}");
        assert_eq!(adapter.starts(), 1);
    }

    #[test]
    fn create_tasks_from_plan_creates_draft_tasks_with_blocks_edges() {
        let fixture = WorktreeFixture::new("plan-batch");
        let project = repo_project(&fixture, true);
        let commands = evidence::discover_commands(&fixture.repo);
        let test_cmd = commands
            .iter()
            .find(|command| command.kind == evidence::CmdKind::Test)
            .expect("夹具应发现 test 命令")
            .cmd
            .clone();
        let build_cmd = commands
            .iter()
            .find(|command| command.kind == evidence::CmdKind::Build)
            .expect("夹具应发现 build 命令")
            .cmd
            .clone();
        let raw = serde_json::json!({
            "issues": [
                plan_issue_json("解析输入", &test_cmd, vec![]),
                plan_issue_json("接入界面", &build_cmd, vec![0]),
                plan_issue_json("发布", &test_cmd, vec![0, 1])
            ]
        })
        .to_string();
        let draft = planner::parse_plan(&raw, &commands).expect("草案应通过校验");
        let conn = fixture.db.conn();

        let details =
            create_plan_tasks(conn, &project.id, &fixture.repo, draft).expect("确认计划应创建任务");

        assert_eq!(details.len(), 3);
        for detail in &details {
            assert_eq!(detail.task.status, store::TaskStatus::Draft);
            assert_eq!(detail.task.project_id, project.id);
            assert_eq!(detail.criteria.len(), 1);
        }
        assert_eq!(details[0].criteria[0].cmd, test_cmd);
        assert_eq!(details[1].criteria[0].cmd, build_cmd);

        let (blockers, blocks) =
            store::task_dependencies(conn, &details[1].task.id).expect("读取依赖");
        assert_eq!(
            blockers
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            vec![details[0].task.id.as_str()]
        );
        assert_eq!(
            blocks
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            vec![details[2].task.id.as_str()],
            "第二个任务应阻塞第三个任务"
        );

        let (blockers, blocks) =
            store::task_dependencies(conn, &details[2].task.id).expect("读取依赖");
        let mut blocker_ids = blockers
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>();
        blocker_ids.sort_unstable();
        let mut expected = vec![details[0].task.id.as_str(), details[1].task.id.as_str()];
        expected.sort_unstable();
        assert_eq!(blocker_ids, expected);
        assert!(blocks.is_empty());

        let (blockers, blocks) =
            store::task_dependencies(conn, &details[0].task.id).expect("读取依赖");
        assert!(blockers.is_empty());
        assert_eq!(blocks.len(), 2, "首个任务应阻塞后两个任务");

        let task_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
            .unwrap();
        let link_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM links WHERE rel = 'blocks'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(task_count, 3);
        assert_eq!(link_count, 3);
    }

    #[test]
    fn create_tasks_from_plan_rejects_invalid_plan_before_writing() {
        let fixture = WorktreeFixture::new("plan-reject");
        let project = repo_project(&fixture, true);
        let conn = fixture.db.conn();

        let unknown_command = planner::PlanDraft {
            issues: vec![planned_issue("危险任务", "rm -rf /", vec![])],
        };
        let err = create_plan_tasks(conn, &project.id, &fixture.repo, unknown_command).unwrap_err();
        assert!(err.contains("rm -rf /"), "{err}");

        let forward_reference = planner::PlanDraft {
            issues: vec![
                planned_issue("第一个", "npm run test", vec![]),
                planned_issue("第二个", "npm run build", vec![1]),
            ],
        };
        let err =
            create_plan_tasks(conn, &project.id, &fixture.repo, forward_reference).unwrap_err();
        assert!(err.contains("depends_on"), "{err}");

        let task_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
            .unwrap();
        let item_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(task_count, 0, "校验失败不得写入任务");
        assert_eq!(item_count, 0, "校验失败不得写入条目");
    }

    #[test]
    fn create_tasks_from_draft_rolls_back_on_failure() {
        let fixture = WorktreeFixture::new("plan-rollback");
        let project = repo_project(&fixture, true);
        let conn = fixture.db.conn();

        // 绕过 planner 校验注入一个 store 会拒绝的验收 id：第二个任务失败时，
        // 第一个任务必须被回滚删除。
        let mut broken = planned_issue("第二个", "npm run build", vec![]);
        broken.acceptance[0].id = "bad id".to_string();
        let draft = planner::PlanDraft {
            issues: vec![planned_issue("第一个", "npm run test", vec![]), broken],
        };
        let err = create_tasks_from_draft(conn, &project.id, draft).unwrap_err();
        assert!(err.contains("id 非法"), "{err}");
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM items", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );

        // 依赖写入阶段失败（自依赖）时，已写入的任务与 blocks 边同样要回滚。
        let self_dependency = planner::PlanDraft {
            issues: vec![
                planned_issue("第一个", "npm run test", vec![]),
                planned_issue("第二个", "npm run build", vec![0, 1]),
            ],
        };
        let err = create_tasks_from_draft(conn, &project.id, self_dependency).unwrap_err();
        assert!(err.contains("依赖自身"), "{err}");
        let task_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
            .unwrap();
        let item_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
            .unwrap();
        let link_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM links", [], |row| row.get(0))
            .unwrap();
        assert_eq!(task_count, 0);
        assert_eq!(item_count, 0);
        assert_eq!(link_count, 0);
    }
    #[test]
    #[ignore = "requires a live OMP model"]
    fn live_plan_generation_with_omp() {
        use crate::agent::{AgentAdapter, AgentEvent, Decision, EventSink, RunSpec, RunStatus};
        use std::sync::Arc;
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        let fixture = WorktreeFixture::new("plan-live");
        let project = repo_project(&fixture, true);
        let commands = evidence::discover_commands(&fixture.repo);
        assert!(!commands.is_empty(), "夹具应发现可执行命令");

        let adapter = crate::agent::omp::OmpAdapter::new();
        let prompt = planner::plan_prompt(
            "增加两个小功能：为 slugify 补充空输入测试，并在 README 增加使用示例。",
            &project,
            &commands,
        );
        let (tx, rx) = mpsc::channel();
        let sink: EventSink = Arc::new(move |event| {
            let _ = tx.send(event);
        });
        let spec = RunSpec {
            cwd: fixture.repo.clone(),
            prompt,
            model: None,
            max_minutes: 10,
            env_allowlist: Vec::new(),
        };
        let handle = adapter.start(spec, sink).expect("启动 OMP 会话");
        let deadline = Instant::now() + Duration::from_secs(300);
        let mut text = String::new();
        let status = loop {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(AgentEvent::TextDelta { text: delta }) => text.push_str(&delta),
                Ok(AgentEvent::ApprovalRequest { request_id, .. }) => {
                    let _ = adapter.respond(&handle, &request_id, Decision::Deny);
                }
                Ok(AgentEvent::Ended { status }) => break status,
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                    let _ = adapter.cancel(&handle);
                    panic!("OMP 规划超时");
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => panic!("事件流中断"),
            }
        };
        let _ = adapter.close(&handle);
        assert_eq!(status, RunStatus::Completed);
        println!("LIVE PLAN OUTPUT:\n{text}");

        let draft = planner::parse_plan(&text, &commands).expect("真实计划应通过校验");
        assert!(draft.issues.len() >= 2, "应生成多个 Issue，实际: {draft:?}");
        let details =
            create_plan_tasks(fixture.db.conn(), &project.id, &fixture.repo, draft).expect("落库");
        assert!(
            details
                .iter()
                .all(|detail| detail.task.status == store::TaskStatus::Draft)
        );
    }
}
