mod agent;
mod db;
mod evidence;
mod gitops;
mod orchestrator;
mod prompts;
mod store;

use agent::{
    AdapterRegistry, AgentAdapter, AgentEvent, AgentManifest, Decision, EventSink, RunSpec,
};
use db::{Db, Project, RegisterProjectInput, canonical_repo};
use evidence::DiscoveredCommand;
use rusqlite::params;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use store::{Criterion, DecisionRow, EventRow, TaskDetail, TaskSummary};
use tauri::{Manager, State};

struct AppState {
    db: Arc<Mutex<Db>>,
    adapters: Arc<AdapterRegistry>,
    data_dir: PathBuf,
    /// 本进程内已启动、尚未结束的任务 id；`start_task` 的重复启动闸门。
    running: Arc<Mutex<HashSet<String>>>,
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
struct ResolveDecisionInput {
    id: String,
    resolution: String,
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
        run_id: uuid::Uuid::new_v4().to_string(),
        cwd: repo.to_path_buf(),
        prompt,
        model: None,
        max_minutes: 2,
        max_cost_usd: None,
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
        let planned = if commands.is_empty() {
            None
        } else {
            state.adapters.get("omp").and_then(|adapter| {
                plan_acceptance(
                    adapter.as_ref(),
                    &repo,
                    &input.title,
                    &input.prompt,
                    &commands,
                )
            })
        };
        let generated = planned.unwrap_or_else(|| evidence::default_acceptance(&commands));
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

#[tauri::command]
fn get_task(state: State<AppState>, task_id: String) -> Result<TaskDetail, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::get_task(db.conn(), &task_id)
}

/// 校验任务当前状态是否允许启动，并把可恢复状态（failed / interrupted）推回 queued。
fn prepare_start(state: &AppState, task_id: &str) -> Result<(), String> {
    let (status, attempts) = {
        let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
        let detail = store::get_task(db.conn(), task_id)?;
        (detail.task.status, detail.task.attempts)
    };

    match status {
        store::TaskStatus::Draft | store::TaskStatus::Ready | store::TaskStatus::Queued => Ok(()),
        store::TaskStatus::Running | store::TaskStatus::Verifying => {
            Err("任务正在执行".to_string())
        }
        store::TaskStatus::NeedsHuman => Err("请先在决策收件箱处理".to_string()),
        store::TaskStatus::Merged => Err("任务已合并，不能重新执行".to_string()),
        store::TaskStatus::Failed => {
            if attempts >= store::MAX_ATTEMPTS {
                return Err("已达重试上限".to_string());
            }
            let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
            store::transition(db.conn(), task_id, store::TaskStatus::Queued)?;
            Ok(())
        }
        store::TaskStatus::Interrupted => {
            let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
            store::transition(db.conn(), task_id, store::TaskStatus::Queued)?;
            Ok(())
        }
    }
}

fn start_task_inner(state: &AppState, task_id: &str) -> Result<(), String> {
    let adapter_id = std::env::var("TEMLI_AGENT").unwrap_or_else(|_| "omp".to_string());
    let adapter = state
        .adapters
        .get(&adapter_id)
        .ok_or_else(|| format!("适配器未注册：{adapter_id}"))?;
    prepare_start(state, task_id)?;
    orchestrator::spawn_run(
        orchestrator::RunContext {
            db: state.db.clone(),
            adapter,
            data_dir: state.data_dir.clone(),
            running: state.running.clone(),
        },
        task_id.to_string(),
    )
}

#[tauri::command]
fn start_task(state: State<AppState>, task_id: String) -> Result<(), String> {
    {
        let mut running = state
            .running
            .lock()
            .map_err(|_| "运行状态锁被占用".to_string())?;
        if !running.insert(task_id.clone()) {
            return Err("任务已在执行".to_string());
        }
    }
    if let Err(err) = start_task_inner(state.inner(), &task_id) {
        if let Ok(mut running) = state.running.lock() {
            running.remove(&task_id);
        }
        return Err(err);
    }
    Ok(())
}

#[tauri::command]
fn list_decisions(state: State<AppState>, project_id: String) -> Result<Vec<DecisionRow>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    list_project_decisions(db.conn(), &project_id)
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db = Db::open(&dir.join("temli.db")).map_err(std::io::Error::other)?;
            if let Ok(guard) = db.conn().execute_batch("SELECT 1") {
                let _ = guard;
            }
            let _ = store::interrupt_running(db.conn());

            let mut adapters = AdapterRegistry::new();
            adapters.register(Arc::new(agent::omp::OmpAdapter::new()));
            adapters.register(Arc::new(agent::fake::FakeAdapter::new()));

            app.manage(AppState {
                db: Arc::new(Mutex::new(db)),
                adapters: Arc::new(adapters),
                data_dir: dir,
                running: Arc::new(Mutex::new(HashSet::new())),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_projects,
            register_project,
            list_tasks,
            create_task,
            get_task,
            start_task,
            list_decisions,
            resolve_decision,
            list_events,
            list_adapters
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
