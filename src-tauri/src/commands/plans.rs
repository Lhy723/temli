//! 规划、计划与阶段相关命令。

use crate::agent::{AgentAdapter, AgentEvent, Decision, EventSink, RunSpec, RunStatus};
use crate::commands::db_err;
use crate::db::Project;
use crate::evidence::{self, DiscoveredCommand};
use crate::logging;
use crate::planner;
use crate::prompts;
use crate::store::{self, Criterion, TaskDetail};
use crate::{AppError, AppResult, AppState};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use tauri::State;

#[derive(Debug, Deserialize)]
pub(crate) struct PlanProjectInput {
    project_id: String,
    idea: String,
    /// 缺省时使用全局默认适配器。
    #[serde(default)]
    agent: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateTasksFromPlanInput {
    project_id: String,
    draft: planner::PlanDraft,
}

/// 规划器单次运行的硬超时（秒）；超时即取消并回退到默认验收标准。
const PLANNER_TIMEOUT_SECS: u64 = 90;

/// 通过 agent 适配器执行一次规划，返回校验通过的验收标准。
///
/// 结果必须非空，且每条标准的 `cmd` 都要与发现列表中的某条命令完全一致；
/// 启动失败、RPC 异常、超时或解析失败一律返回 `None`，绝不阻塞任务创建。
pub(crate) fn plan_acceptance(
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
pub(crate) fn generate_acceptance(
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
pub(crate) fn run_plan(
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
pub(crate) fn plan_project(
    state: State<AppState>,
    input: PlanProjectInput,
) -> AppResult<planner::PlanDraft> {
    let idea = input.idea.trim();
    if idea.is_empty() {
        return Err(AppError::validation("请提供要规划的想法"));
    }

    let project = {
        let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
        db.get_project(&input.project_id)?
            .ok_or_else(|| AppError::not_found("项目不存在"))?
    };

    let default_agent = state
        .default_agent
        .read()
        .map_err(|_| "默认适配器锁被占用".to_string())?
        .clone();
    let requested = input
        .agent
        .as_deref()
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
        .unwrap_or(&default_agent);
    let adapter = state
        .adapters
        .get(requested)
        .ok_or_else(|| AppError::validation(format!("适配器未注册：{requested}")))?;

    let repo = Path::new(&project.repo_path);
    let commands = evidence::discover_commands(repo);
    if commands.is_empty() {
        return Err(AppError::validation(
            "未发现可执行的测试/构建命令，请先配置测试或构建脚本",
        ));
    }

    run_plan(adapter.as_ref(), &project, idea, &commands).map_err(AppError::from)
}

/// 校验草案并把每个任务写入数据库。
///
/// 与 [`plan_project`] 使用同一套 [`planner::validate_plan`] 规则，因此前端回传的
/// 草案在落库前会再次被严格校验（包括命令必须仍存在于发现列表）。
pub(crate) fn create_plan_tasks(
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
pub(crate) fn create_tasks_from_draft(
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
pub(crate) fn create_tasks_from_plan(
    state: State<AppState>,
    input: CreateTasksFromPlanInput,
) -> AppResult<Vec<TaskDetail>> {
    let project = {
        let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
        db.get_project(&input.project_id)?
            .ok_or_else(|| AppError::not_found("项目不存在"))?
    };
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    create_plan_tasks(
        db.conn(),
        &project.id,
        Path::new(&project.repo_path),
        input.draft,
    )
    .map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn create_plan(
    state: State<AppState>,
    project_id: String,
    title: String,
    goal: String,
    stages: Vec<store::NewPlanStage>,
) -> AppResult<store::Plan> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::create_plan(
        db.conn(),
        store::NewPlan {
            project_id,
            title,
            goal,
            stages,
        },
    )
    .map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn list_plans(
    state: State<AppState>,
    project_id: String,
) -> AppResult<Vec<store::Plan>> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::list_plans(db.conn(), &project_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn get_plan(state: State<AppState>, plan_id: String) -> AppResult<store::Plan> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::get_plan(db.conn(), &plan_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn start_plan(state: State<AppState>, plan_id: String) -> AppResult<store::Plan> {
    state.scheduler.start_plan(&plan_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn accept_stage(
    state: State<AppState>,
    stage_id: String,
    note: Option<String>,
) -> AppResult<store::Plan> {
    let plan = {
        let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
        store::accept_stage_with_note(db.conn(), &stage_id, note.as_deref())?
    };
    if let Some(stage) = plan
        .stages
        .iter()
        .find(|stage| stage.status == store::StageStatus::Active)
    {
        for task in &stage.tasks {
            match task.status {
                store::TaskStatus::Draft | store::TaskStatus::Ready => {
                    if let Err(err) = state.scheduler.enqueue(&task.id) {
                        logging::log(format!(
                            "计划 {} 阶段 {} 的任务 {} 入队失败：{err}",
                            plan.id, stage.id, task.id
                        ));
                    }
                }
                store::TaskStatus::Queued
                | store::TaskStatus::Running
                | store::TaskStatus::Verifying
                | store::TaskStatus::Merged => {}
                store::TaskStatus::Failed
                | store::TaskStatus::Interrupted
                | store::TaskStatus::NeedsHuman => {
                    logging::log(format!(
                        "计划 {} 阶段 {} 的任务 {} 处于 {}，不自动重新入队",
                        plan.id, stage.id, task.id, task.status
                    ));
                }
            }
        }
    }
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::get_plan(db.conn(), &plan.id).map_err(AppError::from)
}
