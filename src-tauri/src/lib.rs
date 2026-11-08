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
        let adapter = state.adapters.get("omp");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AuthState, Capabilities, DetectResult, RunStatus, SessionHandle};
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
}
