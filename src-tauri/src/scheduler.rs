//! 单工作线程任务调度器。
//!
//! `start_task` 只负责把任务压入 `queued`，真正的执行按 FIFO 在唯一的工作线程里
//! 逐个进行，因此同一时刻最多只有一个任务占用工作区。任务结束后由本模块决定
//! 自动重试还是转人工处理。

use crate::orchestrator::{self, RunContext};
use crate::store::{self, NewDecision, NewEvent, TaskStatus};
use std::collections::HashSet;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 队列为空时的轮询间隔。
const IDLE_POLL: Duration = Duration::from_millis(300);

pub struct Scheduler {
    ctx: RunContext,
    /// 已请求取消的已认领任务。
    ///
    /// `RunContext.cancelled` 是给运行器握手阶段读取的共享意图，会被运行的收尾
    /// 守卫清除；调度器在 `settle` 时仍要知道本次运行是否被用户取消，因此单独
    /// 保留一份记录。
    cancel_requested: Mutex<HashSet<String>>,
}

/// 运行占位守卫：无论正常结束还是 panic 展开，都会释放 `RunContext.running` 中的 id。
struct RunningGuard {
    running: Arc<Mutex<HashSet<String>>>,
    task_id: String,
}

impl Drop for RunningGuard {
    fn drop(&mut self) {
        if let Ok(mut running) = self.running.lock() {
            running.remove(&self.task_id);
        }
    }
}

impl Scheduler {
    pub fn new(ctx: RunContext) -> Arc<Self> {
        Arc::new(Self {
            ctx,
            cancel_requested: Mutex::new(HashSet::new()),
        })
    }

    /// 启动唯一的工作线程；只能调用一次。
    ///
    /// 线程创建失败必须上报：否则应用会在队列永远不会被消费的情况下继续运行。
    pub fn start(self: &Arc<Self>) -> Result<(), String> {
        let scheduler = Arc::clone(self);
        std::thread::Builder::new()
            .name("temli-scheduler".to_string())
            .spawn(move || scheduler.work_loop())
            .map(|_| ())
            .map_err(|err| format!("无法启动调度线程：{err}"))
    }

    /// 把任务推入队列：draft → ready → queued；已排队时幂等。
    ///
    /// 仍在执行、已合并或等待人工处理的任务返回错误。
    pub fn enqueue(&self, task_id: &str) -> Result<(), String> {
        let requeued;
        {
            let guard = self
                .ctx
                .db
                .lock()
                .map_err(|_| "数据库锁被占用".to_string())?;
            let conn = guard.conn();
            match store::task_status(conn, task_id)? {
                TaskStatus::Draft => {
                    store::transition(conn, task_id, TaskStatus::Ready)?;
                    store::transition(conn, task_id, TaskStatus::Queued)?;
                    requeued = true;
                }
                TaskStatus::Ready => {
                    store::transition(conn, task_id, TaskStatus::Queued)?;
                    requeued = true;
                }
                // 已在队列中，重复启动保持幂等；保留可能存在的取消意图，
                // 避免用户在任务刚被认领、状态尚未切换时重复点击而吞掉取消。
                TaskStatus::Queued => requeued = false,
                TaskStatus::Failed => {
                    if store::get_task(conn, task_id)?.task.attempts >= store::MAX_ATTEMPTS {
                        return Err("已达重试上限".to_string());
                    }
                    store::transition(conn, task_id, TaskStatus::Queued)?;
                    requeued = true;
                }
                TaskStatus::Interrupted => {
                    store::transition(conn, task_id, TaskStatus::Queued)?;
                    requeued = true;
                }
                TaskStatus::Running | TaskStatus::Verifying => {
                    return Err("任务正在执行".to_string());
                }
                TaskStatus::NeedsHuman => return Err("请先在决策收件箱处理".to_string()),
                TaskStatus::Merged => return Err("任务已合并，不能重新执行".to_string()),
            }
        }
        // 手动重新入队时丢弃上一次的取消标记。
        if requeued {
            self.clear_cancelled(task_id);
        }
        Ok(())
    }

    /// 取消排队中或执行中的任务。
    ///
    /// 先记录取消意图，再决定取消方式：已认领或正在执行的任务只通知适配器
    /// （会话尚未登记也算成功，意图留给运行器在握手阶段读取）；仍在排队的
    /// 任务退回 `draft` 并立即清除意图。
    pub fn cancel(&self, task_id: &str) -> Result<(), String> {
        {
            let mut cancelled = self
                .ctx
                .cancelled
                .lock()
                .map_err(|_| "取消意图锁被占用".to_string())?;
            cancelled.insert(task_id.to_string());
        }
        // 调度器本地记录供 `settle` 使用；共享意图供运行器握手阶段读取。
        {
            let mut requested = self
                .cancel_requested
                .lock()
                .map_err(|_| "调度取消记录锁被占用".to_string())?;
            requested.insert(task_id.to_string());
        }

        let running = (|| -> Result<bool, String> {
            let guard = self
                .ctx
                .db
                .lock()
                .map_err(|_| "数据库锁被占用".to_string())?;
            let status = store::task_status(guard.conn(), task_id)?;
            // 与 `run_one` 的认领顺序一致（先数据库锁、后运行集合），
            // 保证这里读到的状态与认领结果不会撕裂。
            let claimed = self
                .ctx
                .running
                .lock()
                .map_err(|_| "运行状态锁被占用".to_string())?
                .contains(task_id);
            if claimed || status == TaskStatus::Running {
                return Ok(true);
            }
            match status {
                TaskStatus::Queued => {
                    store::transition(guard.conn(), task_id, TaskStatus::Draft)?;
                    Ok(false)
                }
                other => Err(format!("任务当前状态不可取消：{other}")),
            }
        })();

        match running {
            // 已认领 / 运行中：状态保持不变，意图由运行器读取并在收尾时清除。
            Ok(true) => self.ctx.cancels.cancel(task_id).map(|_| ()),
            // 排队中取消成功：已退回 draft，意图不再需要。
            Ok(false) => {
                self.clear_cancelled(task_id);
                Ok(())
            }
            // 状态不可取消：撤回刚写入的意图，避免污染后续入队。
            Err(err) => {
                self.clear_cancelled(task_id);
                Err(err)
            }
        }
    }

    fn work_loop(&self) {
        loop {
            match self.next_queued() {
                Ok(Some(task_id)) => {
                    // 跳过（例如刚被取消）时稍作等待，避免空转。
                    if !self.run_one(&task_id) {
                        std::thread::sleep(IDLE_POLL);
                    }
                }
                Ok(None) => std::thread::sleep(IDLE_POLL),
                Err(err) => {
                    eprintln!("调度器读取队列失败：{err}");
                    std::thread::sleep(IDLE_POLL);
                }
            }
        }
    }

    fn next_queued(&self) -> Result<Option<String>, String> {
        let guard = self
            .ctx
            .db
            .lock()
            .map_err(|_| "数据库锁被占用".to_string())?;
        store::next_queued_task(guard.conn())
    }

    fn run_one(&self, task_id: &str) -> bool {
        // 状态校验与运行占位必须在同一把数据库锁内完成。否则 `cancel` 可能在
        // “确认 queued” 之后、登记运行之前把任务退回 draft，任务仍会被启动。
        let guard = {
            let db = match self.ctx.db.lock() {
                Ok(db) => db,
                Err(_) => {
                    eprintln!("调度器无法锁定数据库认领任务 {task_id}：数据库锁被占用");
                    return false;
                }
            };
            // 出队与真正开始之间可能被取消（queued → draft），此时跳过。
            match store::task_status(db.conn(), task_id) {
                Ok(TaskStatus::Queued) => {}
                Ok(_) => return false,
                Err(err) => {
                    eprintln!("调度器读取任务 {task_id} 状态失败：{err}");
                    return false;
                }
            }
            let mut running = match self.ctx.running.lock() {
                Ok(running) => running,
                Err(_) => {
                    eprintln!("调度器无法登记运行中的任务 {task_id}：运行状态锁被占用");
                    return false;
                }
            };
            if !running.insert(task_id.to_string()) {
                return false;
            }
            RunningGuard {
                running: Arc::clone(&self.ctx.running),
                task_id: task_id.to_string(),
            }
        };

        // 单个运行失败（包括 panic）不能带走整个调度线程。
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
            orchestrator::run_task_blocking(&self.ctx, task_id)
        }));
        let run_failed = match outcome {
            Ok(Ok(())) => false,
            Ok(Err(err)) => {
                self.record_failure(task_id, &err);
                true
            }
            Err(payload) => {
                self.record_failure(task_id, &panic_message(payload));
                true
            }
        };

        drop(guard);
        self.settle(task_id, run_failed);
        true
    }

    /// 运行失败且未留下终态时补齐失败状态；运行开始前就失败的任务退回 draft，
    /// 避免调度器反复取到同一个无法启动的 queued 任务。
    fn record_failure(&self, task_id: &str, message: &str) {
        let result = (|| -> Result<(), String> {
            let guard = self
                .ctx
                .db
                .lock()
                .map_err(|_| "数据库锁被占用".to_string())?;
            let detail = store::get_task(guard.conn(), task_id)?;
            store::append_event(
                guard.conn(),
                NewEvent {
                    project_id: detail.task.project_id.clone(),
                    entity: "task".to_string(),
                    entity_id: task_id.to_string(),
                    kind: "orchestrator_error".to_string(),
                    payload: serde_json::json!({ "message": message }),
                },
            )?;
            match detail.task.status {
                TaskStatus::Running | TaskStatus::Verifying => {
                    store::transition(guard.conn(), task_id, TaskStatus::Failed)?;
                }
                TaskStatus::Queued => {
                    store::transition(guard.conn(), task_id, TaskStatus::Draft)?;
                }
                _ => {}
            }
            Ok(())
        })();
        if let Err(err) = result {
            eprintln!("记录任务 {task_id} 失败状态出错：{err}");
        }
    }

    /// 运行收尾：失败的任务在尝试次数未用尽时自动重排，用尽后转人工决策。
    ///
    /// 用户取消只压制自动重排；次数用尽时仍必须留下 `retry_exhausted` 决策，
    /// 否则任务会停在 `failed` 且没有任何人工入口。
    fn settle(&self, task_id: &str, run_failed: bool) {
        // 运行结束（无论成败）都清除取消意图：本次运行已经消费掉它。
        let cancelled = self.take_cancelled(task_id);

        let result = (|| -> Result<(), String> {
            let guard = self
                .ctx
                .db
                .lock()
                .map_err(|_| "数据库锁被占用".to_string())?;
            let detail = store::get_task(guard.conn(), task_id)?;
            match detail.task.status {
                // 运行在开始前就失败（例如 worktree 创建失败）：退回 draft，
                // 避免调度器反复取到同一个无法启动的 queued 任务。
                TaskStatus::Queued if run_failed => {
                    store::transition(guard.conn(), task_id, TaskStatus::Draft)?;
                }
                TaskStatus::Failed => {
                    if detail.task.attempts >= store::MAX_ATTEMPTS {
                        if let Err(err) = store::open_decision(
                            guard.conn(),
                            NewDecision {
                                project_id: detail.task.project_id.clone(),
                                task_id: Some(task_id.to_string()),
                                run_id: None,
                                kind: "retry_exhausted".to_string(),
                                prompt: format!(
                                    "任务已失败 {} 次，达到自动重试上限（{}），请选择重试或放弃。",
                                    detail.task.attempts,
                                    store::MAX_ATTEMPTS
                                ),
                                options: vec!["retry".to_string(), "abort".to_string()],
                            },
                        ) {
                            eprintln!("为任务 {task_id} 创建 retry_exhausted 决策失败：{err}");
                        }
                        store::transition(guard.conn(), task_id, TaskStatus::NeedsHuman)?;
                    } else if !cancelled {
                        store::transition(guard.conn(), task_id, TaskStatus::Queued)?;
                    }
                }
                _ => {}
            }
            Ok(())
        })();
        if let Err(err) = result {
            eprintln!("调度器收尾任务 {task_id} 失败：{err}");
        }
    }

    /// 取出并清除任务的取消记录；返回本次运行是否曾被请求取消。
    fn take_cancelled(&self, task_id: &str) -> bool {
        let requested = self
            .cancel_requested
            .lock()
            .map(|mut requested| requested.remove(task_id))
            .unwrap_or(false);
        self.clear_cancelled(task_id);
        requested
    }

    /// 丢弃任务的取消意图与调度取消记录。
    fn clear_cancelled(&self, task_id: &str) {
        if let Ok(mut requested) = self.cancel_requested.lock() {
            requested.remove(task_id);
        }
        if let Ok(mut cancelled) = self.ctx.cancelled.lock() {
            cancelled.remove(task_id);
        }
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "运行任务时发生 panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{
        AgentAdapter, AgentEvent, AgentManifest, AuthState, Capabilities, Decision, DetectResult,
        EventSink, RunSpec, RunStatus, SessionHandle,
    };
    use crate::db::{Db, RegisterProjectInput};
    use crate::store::{Criterion, NewTask};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Instant;

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);
    /// 任务创建序号：显式写入 `items.created_at`，让 FIFO 顺序不依赖系统时钟精度。
    static TASK_ORDER: AtomicUsize = AtomicUsize::new(0);

    /// 审查输出必须是合法 JSON；`test_diff` 为空是因为夹具改动不涉及测试/CI 文件。
    const REVIEW_JSON: &str =
        r#"{"verdict":"pass","summary":"diff matches the task","risk_flags":[],"test_diff":[]}"#;

    struct StartRecord {
        title: String,
        /// `start` 被调用瞬间仍在启动中的会话数量。
        active: usize,
        reviewer: bool,
    }

    /// 脚本化适配器：不启动子进程、不调用模型，按脚本同步发出事件。
    ///
    /// worker 提示词含任务标题，reviewer 提示词含 `verdict` 输出契约，可据此区分。
    struct ScriptedAdapter {
        starts: Mutex<Vec<StartRecord>>,
        fail_first_worker: AtomicBool,
        worker_starts: AtomicUsize,
        reviewer_starts: AtomicUsize,
        active: AtomicUsize,
        max_active: AtomicUsize,
        next_session: AtomicUsize,
    }

    impl ScriptedAdapter {
        fn new() -> Self {
            Self {
                starts: Mutex::new(Vec::new()),
                fail_first_worker: AtomicBool::new(false),
                worker_starts: AtomicUsize::new(0),
                reviewer_starts: AtomicUsize::new(0),
                active: AtomicUsize::new(0),
                max_active: AtomicUsize::new(0),
                next_session: AtomicUsize::new(0),
            }
        }

        /// 第一次 worker 启动以 `Ended(Failed)` 结束，之后的启动都成功。
        fn failing_first_worker(self) -> Self {
            self.fail_first_worker.store(true, Ordering::SeqCst);
            self
        }

        fn max_active(&self) -> usize {
            self.max_active.load(Ordering::SeqCst)
        }

        fn worker_starts(&self) -> usize {
            self.worker_starts.load(Ordering::SeqCst)
        }

        fn reviewer_starts(&self) -> usize {
            self.reviewer_starts.load(Ordering::SeqCst)
        }

        /// 按启动顺序返回 worker 的 `(标题, 启动时并发数)`。
        fn worker_records(&self) -> Vec<(String, usize)> {
            self.starts
                .lock()
                .expect("启动记录锁被占用")
                .iter()
                .filter(|record| !record.reviewer)
                .map(|record| (record.title.clone(), record.active))
                .collect()
        }
    }

    /// worker 提示词为 `任务：{title}`；reviewer 提示词为 `TASK TITLE\n{title}`。
    fn prompt_title(prompt: &str) -> String {
        let rest = prompt
            .split_once("任务：")
            .map(|(_, rest)| rest)
            .or_else(|| prompt.split_once("TASK TITLE\n").map(|(_, rest)| rest));
        rest.map(|rest| rest.lines().next().unwrap_or_default().trim().to_string())
            .unwrap_or_default()
    }

    impl AgentAdapter for ScriptedAdapter {
        fn manifest(&self) -> AgentManifest {
            AgentManifest {
                id: "scripted".to_string(),
                name: "Scripted".to_string(),
                version: "0.0.0".to_string(),
                capabilities: Capabilities {
                    streaming: true,
                    approvals: true,
                    cancel: false,
                    usage: false,
                    cost_cap: false,
                    resume: false,
                },
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

        fn start(&self, spec: RunSpec, sink: EventSink) -> Result<SessionHandle, String> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            let reviewer = spec.prompt.contains("verdict");
            let title = prompt_title(&spec.prompt);
            self.starts
                .lock()
                .expect("启动记录锁被占用")
                .push(StartRecord {
                    title: title.clone(),
                    active,
                    reviewer,
                });
            let session = self.next_session.fetch_add(1, Ordering::SeqCst) + 1;

            if reviewer {
                self.reviewer_starts.fetch_add(1, Ordering::SeqCst);
                sink(AgentEvent::TextDelta {
                    text: REVIEW_JSON.to_string(),
                });
                sink(AgentEvent::Ended {
                    status: RunStatus::Completed,
                });
                self.active.fetch_sub(1, Ordering::SeqCst);
                return Ok(SessionHandle {
                    id: format!("scripted-review-{session}"),
                });
            }

            self.worker_starts.fetch_add(1, Ordering::SeqCst);
            // 模拟真实 worker 的启动耗时：串行调度下并发启动数必须保持 1。
            std::thread::sleep(Duration::from_millis(100));
            let result: Result<(), String> = if self.fail_first_worker.swap(false, Ordering::SeqCst)
            {
                sink(AgentEvent::Ended {
                    status: RunStatus::Failed,
                });
                Ok(())
            } else {
                // 每个任务写入不同内容：后续任务基于前一任务合并后的提交工作，
                // 相同内容会产生空 diff 而被判定为“没有改动”。
                match fs::write(spec.cwd.join("done.txt"), format!("{title}\n")) {
                    Ok(()) => {
                        sink(AgentEvent::Ended {
                            status: RunStatus::Completed,
                        });
                        Ok(())
                    }
                    Err(err) => Err(format!("写入 done.txt 失败：{err}")),
                }
            };
            self.active.fetch_sub(1, Ordering::SeqCst);
            result.map(|()| SessionHandle {
                id: format!("scripted-worker-{session}"),
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

    fn git(repo: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .expect("执行 git");
        assert!(status.success(), "git {:?} 失败", args);
    }

    /// 临时夹具：真实 git 仓库 + 独立数据目录 + SQLite 数据库，随 Drop 清理。
    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "temli-scheduler-{}-{unique}-{label}",
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
            Self { root }
        }

        fn repo(&self) -> PathBuf {
            self.root.join("repo")
        }

        /// 打开数据库、登记项目并组装运行上下文。
        fn setup(&self, adapter: Arc<dyn AgentAdapter>) -> (RunContext, String) {
            let repo = self.repo();
            let db = Db::open(&self.root.join("temli.db")).expect("打开临时数据库");
            let project = db
                .insert_project(
                    RegisterProjectInput {
                        repo_path: repo.to_string_lossy().into_owned(),
                        name: Some("demo".to_string()),
                        github_owner: None,
                        github_repo: None,
                        default_branch: Some("main".to_string()),
                    },
                    repo.canonicalize().expect("规范化仓库路径"),
                )
                .expect("登记临时项目");
            let ctx = RunContext {
                db: Arc::new(Mutex::new(db)),
                adapter,
                data_dir: self.root.join("data"),
                running: Arc::new(Mutex::new(HashSet::new())),
                cancels: orchestrator::CancelRegistry::new(),
                merge_lock: Arc::new(Mutex::new(())),
                cancelled: Arc::new(Mutex::new(std::collections::HashSet::new())),
            };
            (ctx, project.id)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn add_task(ctx: &RunContext, project_id: &str, title: &str) -> String {
        let guard = ctx.db.lock().expect("数据库锁被占用");
        let task = store::create_task(
            guard.conn(),
            NewTask {
                project_id: project_id.to_string(),
                title: title.to_string(),
                prompt: format!("实现 {title}"),
                criteria: vec![Criterion {
                    id: "c-1".to_string(),
                    cmd: "test -f done.txt".to_string(),
                    expect_exit: 0,
                    expect_match: None,
                }],
            },
        )
        .expect("创建任务");
        // `next_queued_task` 按 `items.created_at` 排序；序号单调递增即可确定 FIFO 顺序。
        let order = TASK_ORDER.fetch_add(1, Ordering::Relaxed);
        guard
            .conn()
            .execute(
                "UPDATE items SET created_at = ?1
                 WHERE id = (SELECT item_id FROM tasks WHERE id = ?2)",
                rusqlite::params![format!("2026-01-01T00:00:00.{order:03}Z"), task.id],
            )
            .expect("写入确定性的创建时间");
        task.id
    }

    fn status(ctx: &RunContext, task_id: &str) -> TaskStatus {
        let guard = ctx.db.lock().expect("数据库锁被占用");
        store::get_task(guard.conn(), task_id)
            .expect("读取任务")
            .task
            .status
    }

    fn wait_until(mut predicate: impl FnMut() -> bool, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if predicate() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn runs_queued_tasks_serially_in_enqueue_order() {
        let fixture = Fixture::new("serial");
        let adapter = Arc::new(ScriptedAdapter::new());
        let (ctx, project_id) = fixture.setup(adapter.clone());

        let mut ids = Vec::new();
        for title in ["first", "second", "third"] {
            ids.push(add_task(&ctx, &project_id, title));
        }

        let scheduler = Scheduler::new(ctx.clone());
        for id in &ids {
            scheduler.enqueue(id).expect("入队任务");
        }
        scheduler.start().expect("启动调度线程");

        let all_merged = wait_until(
            || ids.iter().all(|id| status(&ctx, id) == TaskStatus::Merged),
            Duration::from_secs(10),
        );
        let observed = ids
            .iter()
            .map(|id| (id.as_str(), status(&ctx, id)))
            .collect::<Vec<_>>();
        assert!(
            all_merged,
            "三个任务都应在 10 秒内合并，实际状态：{observed:?}"
        );

        // 同一时刻只能有一个 worker 在启动。
        assert_eq!(adapter.max_active(), 1, "工作线程必须串行执行任务");
        let starts = adapter.worker_records();
        assert_eq!(
            starts
                .iter()
                .map(|(title, _)| title.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second", "third"],
            "worker 启动顺序必须与入队顺序一致"
        );
        assert!(
            starts.iter().all(|(_, active)| *active == 1),
            "启动瞬间的并发数必须为 1：{starts:?}"
        );

        // 合并事件按 rowid 落库，反映真实的合并顺序。
        let merged_order = {
            let guard = ctx.db.lock().expect("数据库锁被占用");
            let mut stmt = guard
                .conn()
                .prepare("SELECT entity_id FROM events WHERE type = 'merged' ORDER BY rowid ASC")
                .expect("准备合并事件查询");
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .expect("查询合并事件");
            rows.map(|row| row.expect("读取合并事件"))
                .collect::<Vec<_>>()
        };
        assert_eq!(merged_order, ids, "合并顺序必须与入队顺序一致");
    }

    #[test]
    fn failed_run_is_retried_once_then_merges() {
        let fixture = Fixture::new("retry");
        let adapter = Arc::new(ScriptedAdapter::new().failing_first_worker());
        let (ctx, project_id) = fixture.setup(adapter.clone());
        let task_id = add_task(&ctx, &project_id, "retry-me");

        let scheduler = Scheduler::new(ctx.clone());
        scheduler.enqueue(&task_id).expect("入队任务");
        scheduler.start().expect("启动调度线程");

        assert!(
            wait_until(
                || status(&ctx, &task_id) == TaskStatus::Merged,
                Duration::from_secs(10),
            ),
            "任务应在重试后合并，实际状态：{:?}",
            status(&ctx, &task_id)
        );

        let detail = {
            let guard = ctx.db.lock().expect("数据库锁被占用");
            store::get_task(guard.conn(), &task_id).expect("读取任务")
        };
        assert_eq!(detail.task.attempts, 2, "第一次运行失败后应重试一次");
        assert_eq!(detail.runs.len(), 2, "失败与重试各对应一次运行");
        assert_eq!(adapter.worker_starts(), 2);
        assert_eq!(adapter.reviewer_starts(), 1, "失败的运行不应进入审查");
    }

    #[test]
    fn cancel_returns_queued_task_to_draft_without_worker() {
        let fixture = Fixture::new("cancel");
        let adapter = Arc::new(ScriptedAdapter::new());
        let (ctx, project_id) = fixture.setup(adapter.clone());
        let task_id = add_task(&ctx, &project_id, "cancel-me");

        // 不调用 `start()`：队列只能由测试显式驱动。
        let scheduler = Scheduler::new(ctx.clone());
        scheduler.enqueue(&task_id).expect("入队任务");
        assert_eq!(status(&ctx, &task_id), TaskStatus::Queued);
        {
            let guard = ctx.db.lock().expect("数据库锁被占用");
            assert_eq!(
                store::next_queued_task(guard.conn()).expect("读取队列"),
                Some(task_id.clone())
            );
        }

        scheduler.cancel(&task_id).expect("取消排队任务");

        assert_eq!(status(&ctx, &task_id), TaskStatus::Draft);
        assert!(
            !ctx.cancelled
                .lock()
                .expect("取消意图锁被占用")
                .contains(&task_id),
            "排队取消成功后必须清除取消意图"
        );
        {
            let guard = ctx.db.lock().expect("数据库锁被占用");
            assert!(
                store::next_queued_task(guard.conn())
                    .expect("读取队列")
                    .is_none(),
                "取消后队列必须为空"
            );
        }
        assert!(
            !scheduler.run_one(&task_id),
            "已退回 draft 的任务不能被 worker 认领"
        );
        assert_eq!(adapter.worker_starts(), 0, "未启动工作线程时不应调用适配器");
    }

    #[test]
    fn cancel_while_claimed_keeps_status_and_records_intent() {
        let fixture = Fixture::new("cancel-claimed");
        let (ctx, project_id) = fixture.setup(Arc::new(ScriptedAdapter::new()));
        let task_id = add_task(&ctx, &project_id, "claimed");

        let scheduler = Scheduler::new(ctx.clone());
        scheduler.enqueue(&task_id).expect("入队任务");
        // 模拟 worker 已认领、适配器会话尚未登记的窗口。
        ctx.running
            .lock()
            .expect("运行状态锁被占用")
            .insert(task_id.clone());

        scheduler.cancel(&task_id).expect("取消已认领任务");

        assert_eq!(
            status(&ctx, &task_id),
            TaskStatus::Queued,
            "已认领的任务不能被退回 draft"
        );
        assert!(
            ctx.cancelled
                .lock()
                .expect("取消意图锁被占用")
                .contains(&task_id),
            "取消意图必须保留到运行器握手阶段"
        );

        // 运行收尾时清除意图，避免影响下一次入队。
        scheduler.settle(&task_id, false);
        assert!(
            !ctx.cancelled
                .lock()
                .expect("取消意图锁被占用")
                .contains(&task_id),
            "运行收尾必须清除取消意图"
        );
    }

    #[test]
    fn cancelled_run_at_max_attempts_still_needs_human() {
        let fixture = Fixture::new("cancel-exhausted");
        let (ctx, project_id) = fixture.setup(Arc::new(ScriptedAdapter::new()));
        let task_id = add_task(&ctx, &project_id, "exhausted");

        let scheduler = Scheduler::new(ctx.clone());
        scheduler.enqueue(&task_id).expect("入队任务");
        {
            let guard = ctx.db.lock().expect("数据库锁被占用");
            store::transition(guard.conn(), &task_id, TaskStatus::Running).expect("进入运行");
            store::transition(guard.conn(), &task_id, TaskStatus::Failed).expect("标记失败");
            guard
                .conn()
                .execute(
                    "UPDATE tasks SET attempts = ?2 WHERE id = ?1",
                    rusqlite::params![task_id, store::MAX_ATTEMPTS],
                )
                .expect("写入尝试次数");
        }
        scheduler
            .cancel_requested
            .lock()
            .expect("调度取消记录锁被占用")
            .insert(task_id.clone());
        ctx.cancelled
            .lock()
            .expect("取消意图锁被占用")
            .insert(task_id.clone());

        scheduler.settle(&task_id, true);

        assert_eq!(
            status(&ctx, &task_id),
            TaskStatus::NeedsHuman,
            "取消的运行用尽重试次数也必须转人工"
        );
        let open_decisions: i64 = {
            let guard = ctx.db.lock().expect("数据库锁被占用");
            guard
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM decisions
                     WHERE task_id = ?1 AND kind = 'retry_exhausted' AND status = 'open'",
                    rusqlite::params![task_id],
                    |row| row.get(0),
                )
                .expect("统计未决决策")
        };
        assert_eq!(open_decisions, 1, "必须留下 retry_exhausted 决策");
        assert!(
            !ctx.cancelled
                .lock()
                .expect("取消意图锁被占用")
                .contains(&task_id),
            "运行收尾必须清除取消意图"
        );
    }
}
