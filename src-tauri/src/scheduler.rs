//! 有界并发任务调度器。
//!
//! `enqueue` 只负责把任务压入 `queued`。工作线程每 200ms 扫描一次项目，按项目
//! 并发上限认领就绪任务；依赖尚未全部合并的任务不会被 `next_ready_tasks` 返回。
//! 每个被认领的任务在独立线程里运行，单个运行失败（包括 panic）不会阻塞其它任务。
//! 任务结束后由本模块决定自动重试还是转人工处理。

use crate::orchestrator::{self, RunContext};
use crate::store::{self, NewDecision, NewEvent, TaskStatus};
use std::collections::{HashMap, HashSet};
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 调度扫描间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(200);

pub struct Scheduler {
    ctx: RunContext,
    /// 每个项目已认领但尚未收尾的任务数。
    ///
    /// 认领后到状态迁移为 `running` 之间（worktree 创建、运行记录写入）任务仍是
    /// `queued`，而进入 `verifying` 后又会离开 `running`；只看数据库状态会低估
    /// 占用。认领计数覆盖整个运行生命周期，保证并发上限不被突破。
    inflight: Arc<Mutex<HashMap<String, usize>>>,
    /// 已请求取消的已认领任务。
    ///
    /// `RunContext.cancelled` 是给运行器握手阶段读取的共享意图，会被运行的收尾
    /// 守卫清除；调度器在 `settle` 时仍要知道本次运行是否被用户取消，因此单独
    /// 保留一份记录。
    cancel_requested: Mutex<HashSet<String>>,
}

/// 运行占位守卫：无论正常结束、提前返回还是 panic 展开，都会释放运行集合与
/// 项目并发计数。
struct RunningGuard {
    running: Arc<Mutex<HashSet<String>>>,
    inflight: Arc<Mutex<HashMap<String, usize>>>,
    project_id: String,
    task_id: String,
}

impl Drop for RunningGuard {
    fn drop(&mut self) {
        if let Ok(mut running) = self.running.lock() {
            running.remove(&self.task_id);
        }
        if let Ok(mut inflight) = self.inflight.lock() {
            let remaining = match inflight.get_mut(&self.project_id) {
                Some(count) => {
                    *count = count.saturating_sub(1);
                    *count
                }
                None => 0,
            };
            if remaining == 0 {
                inflight.remove(&self.project_id);
            }
        }
    }
}

impl Scheduler {
    pub fn new(ctx: RunContext) -> Arc<Self> {
        Arc::new(Self {
            ctx,
            inflight: Arc::new(Mutex::new(HashMap::new())),
            cancel_requested: Mutex::new(HashSet::new()),
        })
    }

    /// 启动唯一的调度扫描线程；只能调用一次。
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
            // 与 `claim_next` 的认领顺序一致（先数据库锁、后运行集合），
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

    fn work_loop(self: Arc<Self>) {
        loop {
            self.dispatch_round();
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// 扫描全部项目并尽量填满每个项目的并发额度。
    ///
    /// 单个项目出错只记录日志，不能影响其它项目继续调度。
    fn dispatch_round(self: &Arc<Self>) {
        let projects = {
            let guard = match self.ctx.db.lock() {
                Ok(guard) => guard,
                Err(_) => {
                    eprintln!("调度器无法锁定数据库读取项目：数据库锁被占用");
                    return;
                }
            };
            match guard.list_projects() {
                Ok(projects) => projects,
                Err(err) => {
                    eprintln!("调度器读取项目列表失败：{err}");
                    return;
                }
            }
        };

        for project in projects {
            if let Err(err) = self.dispatch_project(&project.id) {
                eprintln!("调度器派发项目 {} 的任务失败：{err}", project.id);
            }
        }
    }

    /// 启动线程失败时立即结束本项目本轮派发，不再继续认领。
    ///
    /// 线程创建失败通常意味着系统资源耗尽，继续认领只会堆积更多无法启动的
    /// 任务并反复失败；把错误交给 [`Self::dispatch_round`] 记录，下个扫描周期
    /// 再重试。认领占位已由 `spawn_run` 释放，任务仍停在 queued。
    fn dispatch_project(self: &Arc<Self>, project_id: &str) -> Result<(), String> {
        while let Some((task_id, guard)) = self.claim_next(project_id)? {
            self.spawn_run(task_id, guard)?;
        }
        Ok(())
    }

    /// 在数据库锁内认领一个就绪任务。
    ///
    /// 候选按 [`store::next_ready_tasks`] 的顺序逐个尝试：已被其它线程认领
    /// （登记在运行集合、数据库状态仍是 `queued`）或已不再就绪的任务直接跳过，
    /// 让后面的就绪任务填补空余额度，而不是被队头的启动中任务挡住。
    ///
    /// 状态复查、运行集合登记与项目并发计数必须在同一把文件锁内完成：否则
    /// `cancel` 可能在“确认 queued”之后、登记运行之前把任务退回 draft，任务
    /// 仍会被启动；并发计数也会与认领结果撕裂。
    ///
    /// 返回 `None` 表示当前没有空余额度，或没有可认领的就绪任务。
    fn claim_next(&self, project_id: &str) -> Result<Option<(String, RunningGuard)>, String> {
        let db = self
            .ctx
            .db
            .lock()
            .map_err(|_| "数据库锁被占用".to_string())?;
        let conn = db.conn();

        let limit = store::project_concurrency(conn, project_id)?.max(1);
        let running_count = count_running_tasks(conn, project_id)?;
        let claimed_count = self
            .inflight
            .lock()
            .map_err(|_| "运行计数锁被占用".to_string())?
            .get(project_id)
            .copied()
            .unwrap_or(0);
        // `running` 与认领计数覆盖的生命周期不同（认领早于 running、verifying
        // 晚于 running），取较大值即可确保任一时刻都不超过上限。
        if running_count.max(claimed_count as i64) >= limit {
            return Ok(None);
        }

        let mut running = self
            .ctx
            .running
            .lock()
            .map_err(|_| "运行状态锁被占用".to_string())?;
        for task_id in store::next_ready_tasks(conn, project_id)? {
            // 已认领但尚未迁移到 running 的任务仍会出现在就绪查询里；继续看下一个
            // 候选，避免它们长期占用队头却又不启动。
            if running.contains(&task_id) {
                continue;
            }
            // 查询与认领持有同一把锁，理论上状态不会变化；仍以库内就绪条件为准，
            // 防止就绪列表来自更早的快照。
            if !store::task_ready(conn, &task_id)? {
                continue;
            }
            if !running.insert(task_id.clone()) {
                continue;
            }

            let mut inflight = match self.inflight.lock() {
                Ok(inflight) => inflight,
                Err(_) => {
                    running.remove(&task_id);
                    return Err("运行计数锁被占用".to_string());
                }
            };
            *inflight.entry(project_id.to_string()).or_insert(0) += 1;
            drop(inflight);
            drop(running);

            return Ok(Some((
                task_id.clone(),
                RunningGuard {
                    running: Arc::clone(&self.ctx.running),
                    inflight: Arc::clone(&self.inflight),
                    project_id: project_id.to_string(),
                    task_id,
                },
            )));
        }
        Ok(None)
    }

    /// 在独立线程中运行已认领的任务；调度扫描线程绝不等候运行结束。
    ///
    /// 线程创建失败时闭包连同 `RunningGuard` 一起被丢弃，运行集合与并发计数
    /// 自动释放，任务仍是 queued，下一轮扫描会重试。
    fn spawn_run(self: &Arc<Self>, task_id: String, guard: RunningGuard) -> Result<(), String> {
        let name = format!("temli-run-{}", short_id(&task_id));
        let scheduler = Arc::clone(self);
        std::thread::Builder::new()
            .name(name.clone())
            .spawn(move || scheduler.run_claimed(task_id, guard))
            .map(|_| ())
            .map_err(|err| format!("无法启动任务线程 {name}：{err}"))
    }

    fn run_claimed(self: Arc<Self>, task_id: String, guard: RunningGuard) {
        // 单个运行失败（包括 panic）不能带走调度线程或其它任务线程。
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
            orchestrator::run_task_blocking(&self.ctx, &task_id)
        }));
        let run_failed = match outcome {
            Ok(Ok(())) => false,
            Ok(Err(err)) => {
                self.record_failure(&task_id, &err);
                true
            }
            Err(payload) => {
                self.record_failure(&task_id, &panic_message(payload));
                true
            }
        };

        self.settle(&task_id, run_failed);
        // 收尾完成后再释放占位，避免自动重排的任务在 settle 期间被再次认领。
        drop(guard);
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
    /// 用户取消会压制自动重排，并把已重排回 `queued` 的任务退回 `draft`；
    /// 次数用尽时仍必须留下 `retry_exhausted` 决策，否则任务会停在 `failed`
    /// 且没有任何人工入口。
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
                // 运行期间被取消、随后又被重排回 queued 的任务（例如 rebase
                // 重试）也必须退回 draft：取消意图已在这里消费，留着它会让调度
                // 器继续启动一个用户已经取消的任务。
                TaskStatus::Queued if run_failed || cancelled => {
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

/// 统计项目内正在运行（适配器会话未结束）的任务数。
fn count_running_tasks(conn: &rusqlite::Connection, project_id: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM tasks t JOIN items i ON i.id = t.item_id
         WHERE i.project_id = ?1 AND i.status = 'running'",
        rusqlite::params![project_id],
        |row| row.get(0),
    )
    .map_err(|err| format!("统计项目 {project_id} 运行中任务失败：{err}"))
}

/// 线程名里使用的任务 id 前缀。
fn short_id(task_id: &str) -> &str {
    task_id.get(..8).unwrap_or(task_id)
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
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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
        /// worker `start` 的模拟耗时；并发测试把它调大，确保运行窗口重叠。
        worker_delay_ms: AtomicU64,
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
                worker_delay_ms: AtomicU64::new(100),
            }
        }

        /// 第一次 worker 启动以 `Ended(Failed)` 结束，之后的启动都成功。
        fn failing_first_worker(self) -> Self {
            self.fail_first_worker.store(true, Ordering::SeqCst);
            self
        }

        /// 调整 worker 启动的模拟耗时，用于制造并发窗口。
        fn with_worker_delay(self, millis: u64) -> Self {
            self.worker_delay_ms.store(millis, Ordering::SeqCst);
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
            // 模拟真实 worker 的启动耗时：并发上限为 1 时必须保持串行。
            let delay = self.worker_delay_ms.load(Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(delay));
            let result: Result<(), String> = if self.fail_first_worker.swap(false, Ordering::SeqCst)
            {
                sink(AgentEvent::Ended {
                    status: RunStatus::Failed,
                });
                Ok(())
            } else {
                // 每个任务写入自己的文件：并发任务的 worktree 基线相同，改同一个
                // 文件会在合并时产生 add/add 冲突。
                let file = format!("done-{title}.txt");
                match fs::write(spec.cwd.join(file.as_str()), format!("{title}\n")) {
                    Ok(()) => {
                        sink(AgentEvent::Ended {
                            status: RunStatus::Completed,
                        });
                        Ok(())
                    }
                    Err(err) => Err(format!("写入 {file} 失败：{err}")),
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
                    cmd: format!("test -f done-{title}.txt"),
                    expect_exit: 0,
                    expect_match: None,
                }],
            },
        )
        .expect("创建任务");
        // `next_ready_task` 按 `items.created_at` 排序；序号单调递增即可确定 FIFO 顺序。
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

    fn set_project_limit(ctx: &RunContext, project_id: &str, limit: i64) {
        let guard = ctx.db.lock().expect("数据库锁被占用");
        store::set_project_concurrency(guard.conn(), project_id, limit).expect("设置并发上限");
    }

    /// 读取某条事件第一次落库时的 rowid；事件按写入顺序自增，可据此判断先后。
    fn event_rowid(ctx: &RunContext, entity_id: &str, kind: &str) -> i64 {
        let guard = ctx.db.lock().expect("数据库锁被占用");
        guard
            .conn()
            .query_row(
                "SELECT rowid FROM events
                 WHERE entity_id = ?1 AND type = ?2
                 ORDER BY rowid ASC LIMIT 1",
                rusqlite::params![entity_id, kind],
                |row| row.get(0),
            )
            .expect("读取事件 rowid")
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
    fn project_limit_two_runs_independent_tasks_concurrently() {
        let fixture = Fixture::new("concurrent-two");
        // 运行窗口足够长，两次扫描之间启动的任务必然重叠。
        let adapter = Arc::new(ScriptedAdapter::new().with_worker_delay(1_000));
        let (ctx, project_id) = fixture.setup(adapter.clone());
        set_project_limit(&ctx, &project_id, 2);

        let first = add_task(&ctx, &project_id, "alpha");
        let second = add_task(&ctx, &project_id, "beta");

        let scheduler = Scheduler::new(ctx.clone());
        scheduler.enqueue(&first).expect("入队第一个任务");
        scheduler.enqueue(&second).expect("入队第二个任务");
        scheduler.start().expect("启动调度线程");

        let both_merged = wait_until(
            || {
                status(&ctx, &first) == TaskStatus::Merged
                    && status(&ctx, &second) == TaskStatus::Merged
            },
            Duration::from_secs(15),
        );
        assert!(
            both_merged,
            "两个任务都应在 15 秒内合并，实际状态：first={:?} second={:?}",
            status(&ctx, &first),
            status(&ctx, &second)
        );
        assert_eq!(
            adapter.max_active(),
            2,
            "并发上限为 2 时两个任务必须同时运行"
        );
        assert_eq!(adapter.worker_starts(), 2);

        let mut titles = adapter
            .worker_records()
            .into_iter()
            .map(|(title, _)| title)
            .collect::<Vec<_>>();
        titles.sort();
        assert_eq!(titles, vec!["alpha".to_string(), "beta".to_string()]);
    }

    #[test]
    fn project_limit_one_never_overlaps_tasks() {
        let fixture = Fixture::new("concurrent-one");
        let adapter = Arc::new(ScriptedAdapter::new().with_worker_delay(500));
        let (ctx, project_id) = fixture.setup(adapter.clone());
        // 默认并发上限即 1。
        {
            let guard = ctx.db.lock().expect("数据库锁被占用");
            assert_eq!(
                store::project_concurrency(guard.conn(), &project_id).expect("读取并发上限"),
                1
            );
        }

        let first = add_task(&ctx, &project_id, "alpha");
        let second = add_task(&ctx, &project_id, "beta");

        let scheduler = Scheduler::new(ctx.clone());
        scheduler.enqueue(&first).expect("入队第一个任务");
        scheduler.enqueue(&second).expect("入队第二个任务");
        scheduler.start().expect("启动调度线程");

        let both_merged = wait_until(
            || {
                status(&ctx, &first) == TaskStatus::Merged
                    && status(&ctx, &second) == TaskStatus::Merged
            },
            Duration::from_secs(15),
        );
        assert!(
            both_merged,
            "两个任务都应在 15 秒内合并，实际状态：first={:?} second={:?}",
            status(&ctx, &first),
            status(&ctx, &second)
        );
        assert_eq!(
            adapter.max_active(),
            1,
            "并发上限为 1 时任何时刻都只能有一个任务运行"
        );
        assert_eq!(adapter.worker_starts(), 2);
    }

    #[test]
    fn dependency_blocks_task_until_blocker_merges() {
        let fixture = Fixture::new("dependency");
        let adapter = Arc::new(ScriptedAdapter::new().with_worker_delay(300));
        let (ctx, project_id) = fixture.setup(adapter.clone());
        set_project_limit(&ctx, &project_id, 2);

        let blocker = add_task(&ctx, &project_id, "alpha");
        let dependent = add_task(&ctx, &project_id, "beta");
        {
            let guard = ctx.db.lock().expect("数据库锁被占用");
            store::add_dependency(guard.conn(), &dependent, &blocker).expect("登记依赖");
        }

        let scheduler = Scheduler::new(ctx.clone());
        scheduler.enqueue(&blocker).expect("入队前置任务");
        scheduler.enqueue(&dependent).expect("入队依赖任务");
        scheduler.start().expect("启动调度线程");

        let both_merged = wait_until(
            || {
                status(&ctx, &blocker) == TaskStatus::Merged
                    && status(&ctx, &dependent) == TaskStatus::Merged
            },
            Duration::from_secs(15),
        );
        assert!(
            both_merged,
            "两个任务都应在 15 秒内合并，实际状态：blocker={:?} dependent={:?}",
            status(&ctx, &blocker),
            status(&ctx, &dependent)
        );

        let starts = adapter
            .worker_records()
            .into_iter()
            .map(|(title, _)| title)
            .collect::<Vec<_>>();
        assert_eq!(
            starts,
            vec!["alpha".to_string(), "beta".to_string()],
            "依赖任务必须在前置任务之后启动"
        );
        assert_eq!(adapter.max_active(), 1, "前置任务未合并时依赖任务不能启动");

        let blocker_merged = event_rowid(&ctx, &blocker, "merged");
        let dependent_started = event_rowid(&ctx, &dependent, "run_started");
        assert!(
            blocker_merged < dependent_started,
            "依赖任务必须在前置任务合并后才开始运行（merged rowid {blocker_merged}，run_started rowid {dependent_started}）"
        );
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
            scheduler
                .claim_next(&project_id)
                .expect("读取就绪任务")
                .is_none(),
            "已退回 draft 的任务不能被调度器认领"
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

    #[test]
    fn cancelled_queued_task_returns_to_draft_on_settle() {
        let fixture = Fixture::new("cancel-requeued");
        let (ctx, project_id) = fixture.setup(Arc::new(ScriptedAdapter::new()));
        let task_id = add_task(&ctx, &project_id, "requeued");

        let scheduler = Scheduler::new(ctx.clone());
        scheduler.enqueue(&task_id).expect("入队任务");
        // 模拟运行在 rebase 重试后把任务重新排回 queued，取消意图尚未被消费。
        scheduler
            .cancel_requested
            .lock()
            .expect("调度取消记录锁被占用")
            .insert(task_id.clone());
        ctx.cancelled
            .lock()
            .expect("取消意图锁被占用")
            .insert(task_id.clone());
        assert_eq!(status(&ctx, &task_id), TaskStatus::Queued);

        scheduler.settle(&task_id, false);

        assert_eq!(
            status(&ctx, &task_id),
            TaskStatus::Draft,
            "已取消并重排的 queued 任务必须在收尾时退回 draft"
        );
        assert!(
            !ctx.cancelled
                .lock()
                .expect("取消意图锁被占用")
                .contains(&task_id),
            "收尾必须清除取消意图"
        );
        {
            let guard = ctx.db.lock().expect("数据库锁被占用");
            assert!(
                store::next_queued_task(guard.conn())
                    .expect("读取队列")
                    .is_none(),
                "已取消的任务不能留在队列里"
            );
        }
    }

    #[test]
    fn claimed_queued_task_does_not_block_other_ready_tasks() {
        let fixture = Fixture::new("starvation");
        let (ctx, project_id) = fixture.setup(Arc::new(ScriptedAdapter::new()));
        set_project_limit(&ctx, &project_id, 2);

        let first = add_task(&ctx, &project_id, "alpha");
        let second = add_task(&ctx, &project_id, "beta");

        let scheduler = Scheduler::new(ctx.clone());
        scheduler.enqueue(&first).expect("入队第一个任务");
        scheduler.enqueue(&second).expect("入队第二个任务");

        // 模拟第一个任务已被认领、仍在创建 worktree：数据库状态还是 queued，
        // 但运行集合与项目认领计数已经登记。
        ctx.running
            .lock()
            .expect("运行状态锁被占用")
            .insert(first.clone());
        *scheduler
            .inflight
            .lock()
            .expect("运行计数锁被占用")
            .entry(project_id.clone())
            .or_insert(0) += 1;

        let (claimed, guard) = scheduler
            .claim_next(&project_id)
            .expect("读取就绪任务")
            .expect("必须越过已认领但未启动的任务继续认领");
        assert_eq!(
            claimed, second,
            "队头任务仍在启动时，后面的就绪任务必须能填补空余额度"
        );

        // 额度已满：再次认领必须停下。
        assert!(
            scheduler
                .claim_next(&project_id)
                .expect("读取就绪任务")
                .is_none(),
            "额度用尽时不能再认领"
        );

        // 释放占位后第二个任务仍是 queued，可以再次被认领。
        drop(guard);
        let (reclaimed, guard) = scheduler
            .claim_next(&project_id)
            .expect("读取就绪任务")
            .expect("释放额度后可再次认领");
        assert_eq!(reclaimed, second);
        drop(guard);
    }
}
