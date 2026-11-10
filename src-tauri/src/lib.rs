mod agent;
mod commands;
mod db;
mod dev_api;
mod error;
mod evidence;
mod github;
mod gitops;
mod logging;
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

use agent::AdapterRegistry;
use commands::settings::resolve_default_agent;
use db::Db;

pub(crate) use error::{AppError, AppResult};
use rusqlite::params;
use scheduler::Scheduler;
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};
use tauri::{
    Manager, WindowEvent,
    menu::{MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

/// 运行时可变的默认适配器 id；AppState 与调度线程的 RunContext 共享同一份。
pub(crate) type SharedDefaultAgent = Arc<RwLock<String>>;

pub(crate) struct AppState {
    pub(crate) db: Arc<Mutex<Db>>,
    pub(crate) adapters: Arc<AdapterRegistry>,
    /// 未在任务上单独指定适配器时使用的默认适配器 id。
    ///
    /// 与调度线程共享；设置页保存后立即生效（`TEMLI_AGENT` 存在时保持环境变量优先）。
    pub(crate) default_agent: SharedDefaultAgent,
    /// 启动时的 `app_data_dir` 路径字符串，供设置页展示。
    pub(crate) data_dir: String,
    /// 任务队列的唯一入口；所有执行都在调度线程内串行进行。
    pub(crate) scheduler: Arc<Scheduler>,
    /// GitHub issue 后台同步。
    pub(crate) sync: Arc<sync::SyncEngine>,
}

/// 启动时回收 worktree：先让 git 清掉失效登记，再删除项目目录下未被 git 登记的
/// 残留目录；登记仍在但 run 行已消失的孤儿按 `retention_days` 天的保留窗口回收
/// （窗口内一律保留，供排查；失败/需决策任务的 run 行仍在库，不受影响）。
fn cleanup_worktrees(db: &Db, data_dir: &Path, retention_days: u32) {
    let cutoff = SystemTime::now() - Duration::from_secs(u64::from(retention_days) * 24 * 60 * 60);
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
                if reclaim_orphan_worktree(db.conn(), &path, cutoff) {
                    reclaimed = true;
                }
                continue;
            }
            if let Err(err) = std::fs::remove_dir_all(&path) {
                eprintln!("删除残留 worktree {} 失败：{err}", path.display());
            }
        }
        if reclaimed && let Err(err) = gitops::prune_worktrees(repo) {
            eprintln!("回收孤儿 worktree 后清理登记失败：{err}");
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
        // 单实例守卫必须在其他 plugin 之前注册：第二个进程在 plugin setup
        // 阶段把参数交给首个实例后立即退出。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            logging::log("检测到第二个实例，激活主窗口");
            show_main_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            notify::init(app.handle().clone());
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            logging::init(&dir);
            logging::log(format!(
                "启动 temli {}（数据目录 {}）",
                env!("CARGO_PKG_VERSION"),
                dir.display()
            ));
            logging::log(format!(
                "日志文件：{}",
                logging::path()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "未启用".to_string())
            ));
            let db = Db::open(&dir.join("temli.db")).map_err(std::io::Error::other)?;
            let _ = db.conn().execute_batch("SELECT 1");
            let _ = store::interrupt_running(db.conn());
            let retention_days = store::worktree_retention_days(db.conn());
            cleanup_worktrees(&db, &dir, retention_days);

            let mut adapters = AdapterRegistry::new();
            adapters.register(Arc::new(agent::omp::OmpAdapter::new()));
            adapters.register(Arc::new(agent::fake::FakeAdapter::new()));
            adapters.register(Arc::new(agent::codex::CodexAdapter::new()));
            adapters.register(Arc::new(agent::claude::ClaudeAdapter::new()));

            let env_agent = std::env::var("TEMLI_AGENT").ok();
            let setting_agent = store::get_setting(db.conn(), store::SETTING_DEFAULT_AGENT)
                .ok()
                .flatten();
            let manifests = adapters.manifests();
            let default_agent =
                resolve_default_agent(env_agent.as_deref(), setting_agent.as_deref(), &manifests);
            if adapters.get(&default_agent).is_none() {
                return Err(std::io::Error::other(format!("适配器未注册：{default_agent}")).into());
            }
            logging::log(format!(
                "默认 agent：{default_agent}；worktree 保留：{retention_days} 天"
            ));
            let adapters = Arc::new(adapters);
            let data_dir = dir.to_string_lossy().into_owned();
            let default_agent: SharedDefaultAgent = Arc::new(RwLock::new(default_agent));

            let db = Arc::new(Mutex::new(db));
            let github: Arc<dyn github::GitHubOps> = Arc::new(github::GhOps::new());
            let scheduler = Scheduler::new(orchestrator::RunContext {
                db: Arc::clone(&db),
                adapters: Arc::clone(&adapters),
                default_agent: Arc::clone(&default_agent),
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
                data_dir,
                scheduler,
                sync,
            });

            // 开发者模式默认关闭；开启时自动起本地 API，失败只写日志不阻断启动。
            dev_api::autostart(app.handle());

            setup_tray(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // 关闭主窗口只隐藏到托盘，调度器与后台同步继续运行。
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "main"
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::projects::list_projects,
            commands::projects::register_project,
            commands::tasks::list_tasks,
            commands::projects::add_dependency,
            commands::projects::remove_dependency,
            commands::projects::list_dependencies,
            commands::projects::get_project_concurrency,
            commands::projects::set_project_concurrency,
            commands::settings::get_app_settings,
            commands::settings::set_default_agent,
            commands::settings::set_worktree_retention_days,
            commands::tasks::create_task,
            commands::tasks::create_task_from_issue,
            commands::plans::plan_project,
            commands::plans::create_tasks_from_plan,
            commands::plans::create_plan,
            commands::plans::list_plans,
            commands::plans::get_plan,
            commands::plans::start_plan,
            commands::plans::accept_stage,
            commands::tasks::add_task_attachment,
            commands::tasks::list_task_attachments,
            commands::tasks::read_task_attachment,
            commands::tasks::delete_task_attachment,
            commands::tasks::get_task,
            commands::tasks::start_task,
            commands::tasks::cancel_task,
            commands::tasks::list_decisions,
            commands::tasks::list_reviews,
            commands::tasks::resolve_decision,
            commands::tasks::list_events,
            commands::settings::list_adapters,
            commands::sync::sync_now,
            commands::sync::update_issue,
            commands::sync::publish_task,
            commands::sync::get_project_issues,
            commands::sync::get_auto_merge,
            commands::sync::set_auto_merge,
            commands::sync::get_require_checks,
            commands::sync::set_require_checks,
            commands::sync::get_auto_docs,
            commands::sync::set_auto_docs,
            commands::tasks::set_task_agents,
            commands::tasks::set_task_model,
            commands::vaults::add_vault,
            commands::vaults::list_vaults,
            commands::vaults::remove_vault,
            commands::vaults::index_vault,
            commands::vaults::vault_tree,
            commands::vaults::get_vault_note,
            commands::vaults::create_vault_note,
            commands::vaults::save_vault_note,
            commands::vaults::delete_vault_note,
            commands::vaults::search_vault,
            commands::vaults::note_backlinks,
            commands::vaults::vault_graph,
            commands::vaults::vault_tags,
            commands::vaults::notes_by_tag,
            commands::vaults::save_vault_attachment,
            commands::vaults::read_vault_attachment,
            commands::vaults::list_vault_templates,
            commands::vaults::create_vault_note_with_content,
            commands::vaults::set_note_properties,
            commands::vaults::rename_vault_note,
            commands::workspace::list_docs,
            commands::workspace::get_doc,
            commands::workspace::reindex_workspace,
            commands::workspace::search_workspace,
            commands::workspace::global_search,
            commands::workspace::link_context,
            commands::workspace::unlink_context,
            commands::workspace::list_context,
            dev_api::get_dev_status,
            dev_api::set_dev_mode,
            dev_api::read_app_log
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(on_run_event);
}

/// macOS：窗口全部隐藏后点击 Dock 图标重新显示主窗口；退出时落一行日志。
#[cfg(target_os = "macos")]
fn on_run_event(app: &tauri::AppHandle, event: tauri::RunEvent) {
    match event {
        tauri::RunEvent::Reopen {
            has_visible_windows: false,
            ..
        } => show_main_window(app),
        tauri::RunEvent::Exit => logging::log("应用退出"),
        _ => {}
    }
}

/// 其他平台没有 Reopen 事件，只记录退出。
#[cfg(not(target_os = "macos"))]
fn on_run_event(_app: &tauri::AppHandle, event: tauri::RunEvent) {
    if let tauri::RunEvent::Exit = event {
        logging::log("应用退出");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{
        AgentAdapter, AgentEvent, AgentManifest, AgentRole, AuthState, Capabilities, Decision,
        DetectResult, EventSink, RunSpec, RunStatus, SessionHandle,
    };
    use crate::commands::plans::{
        create_plan_tasks, create_tasks_from_draft, generate_acceptance, plan_acceptance, run_plan,
    };
    use crate::commands::settings::{
        apply_default_agent, resolve_default_agent, runtime_default_agent, validate_retention_days,
    };
    use crate::commands::sync::UpdateIssueInput;
    use crate::commands::tasks::{CreateTaskFromIssueInput, convert_issue_to_task};
    use crate::db::{Project, RegisterProjectInput};
    use crate::evidence::DiscoveredCommand;
    use rusqlite::params;
    use std::fs;
    use std::path::PathBuf;
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

        cleanup_worktrees(&fixture.db, &fixture.data_dir, 7);

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
        cleanup_worktrees(&fixture.db, &fixture.data_dir, 7);
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

    #[test]
    fn cleanup_worktrees_uses_retention_days_parameter() {
        let fixture = WorktreeFixture::new("retention-days");
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
        // 登记仍在、但 run 行已消失的孤儿。
        let orphan = project_root.join("run-retention");
        gitops::create_worktree(&repo, &orphan, "temli/cleanup-retention", &base)
            .expect("创建孤儿 worktree");

        // 默认 7 天窗口：新鲜孤儿保留。
        cleanup_worktrees(&fixture.db, &fixture.data_dir, 7);
        assert!(orphan.exists(), "7 天窗口内的孤儿应保留");

        // 0 天窗口（测试用下界）：同一孤儿立即回收。
        cleanup_worktrees(&fixture.db, &fixture.data_dir, 0);
        assert!(!orphan.exists(), "0 天窗口内的孤儿应立即回收");
    }

    fn manifest(id: &str) -> AgentManifest {
        AgentManifest {
            id: id.to_string(),
            name: id.to_string(),
            version: "0.0.0".to_string(),
            roles: vec![AgentRole::Developer],
            capabilities: Capabilities::default(),
        }
    }

    #[test]
    fn resolve_default_agent_prefers_env_then_setting_then_omp() {
        let manifests = vec![manifest("omp"), manifest("codex"), manifest("fake")];

        // 优先级：环境变量 > 设置 > omp。
        assert_eq!(
            resolve_default_agent(Some("codex"), Some("fake"), &manifests),
            "codex"
        );
        assert_eq!(
            resolve_default_agent(None, Some("fake"), &manifests),
            "fake"
        );
        assert_eq!(resolve_default_agent(None, None, &manifests), "omp");

        // 空白环境变量不覆盖设置。
        assert_eq!(
            resolve_default_agent(Some("  "), Some("fake"), &manifests),
            "fake"
        );

        // 未注册的 id 一律回退 omp；环境变量优先，因此不会落回设置。
        assert_eq!(
            resolve_default_agent(Some("missing"), Some("fake"), &manifests),
            "omp"
        );
        assert_eq!(
            resolve_default_agent(None, Some("missing"), &manifests),
            "omp"
        );
    }

    #[test]
    fn runtime_default_agent_follows_setting_unless_env_pins_it() {
        // 无环境变量：运行值跟随设置；清空设置回落 omp。
        assert_eq!(
            runtime_default_agent(Some("  "), Some("fake")),
            Some("fake".to_string())
        );
        assert_eq!(
            runtime_default_agent(None, Some("fake")),
            Some("fake".to_string())
        );
        assert_eq!(runtime_default_agent(None, None), Some("omp".to_string()));

        // 环境变量存在：设置仍持久化，但不覆盖运行值。
        assert_eq!(runtime_default_agent(Some("codex"), Some("fake")), None);
        assert_eq!(runtime_default_agent(Some("codex"), None), None);
    }

    #[test]
    fn set_default_agent_rejects_unknown_adapter_and_roundtrips() {
        let fixture = WorktreeFixture::new("agent-setting");
        let conn = fixture.db.conn();
        let mut adapters = AdapterRegistry::new();
        adapters.register(Arc::new(ScriptedAdapter::new("ok")));

        // 非法 id 拒绝写入。
        let err = apply_default_agent(conn, &adapters, Some("missing")).unwrap_err();
        assert!(err.contains("missing"), "{err}");
        assert_eq!(
            store::get_setting(conn, store::SETTING_DEFAULT_AGENT).unwrap(),
            None
        );

        // 合法 id 写入；`None` 删除。
        apply_default_agent(conn, &adapters, Some("scripted")).unwrap();
        assert_eq!(
            store::get_setting(conn, store::SETTING_DEFAULT_AGENT)
                .unwrap()
                .as_deref(),
            Some("scripted")
        );
        apply_default_agent(conn, &adapters, None).unwrap();
        assert_eq!(
            store::get_setting(conn, store::SETTING_DEFAULT_AGENT).unwrap(),
            None
        );
    }

    #[test]
    fn retention_days_command_range_is_validated() {
        assert!(validate_retention_days(1).is_ok());
        assert!(validate_retention_days(90).is_ok());
        assert!(validate_retention_days(0).is_err());
        assert!(validate_retention_days(91).is_err());
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
