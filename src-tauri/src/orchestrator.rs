use crate::agent::{
    AdapterRegistry, AgentAdapter, AgentEvent, Decision, EventSink, RunSpec, RunStatus,
    SessionHandle,
};
use crate::db::Db;
use crate::evidence::{self, RiskKind};
use crate::gitops;
use crate::prompts;
use crate::store::{
    self, NewDecision, NewEvent, NewEvidence, NewReview, NewRun, TaskRow, TaskStatus,
};
use rusqlite::{Connection, params};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Clone)]
pub struct RunContext {
    pub db: Arc<Mutex<Db>>,
    /// 可用的 agent 适配器；任务级指派与默认值都从这里解析。
    pub adapters: Arc<AdapterRegistry>,
    /// 任务未指派开发者时使用的默认适配器 id。
    pub default_agent: String,
    pub data_dir: PathBuf,
    /// 已启动但尚未结束的任务 id，防止同一任务被并发启动两次。
    pub running: Arc<Mutex<HashSet<String>>>,
    /// 运行中的适配器会话，供取消操作使用。
    pub cancels: CancelRegistry,
    /// 目标分支合并写锁（M2 合并队列）。
    pub merge_lock: Arc<Mutex<()>>,
    /// 已请求取消但尚未拿到会话的任务（取消意图）。
    pub cancelled: Arc<Mutex<HashSet<String>>>,
    /// GitHub 操作面；未配置远端时为 None。
    pub github: Option<Arc<dyn crate::github::GitHubOps>>,
}

/// 任务 id -> (适配器, 会话句柄)，用于取消正在运行的任务。
#[derive(Clone, Default)]
pub struct CancelRegistry {
    inner: Arc<Mutex<HashMap<String, (Arc<dyn AgentAdapter>, SessionHandle)>>>,
}

impl CancelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, task_id: &str, adapter: Arc<dyn AgentAdapter>, handle: SessionHandle) {
        if let Ok(mut map) = self.inner.lock() {
            map.insert(task_id.to_string(), (adapter, handle));
        }
    }

    pub fn clear(&self, task_id: &str) {
        if let Ok(mut map) = self.inner.lock() {
            map.remove(task_id);
        }
    }

    /// 取消正在运行的任务；返回 false 表示没有可取消的会话。
    pub fn cancel(&self, task_id: &str) -> Result<bool, String> {
        let entry = self
            .inner
            .lock()
            .map_err(|_| "取消表锁被占用".to_string())?
            .get(task_id)
            .cloned();
        match entry {
            Some((adapter, handle)) => adapter.cancel(&handle).map(|_| true),
            // 会话尚未注册或已关闭：视为已处理（取消意图由 RunContext.cancelled 记录）。
            None => Ok(true),
        }
    }
}

/// 取消表作用域守卫：无论正常结束、提前返回还是 panic 展开，都会注销本任务的会话，
/// 避免 UI 取消操作命中已经结束的旧句柄。
struct CancelGuard {
    cancels: CancelRegistry,
    task_id: String,
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        self.cancels.clear(&self.task_id);
    }
}

/// 取消意图作用域守卫：无论正常结束、提前返回还是 panic 展开，都会清除本次任务
/// 的取消意图，避免残留标记误杀下一次运行。
struct CancelIntentGuard {
    cancelled: Arc<Mutex<HashSet<String>>>,
    task_id: String,
}

impl Drop for CancelIntentGuard {
    fn drop(&mut self) {
        if let Ok(mut cancelled) = self.cancelled.lock() {
            cancelled.remove(&self.task_id);
        }
    }
}

fn cancel_requested(ctx: &RunContext, task_id: &str) -> bool {
    ctx.cancelled
        .lock()
        .map(|cancelled| cancelled.contains(task_id))
        .unwrap_or(false)
}

fn clear_cancel_intent(ctx: &RunContext, task_id: &str) {
    if let Ok(mut cancelled) = ctx.cancelled.lock() {
        cancelled.remove(task_id);
    }
}

fn lock_db(db: &Arc<Mutex<Db>>) -> Result<std::sync::MutexGuard<'_, Db>, String> {
    db.lock().map_err(|_| "数据库锁被占用".to_string())
}

fn transition(db: &Arc<Mutex<Db>>, task_id: &str, to: TaskStatus) -> Result<(), String> {
    let guard = lock_db(db)?;
    store::transition(guard.conn(), task_id, to).map(|_| ())
}

/// 读取任务允许的最长执行时间（分钟）；缺失或读取失败时回退到默认 30 分钟。
fn task_max_minutes(ctx: &RunContext, task_id: &str) -> u32 {
    let Ok(guard) = ctx.db.lock() else {
        return 30;
    };
    let minutes: i64 = guard
        .conn()
        .query_row(
            "SELECT max_minutes FROM tasks WHERE id = ?1",
            params![task_id],
            |row| row.get(0),
        )
        .unwrap_or(30);
    minutes.clamp(0, 1440) as u32
}

/// 项目的自动合并开关；缺失、读取失败或旧库默认值都视为关闭。读取失败时绝不能
/// 回退到“开启”，否则一次数据库抖动就可能把未经人工确认的改动推到默认分支。
fn project_auto_merge(ctx: &RunContext, project_id: &str) -> bool {
    let Ok(guard) = ctx.db.lock() else {
        return false;
    };
    guard
        .conn()
        .query_row(
            "SELECT auto_merge FROM projects WHERE id = ?1",
            params![project_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value != 0)
        .unwrap_or(false)
}

/// 读取项目是否强制要求 PR 检查；读取失败回退到 `false`（空检查列表可以通过），
/// 保持升级前的旧行为，避免一次数据库抖动改变合并判定。
fn project_require_checks(ctx: &RunContext, project_id: &str) -> bool {
    let Ok(guard) = ctx.db.lock() else {
        return false;
    };
    store::project_require_checks(guard.conn(), project_id).unwrap_or(false)
}

/// 自动合并所需的 GitHub 上下文：开关为真、项目配置了 `owner/repo`、且本次运行
/// 注入了 `GitHubOps`。任一条件不满足都回退到本地快进合并路径。
#[derive(Clone)]
struct AutoMergeConfig {
    ops: Arc<dyn crate::github::GitHubOps>,
    /// GitHub API 的 `owner/repo` 形式。
    repo: String,
    /// 项目是否要求 PR 必须带有通过的检查；为 false 时空检查列表视为通过。
    require_checks: bool,
}

fn auto_merge_config(ctx: &RunContext, project: &crate::db::Project) -> Option<AutoMergeConfig> {
    if !project_auto_merge(ctx, &project.id) {
        return None;
    }
    let ops = ctx.github.clone()?;
    let owner = project.github_owner.as_deref()?.trim();
    let repo = project.github_repo.as_deref()?.trim();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(AutoMergeConfig {
        ops,
        repo: format!("{owner}/{repo}"),
        require_checks: project_require_checks(ctx, &project.id),
    })
}

/// PR 检查轮询间隔与总超时。
const AUTO_MERGE_CHECK_INTERVAL: Duration = Duration::from_secs(10);
const AUTO_MERGE_CHECK_TIMEOUT: Duration = Duration::from_secs(600);

/// 永远不会再变绿的检查结论；出现任意一条就没必要继续轮询到超时。
const FAILED_CHECK_BUCKETS: &[&str] = &[
    "failure",
    "cancelled",
    "timed_out",
    "action_required",
    "startup_failure",
    "stale",
];

/// 检查是否已全部通过。空检查列表只有在项目未强制要求检查（`require_checks`
/// 为 false）时才算通过；要求检查的项目绝不能让无 CI 的 PR 自动合并。
fn checks_passed(checks: &[crate::github::CheckRun], require_checks: bool) -> bool {
    if checks.is_empty() {
        return !require_checks;
    }
    checks
        .iter()
        .all(|check| matches!(check.bucket.as_str(), "success" | "skipped" | "neutral"))
}

fn checks_failed_terminally(checks: &[crate::github::CheckRun]) -> bool {
    checks
        .iter()
        .any(|check| FAILED_CHECK_BUCKETS.contains(&check.bucket.as_str()))
}

/// 把检查状态整理成决策正文里的可读清单。
fn describe_checks(checks: &[crate::github::CheckRun]) -> String {
    if checks.is_empty() {
        return "（没有检查结果）".to_string();
    }
    checks
        .iter()
        .map(|check| format!("- {}: {} ({})", check.name, check.bucket, check.state))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 一次任务运行解析出的 agent：开发者执行实现，审查者输出裁决。
struct TaskAgents {
    developer: Arc<dyn AgentAdapter>,
    reviewer: Arc<dyn AgentAdapter>,
}

fn lookup_adapter(ctx: &RunContext, role: &str, id: &str) -> Result<Arc<dyn AgentAdapter>, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err(format!("未配置{role}适配器"));
    }
    ctx.adapters
        .get(id)
        .ok_or_else(|| format!("{role}适配器未注册：{id}"))
}

/// 任务级指派优先：开发者缺省为默认适配器，审查者缺省沿用开发者。
fn resolve_task_agents(ctx: &RunContext, task: &TaskRow) -> Result<TaskAgents, String> {
    let developer_id = task
        .developer_adapter
        .as_deref()
        .unwrap_or(ctx.default_agent.as_str());
    let developer = lookup_adapter(ctx, "开发者", developer_id)?;
    let reviewer_id = task.reviewer_adapter.as_deref().unwrap_or(developer_id);
    let reviewer = lookup_adapter(ctx, "审查者", reviewer_id)?;
    Ok(TaskAgents {
        developer,
        reviewer,
    })
}

pub fn run_task_blocking(ctx: &RunContext, task_id: &str) -> Result<(), String> {
    let _cancel_intent_guard = CancelIntentGuard {
        cancelled: ctx.cancelled.clone(),
        task_id: task_id.to_string(),
    };
    let (task, detail) = {
        let guard = lock_db(&ctx.db)?;
        let detail = store::get_task(guard.conn(), task_id)?;
        (detail.task.clone(), detail)
    };

    let project = {
        let guard = lock_db(&ctx.db)?;
        guard
            .get_project(&task.project_id)?
            .ok_or_else(|| "项目不存在".to_string())?
    };

    if !matches!(
        task.status,
        TaskStatus::Draft | TaskStatus::Ready | TaskStatus::Queued
    ) {
        return Err(format!(
            "任务当前状态不可执行：{:?}（需要 draft / ready / queued）",
            task.status
        ));
    }

    // 适配器在任何状态迁移 / worktree 创建前解析：未注册的指派只报配置错误，
    // 不给任务留下半启动的副作用。
    let agents = resolve_task_agents(ctx, &task)?;

    if task.status == TaskStatus::Draft {
        transition(&ctx.db, task_id, TaskStatus::Ready)?;
    }

    let repo = PathBuf::from(&project.repo_path);
    let base_sha = gitops::rev_parse(&repo, &project.default_branch)?;
    let run_id = Uuid::new_v4().to_string();
    let short = &run_id[..8];
    let branch = format!("temli/{}-{short}", short_id(&task.id));
    let worktree = ctx
        .data_dir
        .join("worktrees")
        .join(&project.id)
        .join(&run_id);
    let output_dir = ctx.data_dir.join("runs").join(&run_id);
    std::fs::create_dir_all(&output_dir).map_err(|e| e.to_string())?;
    if let Some(parent) = worktree.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    gitops::create_worktree(&repo, &worktree, &branch, &base_sha)?;

    if task.status != TaskStatus::Queued {
        transition(&ctx.db, task_id, TaskStatus::Queued)?;
    }

    {
        let guard = lock_db(&ctx.db)?;
        // 出队与真正开始之间可能已被取消（queued → draft），也可能在 worktree
        // 创建之后才被写入新的依赖边：两种情况都以库内状态为准，不能凭函数入口
        // 的快照给不满足条件的任务插入运行记录。依赖未就绪时任务保持 queued，
        // 由调度器在依赖完成后重新派发。
        let queued = store::task_status(guard.conn(), task_id)? == TaskStatus::Queued;
        let dependencies_ready = store::task_ready(guard.conn(), task_id)?;
        if !queued || !dependencies_ready {
            drop(guard);
            let _ = gitops::remove_worktree(&repo, &worktree);
            clear_cancel_intent(ctx, task_id);
            return Ok(());
        }
        store::start_run(
            guard.conn(),
            NewRun {
                id: Some(run_id.clone()),
                task_id: task_id.to_string(),
                adapter_id: agents.developer.manifest().id,
                adapter_version: Some(agents.developer.manifest().version),
                model: task.model.clone(),
                branch: Some(branch.clone()),
                worktree_path: Some(worktree.to_string_lossy().into_owned()),
                base_sha: Some(base_sha.clone()),
            },
        )?;
        store::append_event(
            guard.conn(),
            NewEvent {
                project_id: project.id.clone(),
                entity: "task".into(),
                entity_id: task_id.to_string(),
                kind: "run_started".into(),
                payload: serde_json::json!({ "run_id": run_id, "branch": branch }),
            },
        )?;
        // 与状态复查、run 记录插入同锁完成 Running 迁移：取消操作要在
        // queued → draft 迁移前拿到同一把 DB 锁，因此无法在这里插入运行。
        store::transition(guard.conn(), task_id, TaskStatus::Running)?;
    }

    let (tx, rx) = mpsc::channel::<AgentEvent>();
    let decisions: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
    let sink = event_sink(
        ctx.clone(),
        task_id.to_string(),
        project.id.clone(),
        run_id.clone(),
        tx,
        decisions.clone(),
    );

    let max_minutes = task_max_minutes(ctx, task_id);
    let prompt = {
        let guard = lock_db(&ctx.db)?;
        worker_prompt(guard.conn(), &task)
    };
    let spec = RunSpec {
        run_id: run_id.clone(),
        cwd: worktree.clone(),
        prompt,
        model: task.model.clone(),
        max_minutes: u64::from(max_minutes),
        max_cost_usd: None,
        env_allowlist: Vec::new(),
    };

    let handle = agents.developer.start(spec, sink)?;

    // 取消请求可能落在 start 的握手窗口内：此时会话尚未登记，事件流不会收到取消，
    // 必须在驱动前消费取消意图并主动终止刚启动的会话。
    if cancel_requested(ctx, task_id) {
        let _ = agents.developer.cancel(&handle);
        let _ = agents.developer.close(&handle);
        let interrupted = transition(&ctx.db, task_id, TaskStatus::Interrupted);
        close_run(ctx, &run_id, "cancelled", Some("cancelled during startup"));
        let _ = gitops::remove_worktree(&repo, &worktree);
        clear_cancel_intent(ctx, task_id);
        interrupted?;
        return Ok(());
    }

    let _cancel_guard = CancelGuard {
        cancels: ctx.cancels.clone(),
        task_id: task_id.to_string(),
    };
    ctx.cancels
        .register(task_id, agents.developer.clone(), handle.clone());
    let session_saved =
        lock_db(&ctx.db).and_then(|guard| match agents.developer.session_id(&handle) {
            Some(sid) => store::set_run_session(guard.conn(), &run_id, &sid),
            None => Ok(()),
        });
    if let Err(err) = session_saved {
        let _ = agents.developer.close(&handle);
        return Err(err);
    }

    let session = drive_session(
        ctx,
        &agents.developer,
        &handle,
        &rx,
        &decisions,
        u64::from(max_minutes).saturating_mul(60),
    );
    let _ = agents.developer.close(&handle);
    // 会话已关闭：立即注销，避免 commit/verify 期间的取消请求命中旧句柄。
    // `_cancel_guard` 仍负责其余提前返回路径。
    ctx.cancels.clear(task_id);
    match session? {
        SessionEnd::TimedOut => {
            {
                let guard = lock_db(&ctx.db)?;
                store::open_decision(
                    guard.conn(),
                    NewDecision {
                        project_id: project.id.clone(),
                        task_id: Some(task_id.to_string()),
                        run_id: Some(run_id.clone()),
                        kind: "timeout".into(),
                        prompt: format!(
                            "Agent 执行超过 {max_minutes} 分钟，已中止；请选择重试或放弃。"
                        ),
                        options: vec!["retry".into(), "abort".into()],
                    },
                )?;
            }
            transition(&ctx.db, task_id, TaskStatus::Failed)?;
            transition(&ctx.db, task_id, TaskStatus::NeedsHuman)?;
            close_run(ctx, &run_id, "failed", Some("agent timed out"));
            return Ok(());
        }
        SessionEnd::Ended(RunStatus::Completed) => {}
        SessionEnd::Ended(RunStatus::Cancelled) => {
            close_run(ctx, &run_id, "cancelled", Some("agent cancelled"));
            transition(&ctx.db, task_id, TaskStatus::Interrupted)?;
            return Ok(());
        }
        SessionEnd::Ended(RunStatus::Failed) => {
            close_run(ctx, &run_id, "failed", Some("agent did not complete"));
            transition(&ctx.db, task_id, TaskStatus::Failed)?;
            return Ok(());
        }
    }

    gitops::commit_all(&worktree, &format!("temli: {} ({short})", task.title))?;
    let mut head = gitops::head_sha(&worktree)?;
    if head == base_sha {
        close_run(ctx, &run_id, "failed", Some("agent produced no changes"));
        transition(&ctx.db, task_id, TaskStatus::Failed)?;
        return Ok(());
    }

    transition(&ctx.db, task_id, TaskStatus::Verifying)?;

    // 目标分支可能在 agent 执行期间前移：必须在收集证据前把 worktree rebase 到最新
    // 目标 tip，否则证据与最终合并内容不一致。有冲突文件且 ≤ 3、无高风险路径时
    // 换到新基线自动重排；其余情况（高风险、冲突过大、重试次数用尽）以及没有
    // 冲突文件的非冲突失败都转人工。
    let target_tip = gitops::rev_parse(&repo, &project.default_branch)?;
    let diff_base = if target_tip != base_sha {
        match gitops::rebase_onto_with_conflicts(&worktree, &target_tip) {
            Ok(()) => {
                head = gitops::head_sha(&worktree)?;
                target_tip
            }
            Err(conflict) => {
                // 两条路径都写入事件与决策，冲突文件清单不能只留在 git 输出里。
                handle_rebase_conflict(
                    ctx,
                    &project.id,
                    task_id,
                    &run_id,
                    &project.default_branch,
                    &target_tip,
                    task.attempts,
                    &conflict,
                )?;
                close_run(ctx, &run_id, "failed", Some("rebase conflict"));
                return Ok(());
            }
        }
    } else {
        base_sha
    };

    let diff = evidence::collect_diff(&worktree, &diff_base)?;
    let diff_patch_path = output_dir.join("diff.patch");
    std::fs::write(&diff_patch_path, &diff.patch).map_err(|e| e.to_string())?;
    let files_path = output_dir.join("files.json");
    let files_json = serde_json::to_vec_pretty(&diff.files).map_err(|e| e.to_string())?;
    std::fs::write(&files_path, &files_json).map_err(|e| e.to_string())?;
    {
        let guard = lock_db(&ctx.db)?;
        store::add_evidence(
            guard.conn(),
            NewEvidence {
                run_id: run_id.clone(),
                criterion_id: None,
                kind: "artifact".into(),
                cmd: Some("git diff".into()),
                exit_code: None,
                duration_ms: None,
                output_path: Some(diff_patch_path.to_string_lossy().into_owned()),
            },
        )?;
    }
    let risk = evidence::classify_risk(&diff.files);
    let test_files = evidence::test_ci_files(&diff.files);
    let has_high_risk = risk.iter().any(|flag| {
        matches!(
            flag.kind,
            RiskKind::Deleted
                | RiskKind::Dependency
                | RiskKind::Migration
                | RiskKind::Ci
                | RiskKind::Secret
        )
    });

    let acceptance = run_acceptance(ctx, &detail, &worktree, &output_dir, &run_id, "")?;
    let evidence_rows = acceptance
        .into_iter()
        .map(|result| result.row)
        .collect::<Vec<_>>();

    let review = match review_run(
        &agents.reviewer,
        &task,
        &worktree,
        &diff.patch,
        &evidence_rows,
        &test_files,
    ) {
        Ok(review) => review,
        Err(err) => {
            let guard = lock_db(&ctx.db)?;
            store::open_decision(
                guard.conn(),
                NewDecision {
                    project_id: project.id.clone(),
                    task_id: Some(task_id.to_string()),
                    run_id: Some(run_id.clone()),
                    kind: "review_failed".into(),
                    prompt: format!("审查未能完成：{err}"),
                    options: vec!["retry".into(), "abort".into()],
                },
            )?;
            drop(guard);
            transition(&ctx.db, task_id, TaskStatus::NeedsHuman)?;
            close_run(ctx, &run_id, "failed", Some("review failed"));
            return Ok(());
        }
    };

    let test_diff_unreviewed = !test_files.is_empty()
        && !test_files
            .iter()
            .all(|path| review.test_diff.iter().any(|seen| seen == path));

    let verdict = if test_diff_unreviewed {
        "needs_human".to_string()
    } else {
        review.verdict.clone()
    };

    let review_path = output_dir.join("review.json");
    {
        let guard = lock_db(&ctx.db)?;
        store::set_review(
            guard.conn(),
            NewReview {
                run_id: run_id.clone(),
                verdict: verdict.clone(),
                summary: Some(review.summary.clone()),
                risk_flags: review.risk_flags.clone(),
                test_diff: review.test_diff.clone(),
            },
        )?;
        let review_json = serde_json::json!({
            "verdict": verdict.clone(),
            "summary": review.summary.clone(),
            "risk_flags": review.risk_flags.clone(),
            "test_diff": review.test_diff.clone(),
        });
        std::fs::write(&review_path, review_json.to_string()).map_err(|e| e.to_string())?;
        store::add_evidence(
            guard.conn(),
            NewEvidence {
                run_id: run_id.clone(),
                criterion_id: None,
                kind: "review".into(),
                cmd: Some("reviewer".into()),
                exit_code: None,
                duration_ms: None,
                output_path: Some(review_path.to_string_lossy().into_owned()),
            },
        )?;
    }

    if test_diff_unreviewed {
        let missing = test_files
            .iter()
            .filter(|path| !review.test_diff.iter().any(|seen| seen == *path))
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        {
            let guard = lock_db(&ctx.db)?;
            store::open_decision(
                guard.conn(),
                NewDecision {
                    project_id: project.id.clone(),
                    task_id: Some(task_id.to_string()),
                    run_id: Some(run_id.clone()),
                    kind: "test_changes_unreviewed".into(),
                    prompt: format!("测试/CI 变更未在审查中逐项确认：{missing}"),
                    options: vec!["allow".into(), "deny".into()],
                },
            )?;
        }
        transition(&ctx.db, task_id, TaskStatus::NeedsHuman)?;
        close_run(ctx, &run_id, "failed", Some("test changes unreviewed"));
        return Ok(());
    }

    let ready = {
        let guard = lock_db(&ctx.db)?;
        store::merge_ready(guard.conn(), task_id)?
    };

    let on_default_branch = gitops::current_branch(&repo)
        .map(|name| name == project.default_branch)
        .unwrap_or(false);
    let approvals_supported = agents.developer.manifest().capabilities.approvals;
    let auto_merge = auto_merge_config(ctx, &project);

    if verdict == "pass" && ready && !has_high_risk && on_default_branch && approvals_supported {
        if let Some(auto_merge) = auto_merge {
            match auto_merge_pull_request(
                ctx,
                &auto_merge,
                &repo,
                &branch,
                &head,
                &project.default_branch,
                &task,
                &run_id,
                &review.summary,
            ) {
                Ok(pr) => {
                    let guard = lock_db(&ctx.db)?;
                    store::append_event(
                        guard.conn(),
                        NewEvent {
                            project_id: project.id.clone(),
                            entity: "task".into(),
                            entity_id: task_id.to_string(),
                            kind: "merged".into(),
                            payload: serde_json::json!({
                                "pr_number": pr.number,
                                "pr_url": pr.url,
                                "branch": branch,
                            }),
                        },
                    )?;
                    drop(guard);
                    let _ = gitops::remove_worktree(&repo, &worktree);
                    close_run(ctx, &run_id, "completed", None);
                    transition(&ctx.db, task_id, TaskStatus::Merged)?;
                    return Ok(());
                }
                Err(AutoMergeFailure::Setup(err)) => {
                    let guard = lock_db(&ctx.db)?;
                    store::open_decision(
                        guard.conn(),
                        NewDecision {
                            project_id: project.id.clone(),
                            task_id: Some(task_id.to_string()),
                            run_id: Some(run_id.clone()),
                            kind: "merge_failed".into(),
                            prompt: format!("GitHub 自动合并准备失败：{err}"),
                            options: vec!["retry".into(), "abort".into()],
                        },
                    )?;
                    drop(guard);
                    transition(&ctx.db, task_id, TaskStatus::NeedsHuman)?;
                    close_run(ctx, &run_id, "failed", Some("auto-merge setup failed"));
                    return Ok(());
                }
                Err(AutoMergeFailure::PullRequest { url, message }) => {
                    let guard = lock_db(&ctx.db)?;
                    store::open_decision(
                        guard.conn(),
                        NewDecision {
                            project_id: project.id.clone(),
                            task_id: Some(task_id.to_string()),
                            run_id: Some(run_id.clone()),
                            kind: "checks_failed".into(),
                            prompt: format!("{message}\nPR：{url}"),
                            options: vec!["retry".into(), "abort".into()],
                        },
                    )?;
                    drop(guard);
                    transition(&ctx.db, task_id, TaskStatus::NeedsHuman)?;
                    close_run(ctx, &run_id, "failed", Some("PR merge failed"));
                    return Ok(());
                }
                Err(AutoMergeFailure::Cancelled) => {
                    // 取消优先于合并：PR 保持未合并，只结束本次运行。
                    close_run(
                        ctx,
                        &run_id,
                        "cancelled",
                        Some("cancelled during PR checks"),
                    );
                    transition(&ctx.db, task_id, TaskStatus::Interrupted)?;
                    return Ok(());
                }
                Err(AutoMergeFailure::MergedLocalSync { url, message }) => {
                    // PR 已在远端合并：`pr_state` 保持 merged，本次失败只进入人工
                    // 同步流程，不提供重试合并的选项。
                    let guard = lock_db(&ctx.db)?;
                    store::open_decision(
                        guard.conn(),
                        NewDecision {
                            project_id: project.id.clone(),
                            task_id: Some(task_id.to_string()),
                            run_id: Some(run_id.clone()),
                            kind: "local_sync_failed".into(),
                            prompt: format!(
                                "{message}\nPR：{url}\nPR 已在 GitHub 合并，请勿再次触发合并；请人工同步本地默认分支后处理该决策。"
                            ),
                            options: vec!["abort".into()],
                        },
                    )?;
                    drop(guard);
                    transition(&ctx.db, task_id, TaskStatus::NeedsHuman)?;
                    close_run(
                        ctx,
                        &run_id,
                        "failed",
                        Some("PR merged but local sync failed"),
                    );
                    return Ok(());
                }
            }
        }
        match merge_and_verify(
            ctx,
            &repo,
            &worktree,
            &branch,
            &head,
            &project.default_branch,
            &detail,
            &output_dir,
            &run_id,
        ) {
            Ok(sha) => {
                let guard = lock_db(&ctx.db)?;
                store::append_event(
                    guard.conn(),
                    NewEvent {
                        project_id: project.id.clone(),
                        entity: "task".into(),
                        entity_id: task_id.to_string(),
                        kind: "merged".into(),
                        payload: serde_json::json!({ "sha": sha, "branch": branch }),
                    },
                )?;
                drop(guard);
                let _ = gitops::remove_worktree(&repo, &worktree);
                close_run(ctx, &run_id, "completed", None);
                transition(&ctx.db, task_id, TaskStatus::Merged)?;
                return Ok(());
            }
            Err(MergeFailure::Merge(err)) => {
                let guard = lock_db(&ctx.db)?;
                store::open_decision(
                    guard.conn(),
                    NewDecision {
                        project_id: project.id.clone(),
                        task_id: Some(task_id.to_string()),
                        run_id: Some(run_id.clone()),
                        kind: "merge_failed".into(),
                        prompt: format!("合并失败：{err}"),
                        options: vec!["retry".into(), "abort".into()],
                    },
                )?;
                drop(guard);
                transition(&ctx.db, task_id, TaskStatus::NeedsHuman)?;
                close_run(ctx, &run_id, "failed", Some("merge failed"));
                return Ok(());
            }
            Err(MergeFailure::PostMerge {
                message,
                rollback_error,
            }) => {
                let mut prompt = format!("合并后验收失败：{message}");
                match rollback_error {
                    Some(err) => prompt.push_str(&format!("\n{err}")),
                    None => prompt.push_str("\n已回滚到合并前提交。"),
                }
                let guard = lock_db(&ctx.db)?;
                store::open_decision(
                    guard.conn(),
                    NewDecision {
                        project_id: project.id.clone(),
                        task_id: Some(task_id.to_string()),
                        run_id: Some(run_id.clone()),
                        kind: "post_merge_failed".into(),
                        prompt,
                        options: vec!["retry".into(), "abort".into()],
                    },
                )?;
                drop(guard);
                transition(&ctx.db, task_id, TaskStatus::NeedsHuman)?;
                close_run(
                    ctx,
                    &run_id,
                    "failed",
                    Some("post-merge verification failed"),
                );
                return Ok(());
            }
            // 冲突处理与运行关闭都已在 `merge_and_verify` 内完成：任务要么已回到
            // `queued` 等待自动重排，要么已随 `rebase_failed` 决策转入 `needs_human`。
            Err(MergeFailure::Requeued) | Err(MergeFailure::RebaseFailed) => return Ok(()),
            Err(MergeFailure::PostRebaseEvidence(message)) => {
                let guard = lock_db(&ctx.db)?;
                store::open_decision(
                    guard.conn(),
                    NewDecision {
                        project_id: project.id.clone(),
                        task_id: Some(task_id.to_string()),
                        run_id: Some(run_id.clone()),
                        kind: "post_merge_failed".into(),
                        prompt: format!(
                            "目标分支前移后，rebase 到最新 tip 并在合并前重跑验收失败，未执行合并：{message}"
                        ),
                        options: vec!["retry".into(), "abort".into()],
                    },
                )?;
                drop(guard);
                transition(&ctx.db, task_id, TaskStatus::NeedsHuman)?;
                close_run(
                    ctx,
                    &run_id,
                    "failed",
                    Some("post-rebase verification failed"),
                );
                return Ok(());
            }
        }
    }

    let reason = if !approvals_supported {
        "适配器不支持审批（capabilities.approvals=false），需要人工放行".to_string()
    } else if !on_default_branch {
        format!(
            "目标仓库当前不在默认分支 {}，无法自动合并",
            project.default_branch
        )
    } else if has_high_risk {
        "检测到高风险变更（删除/依赖/迁移/CI/密钥），需要人工放行".to_string()
    } else if verdict != "pass" {
        format!("审查未通过：{}", review.summary)
    } else {
        "证据门未满足，需要人工检查".to_string()
    };
    {
        let guard = lock_db(&ctx.db)?;
        store::open_decision(
            guard.conn(),
            NewDecision {
                project_id: project.id.clone(),
                task_id: Some(task_id.to_string()),
                run_id: Some(run_id.clone()),
                kind: "evidence_gate".into(),
                prompt: reason,
                options: vec!["allow".into(), "deny".into()],
            },
        )?;
    }
    transition(&ctx.db, task_id, TaskStatus::NeedsHuman)?;
    close_run(ctx, &run_id, "failed", Some("evidence gate blocked"));
    Ok(())
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// rebase 冲突自动重排的文件数上限（`docs/data-model.md` §5）。
const MAX_REBASE_CONFLICT_FILES: usize = 3;

/// 冲突路径是否命中高风险类别。复用审查阶段的 [`evidence::classify_risk`]，保证
/// rebase 重排与证据门使用同一套路径规则；冲突文件没有删除语义，因此只关心
/// 依赖清单、数据库迁移、CI 配置与密钥四类。
fn rebase_conflict_high_risk(files: &[String]) -> bool {
    let changes = files
        .iter()
        .map(|path| evidence::FileChange {
            path: path.clone(),
            status: 'M',
        })
        .collect::<Vec<_>>();
    evidence::classify_risk(&changes).iter().any(|flag| {
        matches!(
            flag.kind,
            RiskKind::Dependency | RiskKind::Migration | RiskKind::Ci | RiskKind::Secret
        )
    })
}

/// 冲突处理结果：状态迁移与事件/决策都已写入，调用方只需关闭当前运行并按结果
/// 决定是否继续。
enum RebaseDisposition {
    /// 冲突在阈值内且仍有重试次数：任务已 `verifying -> failed -> queued`。
    Retried,
    /// 冲突过大、命中高风险路径、重试次数用尽，或 rebase 因非冲突原因失败
    /// （冲突文件为空）：决策已打开并转入 `needs_human`。
    NeedsHuman,
}

/// 统一的 rebase 冲突策略：先在任务事件里记录目标 tip、冲突文件与尝试次数，
/// 再按 `docs/data-model.md` §5 的阈值决定自动重排还是转人工。只有存在冲突文件
/// 且文件数在阈值内、无高风险路径、仍有重试次数时才自动重排；冲突文件为空
/// （rebase 因非冲突原因失败）一律转人工，不消耗自动重试次数。证据收集前的
/// rebase 与合并前的 rebase 共用本函数，保证两条路径的策略完全一致。
fn handle_rebase_conflict(
    ctx: &RunContext,
    project_id: &str,
    task_id: &str,
    run_id: &str,
    default_branch: &str,
    target_tip: &str,
    entry_attempts: u32,
    conflict: &gitops::RebaseConflict,
) -> Result<RebaseDisposition, String> {
    let guard = lock_db(&ctx.db)?;
    let current: u32 = guard
        .conn()
        .query_row(
            "SELECT attempts FROM tasks WHERE id = ?1",
            params![task_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("读取尝试次数失败：{e}"))?;
    // 正常流程中 `start_run` 已把 `tasks.attempts` 同步为本次 attempt；
    // 同时参考入口快照，兼容调度器/人工直接把 attempts 调到上限的收紧操作。
    let attempts = current.max(entry_attempts);
    // 冲突文件为空说明 rebase 是被非冲突原因（hook、损坏的基线、非法目标 ref 等）
    // 拒绝的：换基线重试没有意义，直接转人工，不能消耗任何自动重试次数。
    let retry = !conflict.files.is_empty()
        && conflict.files.len() <= MAX_REBASE_CONFLICT_FILES
        && !rebase_conflict_high_risk(&conflict.files)
        && attempts < store::MAX_ATTEMPTS;
    let kind = if retry {
        "rebase_retry"
    } else {
        "rebase_failed"
    };
    store::append_event(
        guard.conn(),
        NewEvent {
            project_id: project_id.to_string(),
            entity: "task".into(),
            entity_id: task_id.to_string(),
            kind: kind.into(),
            payload: serde_json::json!({
                "run_id": run_id,
                "target_tip": target_tip,
                "files": &conflict.files,
                "attempts": attempts,
                "message": &conflict.message,
                "auto_retry": retry,
            }),
        },
    )?;
    if retry {
        store::transition(guard.conn(), task_id, TaskStatus::Failed)?;
        store::transition(guard.conn(), task_id, TaskStatus::Queued)?;
        return Ok(RebaseDisposition::Retried);
    }
    let files = if conflict.files.is_empty() {
        "（无）".to_string()
    } else {
        conflict.files.join("\n")
    };
    store::open_decision(
        guard.conn(),
        NewDecision {
            project_id: project_id.to_string(),
            task_id: Some(task_id.to_string()),
            run_id: Some(run_id.to_string()),
            kind: "rebase_failed".into(),
            prompt: format!(
                "目标分支 {default_branch} 已前移到 {target_tip}，rebase 冲突：{}\n冲突文件（{}）：\n{files}",
                conflict.message,
                conflict.files.len(),
            ),
            options: vec!["retry".into(), "abort".into()],
        },
    )?;
    store::transition(guard.conn(), task_id, TaskStatus::NeedsHuman)?;
    Ok(RebaseDisposition::NeedsHuman)
}

/// 把默认分支快进到**被审查的那个提交**：分支必须先仍指向 `head`，合并后 HEAD 必须
/// 恰好等于 `head`。任何偏移都说明审查对象与实际合并对象不一致，调用方转人工处理。
fn merge_reviewed_head(repo: &Path, branch: &str, head: &str) -> Result<String, String> {
    let branch_sha = gitops::rev_parse(repo, branch)?;
    if branch_sha != head {
        return Err(format!(
            "分支 {branch} 已移动到 {branch_sha}，不再是被审查的 {head}"
        ));
    }
    let merged = gitops::merge_ff(repo, head)?;
    let current = gitops::head_sha(repo)?;
    if current != head {
        return Err(format!(
            "合并后 HEAD 为 {current}，与被审查的 {head} 不一致"
        ));
    }
    Ok(merged)
}

enum MergeFailure {
    /// SHA 校验或快进合并本身失败，目标分支未改变。
    Merge(String),
    /// 合并已发生但目标仓库重跑验收失败；已在 tip 未被外部移动时尝试回滚。
    PostMerge {
        message: String,
        rollback_error: Option<String>,
    },
    /// 目标 tip 在证据收集后前移且 rebase 冲突，已按阈值自动重排；运行已关闭。
    Requeued,
    /// 目标 tip 在证据收集后前移且 rebase 冲突，已打开 `rebase_failed` 决策；运行已关闭。
    RebaseFailed,
    /// rebase 到新 tip 后重跑验收失败，尚未合并；调用方需打开决策并转人工。
    PostRebaseEvidence(String),
}

/// 仅在目标分支仍停在本次合并产生的提交时才回滚；返回值作为 `rollback_error`
/// 呈现给决策收件箱（`None` 表示已成功回滚）。
///
/// 合并后到重跑验收之间可能有外部提交落到目标分支：此时硬回滚会连同别人的工作
/// 一起丢弃，必须跳过并转人工处理。
fn rollback_to_pre_merge(repo: &Path, merged: &str, pre_merge_sha: &str) -> Option<String> {
    match gitops::head_sha(repo) {
        Ok(current) if current == merged => gitops::reset_hard(repo, pre_merge_sha)
            .err()
            .map(|err| format!("回滚失败：{err}")),
        Ok(_) => Some("目标分支已被外部修改，跳过回滚".to_string()),
        Err(err) => Some(format!("无法读取目标分支 HEAD，跳过回滚：{err}")),
    }
}

/// 在合并锁内完成：校验分支仍指向被审查的 head；若目标 tip 在证据收集后前移，
/// 先把 worktree rebase 到新 tip 并在 worktree 内重跑全部验收标准（新证据写入本次
/// run），再用新 head 快进合并；合并后仍在目标仓库重跑验收，失败则尝试回滚。
///
/// 合并锁必须覆盖 rebase、重跑验收与合并本身：并发任务的合并因此串行，后到者
/// 一定会看到前一个任务推进后的 tip，从而在锁内换基线而不是快进失败。验收命令
/// 在锁内执行（每条最多 600s）是刻意取舍：宁可合并排队，也不能让“验证过的提交”
/// 与“最终合并的提交”之间的基线再次移动。
fn merge_and_verify(
    ctx: &RunContext,
    repo: &Path,
    worktree: &Path,
    branch: &str,
    head: &str,
    default_branch: &str,
    detail: &store::TaskDetail,
    output_dir: &Path,
    run_id: &str,
) -> Result<String, MergeFailure> {
    let _lock = ctx
        .merge_lock
        .lock()
        .map_err(|_| MergeFailure::Merge("合并锁被占用".to_string()))?;
    let current_branch = gitops::current_branch(repo).map_err(MergeFailure::Merge)?;
    if current_branch != default_branch {
        return Err(MergeFailure::Merge(format!(
            "目标仓库当前不在默认分支 {default_branch}（实际 {current_branch}），拒绝合并"
        )));
    }
    let mut head = head.to_string();

    // 分支必须先仍指向被审查的提交；否则 rebase 会把别人的提交也带进来。
    let branch_sha = gitops::rev_parse(repo, branch).map_err(MergeFailure::Merge)?;
    if branch_sha != head {
        return Err(MergeFailure::Merge(format!(
            "分支 {branch} 已移动到 {branch_sha}，不再是被审查的 {head}"
        )));
    }
    let pre_merge_sha = gitops::rev_parse(repo, default_branch).map_err(MergeFailure::Merge)?;

    if !gitops::is_ancestor(repo, &pre_merge_sha, &head).map_err(MergeFailure::Merge)? {
        // 证据收集完成到合并之间目标分支又前移了（并发任务先完成合并）：旧 head
        // 无法快进。先在 worktree 内换基线，成功后基于新 head 重新验证并合并。
        match gitops::rebase_onto_with_conflicts(worktree, &pre_merge_sha) {
            Ok(()) => {
                head = gitops::head_sha(worktree).map_err(MergeFailure::Merge)?;
                // 新基线可能改变验收结果（例如目标分支新增了依赖或配置），
                // 必须重跑；新证据仍挂在同一个 run 上，证据门以最新行为准。
                let outcomes =
                    run_acceptance(ctx, detail, worktree, output_dir, run_id, "merge-rebase-")
                        .map_err(MergeFailure::PostRebaseEvidence)?;
                let failures = outcomes
                    .iter()
                    .filter(|outcome| !outcome.satisfied)
                    .map(|outcome| outcome.note.clone())
                    .collect::<Vec<_>>();
                if !failures.is_empty() {
                    return Err(MergeFailure::PostRebaseEvidence(format!(
                        "rebase 到 {pre_merge_sha} 后重跑验收失败：{}",
                        failures.join("；")
                    )));
                }
            }
            Err(conflict) => {
                // 与证据收集前的 rebase 共用同一套阈值策略（含无冲突文件的非冲突
                // 失败）；状态迁移与决策已由 `handle_rebase_conflict` 写入，这里
                // 只需关闭本次运行。
                let disposition = handle_rebase_conflict(
                    ctx,
                    &detail.task.project_id,
                    &detail.task.id,
                    run_id,
                    default_branch,
                    &pre_merge_sha,
                    detail.task.attempts,
                    &conflict,
                )
                .map_err(MergeFailure::Merge)?;
                close_run(ctx, run_id, "failed", Some("rebase conflict before merge"));
                return Err(match disposition {
                    RebaseDisposition::Retried => MergeFailure::Requeued,
                    RebaseDisposition::NeedsHuman => MergeFailure::RebaseFailed,
                });
            }
        }
    }

    let merged = match merge_reviewed_head(repo, branch, &head) {
        Ok(sha) => sha,
        Err(err) => {
            // 合并过程中的失败若已把目标分支快进到被审查的 head，才回滚；
            // tip 已被外部提交前移时硬回滚会丢弃别人的工作。
            let at_reviewed_head = gitops::head_sha(repo)
                .map(|sha| sha == head)
                .unwrap_or(false);
            if at_reviewed_head {
                let _ = gitops::reset_hard(repo, &pre_merge_sha);
            }
            return Err(MergeFailure::Merge(err));
        }
    };

    let outcomes = match run_acceptance(ctx, detail, repo, output_dir, run_id, "post-merge-") {
        Ok(outcomes) => outcomes,
        Err(err) => {
            return Err(MergeFailure::PostMerge {
                message: format!("合并后无法重跑验收：{err}"),
                rollback_error: rollback_to_pre_merge(repo, &merged, &pre_merge_sha),
            });
        }
    };
    let failures = outcomes
        .iter()
        .filter(|outcome| !outcome.satisfied)
        .map(|outcome| outcome.note.clone())
        .collect::<Vec<_>>();
    if !failures.is_empty() {
        return Err(MergeFailure::PostMerge {
            message: format!("合并后验收失败：{}", failures.join("；")),
            rollback_error: rollback_to_pre_merge(repo, &merged, &pre_merge_sha),
        });
    }
    Ok(merged)
}

/// 自动合并失败的时机：PR 尚未创建（无可展示链接）、PR 已存在但未合并、
/// PR 已合并但本地同步失败（绝不能再重试合并），以及轮询期间被取消。
enum AutoMergeFailure {
    Setup(String),
    PullRequest {
        url: String,
        message: String,
    },
    /// PR 已经在远端合并：本地默认分支未能同步时只能转人工，不能把这次失败
    /// 当作可重试的合并失败（重试会把已合并的 PR 再提交一次）。
    MergedLocalSync {
        url: String,
        message: String,
    },
    /// 轮询检查期间收到取消请求：PR 尚未合并，运行按取消收尾。
    Cancelled,
}

/// 改写运行记录里的 PR 状态；调用方决定写入失败是否可重试。合并成功后的
/// `merged` 状态必须最先落库，后续本地同步失败也不能回退它。
fn mark_pr_state(
    ctx: &RunContext,
    run_id: &str,
    pr: &crate::github::PullRequest,
    state: &str,
) -> Result<(), String> {
    let guard = lock_db(&ctx.db)?;
    store::set_run_pr(guard.conn(), run_id, pr.number, &pr.url, state)
}

/// 证据门与审查均已通过后的 GitHub 自动合并：
/// 推送被审查分支 → 创建 PR → 轮询检查 → 合并 PR → 把本地默认分支快进到远端。
///
/// 推送后先确认 `origin/<branch>` 正是被审查的 `head`，确保 PR 的 head 没有被
/// 移动过的分支顶替。轮询期间一并观察取消意图：一旦取消，PR 保持未合并并结束
/// 本次运行。检查通过后的关键区（复核 head → 合并 → 写 merged → 本地快进）在
/// `merge_lock` 内串行，且合并只钉在复核过的 `head` 上。合并走 squash，默认分支
/// 上的提交是新提交，不能与 `head` 做严格 SHA 相等校验；这里改为要求拉取后默认
/// 分支确实前进了。
fn auto_merge_pull_request(
    ctx: &RunContext,
    cfg: &AutoMergeConfig,
    repo: &Path,
    branch: &str,
    head: &str,
    default_branch: &str,
    task: &TaskRow,
    run_id: &str,
    review_summary: &str,
) -> Result<crate::github::PullRequest, AutoMergeFailure> {
    let current_branch = gitops::current_branch(repo).map_err(AutoMergeFailure::Setup)?;
    if current_branch != default_branch {
        return Err(AutoMergeFailure::Setup(format!(
            "目标仓库当前不在默认分支 {default_branch}（实际 {current_branch}），拒绝自动合并"
        )));
    }
    let branch_sha = gitops::rev_parse(repo, branch).map_err(AutoMergeFailure::Setup)?;
    if branch_sha != head {
        return Err(AutoMergeFailure::Setup(format!(
            "分支 {branch} 已移动到 {branch_sha}，不再是被审查的 {head}"
        )));
    }
    gitops::push_branch(repo, branch).map_err(AutoMergeFailure::Setup)?;
    // push 会刷新 remote-tracking ref：PR 的 head 必须恰好是被审查的提交。
    let pushed = format!("origin/{branch}");
    let remote_sha = gitops::rev_parse(repo, &pushed).map_err(AutoMergeFailure::Setup)?;
    if remote_sha != head {
        return Err(AutoMergeFailure::Setup(format!(
            "远端分支 {branch} 停在 {remote_sha}，与被审查的 {head} 不一致"
        )));
    }

    let body = format!("由 temli 自动创建（run {run_id}）。\n\n审查结论：pass\n{review_summary}");
    let pr = cfg
        .ops
        .create_pull_request(&cfg.repo, branch, default_branch, &task.title, &body)
        .map_err(AutoMergeFailure::Setup)?;
    {
        let persisted = lock_db(&ctx.db).and_then(|guard| {
            store::set_run_pr(guard.conn(), run_id, pr.number, &pr.url, &pr.state)
        });
        persisted.map_err(|err| AutoMergeFailure::PullRequest {
            url: pr.url.clone(),
            message: format!("记录 PR 信息失败：{err}"),
        })?;
    }

    let deadline = Instant::now() + AUTO_MERGE_CHECK_TIMEOUT;
    loop {
        // 取消必须能在下一次轮询前生效：PR 尚未合并，直接停在当前状态。
        if cancel_requested(ctx, &task.id) {
            let _ = mark_pr_state(ctx, run_id, &pr, "cancelled");
            return Err(AutoMergeFailure::Cancelled);
        }
        let checks_result = cfg.ops.pull_request_checks(&cfg.repo, pr.number);
        // 拉取检查结果可能耗时：期间到达的取消同样要立即生效，否则会白等
        // 一个轮询间隔后再合并。
        if cancel_requested(ctx, &task.id) {
            let _ = mark_pr_state(ctx, run_id, &pr, "cancelled");
            return Err(AutoMergeFailure::Cancelled);
        }
        let checks = match checks_result {
            Ok(checks) => checks,
            Err(err) => {
                // 网络抖动不应终止自动合并：记下错误，继续轮询到超时。
                let now = Instant::now();
                if now >= deadline {
                    return Err(AutoMergeFailure::PullRequest {
                        url: pr.url.clone(),
                        message: format!("读取 PR 检查失败（重试超时）：{err}"),
                    });
                }
                std::thread::sleep((deadline - now).min(AUTO_MERGE_CHECK_INTERVAL));
                continue;
            }
        };
        if checks.is_empty() && cfg.require_checks {
            // 要求检查的项目不接受空检查列表，直接交给人工处理，而不是轮询到超时。
            return Err(AutoMergeFailure::PullRequest {
                url: pr.url.clone(),
                message:
                    "项目要求 PR 必须有通过的 CI 检查，但该 PR 没有任何检查结果，拒绝自动合并。"
                        .to_string(),
            });
        }
        if checks_passed(&checks, cfg.require_checks) {
            break;
        }
        if checks_failed_terminally(&checks) {
            return Err(AutoMergeFailure::PullRequest {
                url: pr.url.clone(),
                message: format!("PR 检查未通过：\n{}", describe_checks(&checks)),
            });
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(AutoMergeFailure::PullRequest {
                url: pr.url.clone(),
                message: format!(
                    "等待 PR 检查超时（{} 分钟）：\n{}",
                    AUTO_MERGE_CHECK_TIMEOUT.as_secs() / 60,
                    describe_checks(&checks)
                ),
            });
        }
        std::thread::sleep((deadline - now).min(AUTO_MERGE_CHECK_INTERVAL));
    }

    // 检查通过后的关键区持合并锁执行：轮询本身不持锁，避免整个检查等待期把
    // 并发任务串行化；但“复核 PR head → 合并 → 写 merged → 本地快进”必须原子，
    // 否则并发任务可能在复核与合并之间推进目标分支。
    let _lock = ctx
        .merge_lock
        .lock()
        .map_err(|_| AutoMergeFailure::PullRequest {
            url: pr.url.clone(),
            message: "合并锁被占用".to_string(),
        })?;

    // 等待检查期间分支可能被改写：合并前刷新远端并再次确认 PR head 仍是被审查
    // 的提交，避免把检查之后追加的提交一并合并。
    let checked_head = gitops::fetch_origin(repo).and_then(|()| gitops::rev_parse(repo, &pushed));
    let remote_head = checked_head.map_err(|err| AutoMergeFailure::PullRequest {
        url: pr.url.clone(),
        message: format!("合并前刷新远端分支失败：{err}"),
    })?;
    if remote_head != head {
        return Err(AutoMergeFailure::PullRequest {
            url: pr.url.clone(),
            message: format!("等待检查期间分支 {branch} 被移动到 {remote_head}，拒绝合并"),
        });
    }

    let pre_merge_sha = gitops::head_sha(repo).map_err(|err| AutoMergeFailure::PullRequest {
        url: pr.url.clone(),
        message: format!("读取本地 {default_branch} HEAD 失败：{err}"),
    })?;

    // 取消请求可能恰好落在检查通过之后、合并之前：拿到锁并复核 head 后再查一次。
    if cancel_requested(ctx, &task.id) {
        let _ = mark_pr_state(ctx, run_id, &pr, "cancelled");
        return Err(AutoMergeFailure::Cancelled);
    }

    // 合并必须钉在刚复核过的被审查 head 上，避免 checks 之后新推的提交被一并合并。
    cfg.ops
        .merge_pull_request(&cfg.repo, pr.number, &head)
        .map_err(|err| AutoMergeFailure::PullRequest {
            url: pr.url.clone(),
            message: format!("合并 PR 失败：{err}"),
        })?;

    // PR 已在远端合并：第一件事就是落 `merged` 状态。此后无论本地同步是否成功，
    // 都不能再把它当作未合并的失败去重试合并。
    if let Err(err) = mark_pr_state(ctx, run_id, &pr, "merged") {
        return Err(AutoMergeFailure::MergedLocalSync {
            url: pr.url.clone(),
            message: format!("PR 已合并，但写入 merged 状态失败：{err}"),
        });
    }

    // `git pull` 合并到当前分支：轮询期间用户若切走默认分支，绝不能把远端默认
    // 分支快进到别的本地分支上。
    let still_default = gitops::current_branch(repo)
        .map(|name| name == default_branch)
        .unwrap_or(false);
    let pulled = if still_default {
        gitops::fetch_origin(repo)
            .and_then(|()| gitops::pull_ff(repo, default_branch))
            .and_then(|()| gitops::head_sha(repo))
    } else {
        Err(format!(
            "目标仓库已不在默认分支 {default_branch}，跳过本地更新"
        ))
    };
    let after_sha = pulled.map_err(|err| AutoMergeFailure::MergedLocalSync {
        url: pr.url.clone(),
        message: format!("PR 已合并，但更新本地 {default_branch} 失败：{err}"),
    })?;
    // squash 合并会产生新的提交，无法与被审查的 head 比对；只要求目标分支前进，
    // 说明本地确实拿到了 PR 的合并结果。
    if after_sha == pre_merge_sha {
        return Err(AutoMergeFailure::MergedLocalSync {
            url: pr.url.clone(),
            message: format!("PR 已合并，但本地 {default_branch} 未前进，请人工核对"),
        });
    }
    Ok(pr)
}

/// 上下文中单项正文的最大字符数。
const CONTEXT_ITEM_CHARS: usize = 4_000;
/// 整个上下文段（含标题）的最大字符数。
const CONTEXT_SECTION_CHARS: usize = 12_000;

/// 把任务关联的知识条目渲染成 `## 项目上下文` 段。
///
/// 单项正文截断到 [`CONTEXT_ITEM_CHARS`]，整段截断到
/// [`CONTEXT_SECTION_CHARS`]；条目顺序即注入顺序。没有条目时返回空串，
/// 调用方据此保持原始 prompt 不变。
fn render_context(items: &[store::ContextItem]) -> String {
    if items.is_empty() {
        return String::new();
    }

    let mut section = String::from("## 项目上下文");
    let mut used = section.chars().count();
    for item in items {
        let body: String = item.body.chars().take(CONTEXT_ITEM_CHARS).collect();
        let entry = format!("\n\n### {}\n{}", item.title, body);
        let entry_chars = entry.chars().count();
        let remaining = CONTEXT_SECTION_CHARS.saturating_sub(used);
        if remaining == 0 {
            break;
        }
        if entry_chars > remaining {
            section.extend(entry.chars().take(remaining));
            break;
        }
        used += entry_chars;
        section.push_str(&entry);
    }
    section
}

fn worker_prompt(conn: &Connection, task: &TaskRow) -> String {
    let mut prompt = format!(
        "你在隔离的 git worktree 中开发。\n\n任务：{}\n\n{}\n\n要求：\n- 只修改完成任务所需的文件，不要触碰 .temli/。\n- 完成后运行仓库既有测试。\n- 不要执行 force push、删除仓库或修改 CI 密钥等破坏性操作。\n- 保持改动可审查、可回滚。",
        task.title, task.prompt
    );
    // 上下文是增强信息：读取失败不应阻止任务派发。
    let context = store::task_context(conn, &task.id).unwrap_or_default();
    let section = render_context(&context);
    if !section.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(&section);
    }
    prompt
}

fn event_sink(
    ctx: RunContext,
    task_id: String,
    project_id: String,
    run_id: String,
    tx: mpsc::Sender<AgentEvent>,
    decisions: Arc<Mutex<HashMap<String, String>>>,
) -> EventSink {
    let terminal = Arc::new(AtomicBool::new(false));
    Arc::new(move |event: AgentEvent| {
        if terminal.load(Ordering::SeqCst) {
            // 终态之后到达的事件（例如即发即忘的 extension_ui_request）直接丢弃：
            // 既不能新建审批决策，也不能再持久化或转发给 UI。
            return;
        }
        if matches!(&event, AgentEvent::Ended { .. }) {
            terminal.store(true, Ordering::SeqCst);
        }
        let payload = serde_json::to_value(&event).unwrap_or_else(|_| serde_json::json!({}));
        let kind = match &event {
            AgentEvent::ApprovalRequest { .. } => "approval_request",
            AgentEvent::Ended { .. } => "agent_end",
            AgentEvent::Error { .. } => "agent_error",
            _ => "agent_event",
        };
        if let Ok(guard) = ctx.db.lock() {
            if let AgentEvent::ApprovalRequest {
                request_id,
                title,
                detail,
                options,
                ..
            } = &event
            {
                if let Ok(decision) = store::open_decision(
                    guard.conn(),
                    NewDecision {
                        project_id: project_id.clone(),
                        task_id: Some(task_id.clone()),
                        run_id: Some(run_id.clone()),
                        kind: "agent_approval".into(),
                        prompt: format!("{title}\n{detail}\n请求 {request_id}"),
                        options: options.clone(),
                    },
                ) {
                    if let Ok(mut map) = decisions.lock() {
                        map.insert(request_id.clone(), decision.id);
                    }
                }
            }
            let _ = store::append_event(
                guard.conn(),
                NewEvent {
                    project_id: project_id.clone(),
                    entity: "task".into(),
                    entity_id: task_id.clone(),
                    kind: kind.into(),
                    payload,
                },
            );
        }
        let _ = tx.send(event);
    })
}

/// 驱动 worker 会话得到的终局。用户取消由适配器以 `RunStatus::Cancelled` 结束，
/// 因此与达到执行时限区分开：只有后者会触发 `timeout` 决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionEnd {
    Ended(RunStatus),
    TimedOut,
}

fn drive_session(
    ctx: &RunContext,
    adapter: &Arc<dyn AgentAdapter>,
    handle: &SessionHandle,
    rx: &Receiver<AgentEvent>,
    decisions: &Arc<Mutex<HashMap<String, String>>>,
    timeout_secs: u64,
) -> Result<SessionEnd, String> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(AgentEvent::Ended { status }) => return Ok(SessionEnd::Ended(status)),
            Ok(AgentEvent::ApprovalRequest { request_id, .. }) => {
                if !wait_for_approval(ctx, adapter, handle, &request_id, decisions, deadline)? {
                    let _ = adapter.cancel(handle);
                    return Ok(SessionEnd::TimedOut);
                }
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err("Agent 事件流中断".to_string()),
        }
        if Instant::now() >= deadline {
            let _ = adapter.cancel(handle);
            return Ok(SessionEnd::TimedOut);
        }
    }
}

/// 等待审批决策；返回 `Ok(true)` 表示已答复，`Ok(false)` 表示到达执行时限。
fn wait_for_approval(
    ctx: &RunContext,
    adapter: &Arc<dyn AgentAdapter>,
    handle: &SessionHandle,
    request_id: &str,
    decisions: &Arc<Mutex<HashMap<String, String>>>,
    deadline: Instant,
) -> Result<bool, String> {
    let decision_id = decisions
        .lock()
        .ok()
        .and_then(|map| map.get(request_id).cloned());
    let Some(decision_id) = decision_id else {
        adapter.respond(handle, request_id, Decision::Deny)?;
        return Err("审批请求未登记".to_string());
    };

    while Instant::now() < deadline {
        let resolved = {
            let guard = lock_db(&ctx.db)?;
            let row: Option<(String, Option<String>)> = guard
                .conn()
                .query_row(
                    "SELECT status, resolution FROM decisions WHERE id = ?1",
                    params![decision_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok();
            row
        };
        if let Some((status, resolution)) = resolved {
            if status == "resolved" {
                let decision = match resolution.as_deref() {
                    Some("allow") | Some("approve") | Some("retry") => Decision::Allow,
                    Some(other) => Decision::Select(other.to_string()),
                    None => Decision::Deny,
                };
                adapter.respond(handle, request_id, decision)?;
                return Ok(true);
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    let _ = adapter.respond(handle, request_id, Decision::Deny);
    Ok(false)
}

/// 单条验收标准的执行结果：证据行 + 是否满足 `expect_exit` / `expect_match`。
struct AcceptanceResult {
    row: store::EvidenceRow,
    satisfied: bool,
    note: String,
}

fn run_acceptance(
    ctx: &RunContext,
    detail: &store::TaskDetail,
    cwd: &Path,
    output_dir: &Path,
    run_id: &str,
    file_prefix: &str,
) -> Result<Vec<AcceptanceResult>, String> {
    let mut results = Vec::new();
    for criterion in &detail.criteria {
        let outcome = evidence::run_command(cwd, &criterion.cmd, 600);
        let output_path =
            output_dir.join(format!("{file_prefix}{}.log", output_stem(&criterion.id)));
        let body = format!(
            "$ {}\nexit={:?}\ntimed_out={}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            criterion.cmd, outcome.exit_code, outcome.timed_out, outcome.stdout, outcome.stderr
        );
        std::fs::write(&output_path, &body).map_err(|e| e.to_string())?;

        let mut notes = Vec::new();
        if outcome.exit_code != Some(criterion.expect_exit) {
            notes.push(format!(
                "exit {:?}（期望 {}）",
                outcome.exit_code, criterion.expect_exit
            ));
        }
        if let Some(fragment) = criterion.expect_match.as_deref() {
            if !body.contains(fragment) {
                notes.push(format!("输出未包含 {fragment:?}"));
            }
        }
        if outcome.timed_out {
            notes.push("执行超时".to_string());
        }
        let satisfied = notes.is_empty();
        let note = if satisfied {
            format!("{} 通过", criterion.id)
        } else {
            format!("{}：{}", criterion.id, notes.join("；"))
        };

        let guard = lock_db(&ctx.db)?;
        let row = store::add_evidence(
            guard.conn(),
            NewEvidence {
                run_id: run_id.to_string(),
                criterion_id: Some(criterion.id.clone()),
                kind: criterion_kind(&criterion.cmd).into(),
                cmd: Some(criterion.cmd.clone()),
                exit_code: outcome.exit_code,
                duration_ms: Some(outcome.duration_ms as i64),
                output_path: Some(output_path.to_string_lossy().into_owned()),
            },
        )?;
        results.push(AcceptanceResult {
            row,
            satisfied,
            note,
        });
    }
    Ok(results)
}

/// 证据输出文件名只允许 `[A-Za-z0-9_-]`，其余字符一律替换为 `_`。
fn output_stem(criterion_id: &str) -> String {
    let stem: String = criterion_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if stem.is_empty() {
        "criterion".to_string()
    } else {
        stem
    }
}

fn criterion_kind(cmd: &str) -> &'static str {
    if cmd.contains("lint") || cmd.contains("clippy") {
        "lint"
    } else if cmd.contains("test") {
        "test"
    } else {
        "build"
    }
}

fn review_run(
    reviewer: &Arc<dyn AgentAdapter>,
    task: &TaskRow,
    worktree: &Path,
    patch: &str,
    evidence_rows: &[store::EvidenceRow],
    test_files: &[String],
) -> Result<prompts::ReviewVerdict, String> {
    let summary = evidence_rows
        .iter()
        .map(|row| {
            format!(
                "{} {} -> exit {:?}",
                row.kind,
                row.cmd.as_deref().unwrap_or(""),
                row.exit_code
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = prompts::reviewer_prompt(&task.title, patch, &summary, test_files);
    let (tx, rx) = mpsc::channel::<AgentEvent>();
    let sink: EventSink = Arc::new(move |event| {
        let _ = tx.send(event);
    });
    let spec = RunSpec {
        run_id: Uuid::new_v4().to_string(),
        cwd: worktree.to_path_buf(),
        prompt,
        // 审查角色沿用任务的模型覆盖；后续如需角色级模型再从独立字段读取。
        model: task.model.clone(),
        max_minutes: 10,
        max_cost_usd: None,
        env_allowlist: Vec::new(),
    };
    let handle = reviewer.start(spec, sink)?;
    let collected = drive_reviewer(reviewer, &handle, &rx);
    let _ = reviewer.close(&handle);
    let text = collected?;
    prompts::parse_review(&text)
}

/// 只接受 `Ended { status: Completed }` 作为成功终态：其余终态、断流、超时都是失败，
/// 由调用方转入 needs_human，绝不让未完成的审查被当成通过。
fn drive_reviewer(
    reviewer: &Arc<dyn AgentAdapter>,
    handle: &SessionHandle,
    rx: &Receiver<AgentEvent>,
) -> Result<String, String> {
    let mut text = String::new();
    let deadline = Instant::now() + Duration::from_secs(10 * 60);
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(AgentEvent::TextDelta { text: delta }) => text.push_str(&delta),
            Ok(AgentEvent::ApprovalRequest { request_id, .. }) => {
                let _ = reviewer.respond(handle, &request_id, Decision::Deny);
            }
            Ok(AgentEvent::Ended {
                status: RunStatus::Completed,
            }) => return Ok(text),
            Ok(AgentEvent::Ended { status }) => {
                return Err(format!("审查会话未正常完成：{status:?}"));
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err("审查会话事件流中断".to_string());
            }
        }
        if Instant::now() >= deadline {
            let _ = reviewer.cancel(handle);
            return Err("审查超时".to_string());
        }
    }
}

fn close_run(ctx: &RunContext, run_id: &str, status: &str, error: Option<&str>) {
    if let Ok(guard) = ctx.db.lock() {
        let _ = store::finish_run(guard.conn(), run_id, status, error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{
        AdapterRegistry, AgentManifest, AgentRole, AuthState, Capabilities, DetectResult, EventSink,
    };
    use crate::github::{CheckRun, PullRequest};
    use crate::store::{Criterion, NewRun, NewTask};
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn test_manifest(id: &str) -> AgentManifest {
        AgentManifest {
            id: id.into(),
            name: "Scripted".into(),
            version: "0.0.1".into(),
            roles: vec![AgentRole::Developer, AgentRole::Reviewer],
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

    fn ok_detect() -> DetectResult {
        DetectResult {
            available: true,
            version: Some("0.0.1".into()),
            auth: AuthState::Ok,
            detail: None,
        }
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} 失败：{}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Worker 写入 `done.txt`；reviewer 会话输出一条通过的审查 JSON。
    struct ScriptedAdapter;

    impl AgentAdapter for ScriptedAdapter {
        fn manifest(&self) -> AgentManifest {
            test_manifest("scripted")
        }

        fn detect(&self) -> DetectResult {
            ok_detect()
        }

        fn start(&self, spec: RunSpec, sink: EventSink) -> Result<SessionHandle, String> {
            fs::write(spec.cwd.join("done.txt"), "ok\n").map_err(|e| e.to_string())?;
            sink(AgentEvent::TextDelta {
                text: r#"{"verdict":"pass","summary":"looks good","risk_flags":[],"test_diff":[]}"#
                    .into(),
            });
            sink(AgentEvent::Ended {
                status: RunStatus::Completed,
            });
            Ok(SessionHandle {
                id: "scripted-1".into(),
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

    /// 只执行开发脚本：写 `done.txt`；被要求审查时输出带自身标记的通过裁决。
    struct DeveloperFake {
        starts: AtomicUsize,
        reviewer_starts: AtomicUsize,
    }

    impl DeveloperFake {
        fn new() -> Self {
            Self {
                starts: AtomicUsize::new(0),
                reviewer_starts: AtomicUsize::new(0),
            }
        }

        fn starts(&self) -> usize {
            self.starts.load(Ordering::SeqCst)
        }

        fn reviewer_starts(&self) -> usize {
            self.reviewer_starts.load(Ordering::SeqCst)
        }
    }

    impl AgentAdapter for DeveloperFake {
        fn manifest(&self) -> AgentManifest {
            test_manifest("dev-fake")
        }

        fn detect(&self) -> DetectResult {
            ok_detect()
        }

        fn start(&self, spec: RunSpec, sink: EventSink) -> Result<SessionHandle, String> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            if spec.prompt.contains("verdict") {
                self.reviewer_starts.fetch_add(1, Ordering::SeqCst);
                sink(AgentEvent::TextDelta {
                    text: r#"{"verdict":"pass","summary":"reviewed by dev-fake","risk_flags":[],"test_diff":[]}"#
                        .into(),
                });
                sink(AgentEvent::Ended {
                    status: RunStatus::Completed,
                });
                return Ok(SessionHandle {
                    id: "dev-fake-review-1".into(),
                });
            }
            fs::write(spec.cwd.join("done.txt"), "ok\n").map_err(|e| e.to_string())?;
            sink(AgentEvent::Ended {
                status: RunStatus::Completed,
            });
            Ok(SessionHandle {
                id: "dev-fake-1".into(),
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

    /// 只执行审查脚本：输出带自身标记的通过裁决。
    struct ReviewerFake {
        starts: AtomicUsize,
    }

    impl ReviewerFake {
        fn new() -> Self {
            Self {
                starts: AtomicUsize::new(0),
            }
        }

        fn starts(&self) -> usize {
            self.starts.load(Ordering::SeqCst)
        }
    }

    impl AgentAdapter for ReviewerFake {
        fn manifest(&self) -> AgentManifest {
            test_manifest("review-fake")
        }

        fn detect(&self) -> DetectResult {
            ok_detect()
        }

        fn start(&self, _spec: RunSpec, sink: EventSink) -> Result<SessionHandle, String> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            sink(AgentEvent::TextDelta {
                text: r#"{"verdict":"pass","summary":"reviewed by review-fake","risk_flags":[],"test_diff":[]}"#
                    .into(),
            });
            sink(AgentEvent::Ended {
                status: RunStatus::Completed,
            });
            Ok(SessionHandle {
                id: "review-fake-1".into(),
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

    /// 记录每次启动收到的 `RunSpec.model`，worker / reviewer 共用同一脚本。
    struct ModelRecordingAdapter {
        models: Mutex<Vec<Option<String>>>,
    }

    impl ModelRecordingAdapter {
        fn new() -> Self {
            Self {
                models: Mutex::new(Vec::new()),
            }
        }

        fn models(&self) -> Vec<Option<String>> {
            self.models.lock().unwrap().clone()
        }
    }

    impl AgentAdapter for ModelRecordingAdapter {
        fn manifest(&self) -> AgentManifest {
            test_manifest("model-recorder")
        }

        fn detect(&self) -> DetectResult {
            ok_detect()
        }

        fn start(&self, spec: RunSpec, sink: EventSink) -> Result<SessionHandle, String> {
            self.models.lock().unwrap().push(spec.model.clone());
            fs::write(spec.cwd.join("done.txt"), "ok\n").map_err(|e| e.to_string())?;
            sink(AgentEvent::TextDelta {
                text: r#"{"verdict":"pass","summary":"model recorded","risk_flags":[],"test_diff":[]}"#
                    .into(),
            });
            sink(AgentEvent::Ended {
                status: RunStatus::Completed,
            });
            Ok(SessionHandle {
                id: "model-recorder-1".into(),
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

    /// 持有事件通道但永不发出 `Ended`：只有 orchestrator 的执行时限能结束该会话。
    #[derive(Default)]
    struct SilentAdapter {
        _sinks: Mutex<Vec<EventSink>>,
    }

    impl AgentAdapter for SilentAdapter {
        fn manifest(&self) -> AgentManifest {
            test_manifest("silent")
        }

        fn detect(&self) -> DetectResult {
            ok_detect()
        }

        fn start(&self, _spec: RunSpec, sink: EventSink) -> Result<SessionHandle, String> {
            self._sinks.lock().unwrap().push(sink);
            Ok(SessionHandle {
                id: "silent-1".into(),
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

    /// 在 `start` 中让默认分支与 worktree 改动同一批文件：worktree 基于旧提交，
    /// 默认分支已前移，orchestrator 的 rebase 必然冲突。
    struct ConflictingAdapter {
        repo: PathBuf,
        files: Vec<&'static str>,
    }

    impl AgentAdapter for ConflictingAdapter {
        fn manifest(&self) -> AgentManifest {
            test_manifest("conflicting")
        }

        fn detect(&self) -> DetectResult {
            ok_detect()
        }

        fn start(&self, spec: RunSpec, sink: EventSink) -> Result<SessionHandle, String> {
            for path in &self.files {
                for (root, side) in [(&spec.cwd, "agent"), (&self.repo, "main")] {
                    let dest = root.join(path);
                    if let Some(parent) = dest.parent() {
                        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    fs::write(&dest, format!("{side} {path}\n")).map_err(|e| e.to_string())?;
                }
            }
            run_git(&self.repo, &["add", "-A"]);
            run_git(
                &self.repo,
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "-q",
                    "-m",
                    "advance main",
                ],
            );
            sink(AgentEvent::Ended {
                status: RunStatus::Completed,
            });
            Ok(SessionHandle {
                id: "conflicting-1".into(),
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

    /// Worker 正常写 `done.txt`；reviewer 启动时（证据已收集、审查尚未落库）在默认
    /// 分支提交一个独立文件，模拟并发任务在证据收集后完成合并。合并前必须 rebase
    /// 到新 tip 才能快进，且该 rebase 无冲突。
    struct MainAdvancingAdapter {
        repo: PathBuf,
    }

    impl AgentAdapter for MainAdvancingAdapter {
        fn manifest(&self) -> AgentManifest {
            test_manifest("advancing")
        }

        fn detect(&self) -> DetectResult {
            ok_detect()
        }

        fn start(&self, spec: RunSpec, sink: EventSink) -> Result<SessionHandle, String> {
            if spec.prompt.contains("independent reviewer") {
                fs::write(self.repo.join("main.txt"), "main\n").map_err(|e| e.to_string())?;
                run_git(&self.repo, &["add", "-A"]);
                run_git(
                    &self.repo,
                    &[
                        "-c",
                        "user.name=t",
                        "-c",
                        "user.email=t@t",
                        "commit",
                        "-q",
                        "-m",
                        "advance main after evidence",
                    ],
                );
            } else {
                fs::write(spec.cwd.join("done.txt"), "ok\n").map_err(|e| e.to_string())?;
            }
            sink(AgentEvent::TextDelta {
                text: r#"{"verdict":"pass","summary":"looks good","risk_flags":[],"test_diff":[]}"#
                    .into(),
            });
            sink(AgentEvent::Ended {
                status: RunStatus::Completed,
            });
            Ok(SessionHandle {
                id: "advancing-1".into(),
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

    /// Worker 修改 `files`；reviewer 启动时（证据已收集）在默认分支提交同样的文件，
    /// 制造只会在合并前 rebase 阶段暴露的冲突。
    struct MergeTimeConflictAdapter {
        repo: PathBuf,
        files: Vec<&'static str>,
    }

    impl AgentAdapter for MergeTimeConflictAdapter {
        fn manifest(&self) -> AgentManifest {
            test_manifest("merge-time-conflict")
        }

        fn detect(&self) -> DetectResult {
            ok_detect()
        }

        fn start(&self, spec: RunSpec, sink: EventSink) -> Result<SessionHandle, String> {
            if spec.prompt.contains("independent reviewer") {
                for path in &self.files {
                    fs::write(self.repo.join(path), format!("main {path}\n"))
                        .map_err(|e| e.to_string())?;
                }
                run_git(&self.repo, &["add", "-A"]);
                run_git(
                    &self.repo,
                    &[
                        "-c",
                        "user.name=t",
                        "-c",
                        "user.email=t@t",
                        "commit",
                        "-q",
                        "-m",
                        "advance main after evidence",
                    ],
                );
            } else {
                for path in &self.files {
                    let dest = spec.cwd.join(path);
                    if let Some(parent) = dest.parent() {
                        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    fs::write(&dest, format!("agent {path}\n")).map_err(|e| e.to_string())?;
                }
            }
            sink(AgentEvent::TextDelta {
                text: r#"{"verdict":"pass","summary":"looks good","risk_flags":[],"test_diff":[]}"#
                    .into(),
            });
            sink(AgentEvent::Ended {
                status: RunStatus::Completed,
            });
            Ok(SessionHandle {
                id: "merge-time-conflict-1".into(),
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

    /// 只覆盖自动合并用到的 GitHub 操作；其余方法在测试里不会被调用。
    struct FakeGitHubOps {
        checks: Mutex<Vec<CheckRun>>,
        created: Mutex<Vec<(String, String)>>,
        /// 每次合并请求的 (PR 编号, 钉住的 head SHA)。
        merged: Mutex<Vec<(u64, String)>>,
        calls: Mutex<Vec<&'static str>>,
        /// 模拟 squash 合并的仓库：把分支压到 main 并推回 origin，供本地
        /// `fetch + pull --ff-only` 验证。
        repo: Mutex<Option<PathBuf>>,
        /// 首次读取检查时写入取消意图，模拟轮询期间的取消请求。
        cancel_on_checks: Mutex<Option<(Arc<Mutex<HashSet<String>>>, String)>>,
        /// 远端合并成功后把本地默认分支改成分叉提交，模拟本地同步失败。
        diverge_local: Mutex<bool>,
    }

    impl FakeGitHubOps {
        fn new(checks: Vec<CheckRun>) -> Self {
            Self {
                checks: Mutex::new(checks),
                created: Mutex::new(Vec::new()),
                merged: Mutex::new(Vec::new()),
                calls: Mutex::new(Vec::new()),
                repo: Mutex::new(None),
                cancel_on_checks: Mutex::new(None),
                diverge_local: Mutex::new(false),
            }
        }

        fn simulate_merge_in(&self, repo: PathBuf) {
            *self.repo.lock().unwrap() = Some(repo);
        }

        fn cancel_on_checks(&self, cancelled: Arc<Mutex<HashSet<String>>>, task_id: String) {
            *self.cancel_on_checks.lock().unwrap() = Some((cancelled, task_id));
        }

        fn diverge_local_after_merge(&self) {
            *self.diverge_local.lock().unwrap() = true;
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl crate::github::GitHubOps for FakeGitHubOps {
        fn list_issues(
            &self,
            _repo: &str,
            _etag: Option<&str>,
        ) -> Result<crate::github::IssuePage, String> {
            Err("fixture 未实现 list_issues".into())
        }

        fn get_issue(
            &self,
            _repo: &str,
            _number: u64,
        ) -> Result<crate::github::RemoteIssue, String> {
            Err("fixture 未实现 get_issue".into())
        }

        fn update_issue(
            &self,
            _repo: &str,
            _number: u64,
            _patch: &crate::github::IssuePatch,
        ) -> Result<crate::github::RemoteIssue, String> {
            Err("fixture 未实现 update_issue".into())
        }

        fn create_issue(
            &self,
            _repo: &str,
            _title: &str,
            _body: &str,
        ) -> Result<crate::github::RemoteIssue, String> {
            Err("fixture 未实现 create_issue".into())
        }

        fn create_pull_request(
            &self,
            repo: &str,
            head: &str,
            base: &str,
            _title: &str,
            _body: &str,
        ) -> Result<PullRequest, String> {
            assert_eq!(repo, "acme/demo");
            self.calls.lock().unwrap().push("create_pull_request");
            self.created
                .lock()
                .unwrap()
                .push((head.to_string(), base.to_string()));
            Ok(PullRequest {
                number: 42,
                url: "https://github.com/acme/demo/pull/42".into(),
                state: "open".into(),
            })
        }

        fn pull_request_checks(&self, _repo: &str, _number: u64) -> Result<Vec<CheckRun>, String> {
            self.calls.lock().unwrap().push("pull_request_checks");
            let cancel = self.cancel_on_checks.lock().unwrap().take();
            if let Some((cancelled, task_id)) = cancel {
                cancelled.lock().unwrap().insert(task_id);
            }
            Ok(self.checks.lock().unwrap().clone())
        }

        fn merge_pull_request(&self, _repo: &str, number: u64, sha: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push("merge_pull_request");
            self.merged.lock().unwrap().push((number, sha.to_string()));
            if let Some(repo) = self.repo.lock().unwrap().clone() {
                let head = self
                    .created
                    .lock()
                    .unwrap()
                    .last()
                    .map(|(head, _)| head.clone())
                    .ok_or_else(|| "没有待合并的 PR".to_string())?;
                let pre_merge = gitops::rev_parse(&repo, "HEAD")?;
                run_git(&repo, &["merge", "--squash", head.as_str()]);
                run_git(
                    &repo,
                    &[
                        "-c",
                        "user.name=t",
                        "-c",
                        "user.email=t@t",
                        "commit",
                        "-q",
                        "-m",
                        "squash merge",
                    ],
                );
                run_git(&repo, &["push", "-q", "origin", "main"]);
                if *self.diverge_local.lock().unwrap() {
                    // 远端已前进，本地却回到合并前再提交一次：`pull --ff-only`
                    // 必然失败，但 PR 确实已经合并。
                    run_git(&repo, &["reset", "--hard", &pre_merge]);
                    run_git(
                        &repo,
                        &[
                            "-c",
                            "user.name=t",
                            "-c",
                            "user.email=t@t",
                            "commit",
                            "--allow-empty",
                            "-q",
                            "-m",
                            "local only",
                        ],
                    );
                }
            }
            Ok(())
        }
    }

    fn temp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "temli-orch-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn init_repo() -> PathBuf {
        let repo = temp_dir().join("repo");
        fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);
        fs::write(repo.join(".gitignore"), ".temli/\n").unwrap();
        fs::write(repo.join("README.md"), "demo\n").unwrap();
        run_git(&repo, &["add", "-A"]);
        run_git(
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
        repo
    }

    fn setup(cmd: &str) -> (RunContext, String) {
        setup_with(cmd, |_| Arc::new(ScriptedAdapter))
    }

    fn setup_with(
        cmd: &str,
        adapter: impl FnOnce(&Path) -> Arc<dyn AgentAdapter>,
    ) -> (RunContext, String) {
        setup_full(cmd, adapter, None, false)
    }

    /// 自动合并夹具：`ops` 为 Some 时项目写入 `acme/demo`；`auto_merge` 为真时
    /// 额外建立 bare origin，保证 push / fetch / pull 可执行。
    fn setup_full(
        cmd: &str,
        adapter: impl FnOnce(&Path) -> Arc<dyn AgentAdapter>,
        ops: Option<Arc<dyn crate::github::GitHubOps>>,
        auto_merge: bool,
    ) -> (RunContext, String) {
        setup_agents(cmd, |repo| vec![adapter(repo)], ops, auto_merge)
    }

    /// 注册多个适配器的夹具；第一个适配器同时作为默认值。
    fn setup_agents(
        cmd: &str,
        adapters: impl FnOnce(&Path) -> Vec<Arc<dyn AgentAdapter>>,
        ops: Option<Arc<dyn crate::github::GitHubOps>>,
        auto_merge: bool,
    ) -> (RunContext, String) {
        let repo = init_repo();
        if auto_merge {
            let remote = repo.with_extension("remote.git");
            let remote_path = remote.to_string_lossy().into_owned();
            run_git(&repo, &["init", "-q", "--bare", &remote_path]);
            run_git(&repo, &["remote", "add", "origin", &remote_path]);
            run_git(&repo, &["push", "-q", "-u", "origin", "main"]);
        }
        let db_dir = temp_dir();
        let db = Db::open(&db_dir.join("temli.db")).unwrap();
        let project = db
            .insert_project(
                crate::db::RegisterProjectInput {
                    repo_path: repo.to_string_lossy().into_owned(),
                    name: Some("demo".into()),
                    github_owner: ops.as_ref().map(|_| "acme".into()),
                    github_repo: ops.as_ref().map(|_| "demo".into()),
                    default_branch: Some("main".into()),
                },
                repo.canonicalize().unwrap(),
            )
            .unwrap();
        if auto_merge {
            db.conn()
                .execute(
                    "UPDATE projects SET auto_merge = 1 WHERE id = ?1",
                    rusqlite::params![project.id],
                )
                .unwrap();
        }
        let task = store::create_task(
            db.conn(),
            NewTask {
                project_id: project.id,
                title: "fixture task".into(),
                prompt: "write done.txt".into(),
                criteria: vec![Criterion {
                    id: "c-1".into(),
                    cmd: cmd.into(),
                    expect_exit: 0,
                    expect_match: None,
                }],
            },
        )
        .unwrap();
        let built = adapters(&repo);
        let default_agent = built
            .first()
            .map(|adapter| adapter.manifest().id)
            .expect("至少提供一个适配器");
        let mut registry = AdapterRegistry::new();
        for adapter in built {
            registry.register(adapter);
        }
        let ctx = RunContext {
            db: Arc::new(Mutex::new(db)),
            adapters: Arc::new(registry),
            default_agent,
            data_dir: db_dir.join("data"),
            running: Arc::new(Mutex::new(std::collections::HashSet::new())),
            cancels: CancelRegistry::new(),
            merge_lock: Arc::new(Mutex::new(())),
            cancelled: Arc::new(Mutex::new(HashSet::new())),
            github: ops,
        };
        (ctx, task.id)
    }

    fn repo_of(ctx: &RunContext, task_id: &str) -> PathBuf {
        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), task_id).unwrap();
        let project = guard
            .get_project(&detail.task.project_id)
            .unwrap()
            .expect("project");
        PathBuf::from(project.repo_path)
    }

    fn status(ctx: &RunContext, task_id: &str) -> TaskStatus {
        let guard = ctx.db.lock().unwrap();
        store::get_task(guard.conn(), task_id).unwrap().task.status
    }

    fn set_require_checks(ctx: &RunContext, task_id: &str, enabled: bool) {
        let project_id = {
            let guard = ctx.db.lock().unwrap();
            store::get_task(guard.conn(), task_id)
                .unwrap()
                .task
                .project_id
        };
        let guard = ctx.db.lock().unwrap();
        store::set_project_require_checks(guard.conn(), &project_id, enabled).unwrap();
    }

    #[test]
    fn merges_when_evidence_and_review_pass() {
        let (ctx, task_id) = setup("test -f done.txt");
        run_task_blocking(&ctx, &task_id).unwrap();
        assert_eq!(status(&ctx, &task_id), TaskStatus::Merged);

        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        let criterion = detail
            .evidence
            .iter()
            .find(|row| row.criterion_id.as_deref() == Some("c-1"))
            .expect("criterion evidence");
        assert_eq!(criterion.exit_code, Some(0));
        assert_eq!(
            detail
                .evidence
                .iter()
                .filter(|row| row.kind == "artifact")
                .count(),
            1
        );
        assert_eq!(
            detail
                .evidence
                .iter()
                .filter(|row| row.kind == "review")
                .count(),
            1
        );
        assert_eq!(detail.review.unwrap().verdict, "pass");
        assert!(detail.decisions.is_empty());
    }

    /// 依赖边在 worktree 创建之后才落库（这里用已存在的未合并前置任务验证同一个
    /// 检查点）：不得插入运行，任务保持 queued，worktree 被清理且 attempts 不变。
    #[test]
    fn queued_task_with_unmerged_dependency_does_not_start() {
        let (ctx, task_id) = setup("true");
        let guard = ctx.db.lock().unwrap();
        let project_id = store::get_task(guard.conn(), &task_id)
            .unwrap()
            .task
            .project_id;
        let blocker = store::create_task(
            guard.conn(),
            NewTask {
                project_id,
                title: "blocker".into(),
                prompt: String::new(),
                criteria: Vec::new(),
            },
        )
        .unwrap();
        store::add_dependency(guard.conn(), &task_id, &blocker.id).unwrap();
        drop(guard);

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::Queued);
        let repo = repo_of(&ctx, &task_id);
        assert_eq!(
            gitops::list_worktrees(&repo).unwrap().len(),
            1,
            "未就绪任务不得留下 worktree"
        );
        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        assert_eq!(detail.task.attempts, 0);
        assert!(detail.runs.is_empty(), "未就绪任务不得插入运行");
        assert!(
            !detail.events.iter().any(|row| row.kind == "run_started"),
            "未就绪任务不得记录 run_started"
        );
    }

    /// 证据收集完成后目标分支才被并发任务推进：合并前必须在锁内 rebase 到新 tip、
    /// 在 worktree 重跑验收，再用新 head 快进，而不是落到 `merge_failed`。
    #[test]
    fn rebases_before_merge_when_target_tip_moved_after_evidence() {
        let (ctx, task_id) = setup_with("test -f done.txt", |repo| {
            Arc::new(MainAdvancingAdapter {
                repo: repo.to_path_buf(),
            })
        });
        let repo = repo_of(&ctx, &task_id);
        let base = gitops::rev_parse(&repo, "main").unwrap();

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::Merged);
        // 并发任务提交的 main.txt 与本任务的 done.txt 必须同时出现在 main 上。
        assert_eq!(fs::read_to_string(repo.join("main.txt")).unwrap(), "main\n");
        assert_eq!(fs::read_to_string(repo.join("done.txt")).unwrap(), "ok\n");
        let head = gitops::head_sha(&repo).unwrap();
        assert_ne!(head, base);
        assert!(gitops::is_ancestor(&repo, &base, &head).unwrap());

        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        assert!(
            !detail
                .decisions
                .iter()
                .any(|row| row.kind == "merge_failed"),
            "decisions: {:?}",
            detail
                .decisions
                .iter()
                .map(|row| row.kind.as_str())
                .collect::<Vec<_>>()
        );
        let rebased = detail
            .evidence
            .iter()
            .find(|row| {
                row.criterion_id.as_deref() == Some("c-1")
                    && row
                        .output_path
                        .as_deref()
                        .is_some_and(|path| path.ends_with("merge-rebase-c-1.log"))
            })
            .expect("rebase 后重跑的验收证据");
        assert_eq!(rebased.exit_code, Some(0));
    }

    /// 证据与审查都通过后目标分支才前移，且改动与任务冲突：合并前的 rebase 冲突
    /// 必须走与证据收集前相同的阈值策略——单文件低风险自动重排，不产生决策。
    #[test]
    fn requeues_merge_time_rebase_conflict() {
        let (ctx, task_id) = setup_with("true", |repo| {
            Arc::new(MergeTimeConflictAdapter {
                repo: repo.to_path_buf(),
                files: vec!["README.md"],
            })
        });
        let repo = repo_of(&ctx, &task_id);
        let before = gitops::rev_parse(&repo, "main").unwrap();

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::Queued);
        assert_ne!(
            gitops::rev_parse(&repo, "main").unwrap(),
            before,
            "并发任务的提交必须保留在默认分支上"
        );

        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        assert!(
            detail.decisions.is_empty(),
            "decisions: {:?}",
            detail
                .decisions
                .iter()
                .map(|row| row.kind.as_str())
                .collect::<Vec<_>>()
        );
        let event = detail
            .events
            .iter()
            .find(|row| row.kind == "rebase_retry")
            .expect("rebase_retry event");
        let payload: serde_json::Value = serde_json::from_str(&event.payload_json).unwrap();
        assert_eq!(payload["files"], serde_json::json!(["README.md"]));
        assert_eq!(payload["auto_retry"], serde_json::json!(true));
    }

    #[test]
    fn blocks_on_failed_evidence_and_opens_decision() {
        let (ctx, task_id) = setup("false");
        run_task_blocking(&ctx, &task_id).unwrap();
        assert_eq!(status(&ctx, &task_id), TaskStatus::NeedsHuman);

        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        let criterion = detail
            .evidence
            .iter()
            .find(|row| row.criterion_id.as_deref() == Some("c-1"))
            .expect("criterion evidence");
        assert_eq!(criterion.exit_code, Some(1));
        assert_eq!(detail.decisions.len(), 1);
        assert_eq!(detail.decisions[0].status, "open");
    }

    #[test]
    fn rolls_back_merge_when_post_merge_verification_fails() {
        let (ctx, task_id) = setup("test ! -d .temli");
        let repo = repo_of(&ctx, &task_id);
        // `.temli/` 被 gitignore：worktree 检出时不存在，目标仓库里必然存在，
        // 因此该标准在合并前通过、合并后重跑必然失败。
        fs::create_dir_all(repo.join(".temli")).unwrap();
        let pre_merge = gitops::rev_parse(&repo, "main").unwrap();

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::NeedsHuman);
        assert_eq!(gitops::rev_parse(&repo, "main").unwrap(), pre_merge);
        assert_eq!(gitops::head_sha(&repo).unwrap(), pre_merge);

        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        let decision = detail
            .decisions
            .iter()
            .find(|row| row.kind == "post_merge_failed")
            .expect("post_merge_failed decision");
        assert_eq!(decision.status, "open");
    }

    /// 冲突文件 ≤ 3 且无高风险路径：换到新基线自动重排，不产生决策。
    #[test]
    fn requeues_small_rebase_conflict_without_decision() {
        let (ctx, task_id) = setup_with("true", |repo| {
            Arc::new(ConflictingAdapter {
                repo: repo.to_path_buf(),
                files: vec!["README.md"],
            })
        });
        let repo = repo_of(&ctx, &task_id);
        let before = gitops::rev_parse(&repo, "main").unwrap();

        run_task_blocking(&ctx, &task_id).unwrap();

        // 适配器在 worktree 创建之后前移了默认分支：单文件低风险冲突应直接重排。
        assert_eq!(status(&ctx, &task_id), TaskStatus::Queued);
        assert_ne!(gitops::rev_parse(&repo, "main").unwrap(), before);

        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        assert!(detail.review.is_none());
        assert!(
            detail.decisions.is_empty(),
            "decisions: {:?}",
            detail
                .decisions
                .iter()
                .map(|row| row.kind.as_str())
                .collect::<Vec<_>>()
        );
        let event = detail
            .events
            .iter()
            .find(|row| row.kind == "rebase_retry")
            .expect("rebase_retry event");
        let payload: serde_json::Value = serde_json::from_str(&event.payload_json).unwrap();
        assert_eq!(payload["files"], serde_json::json!(["README.md"]));
        assert_eq!(payload["auto_retry"], serde_json::json!(true));
    }

    /// 3 个冲突文件仍在上界之内（阈值为 `<= 3`），照常自动重排。
    #[test]
    fn requeues_rebase_conflict_at_three_files() {
        let (ctx, task_id) = setup_with("true", |repo| {
            Arc::new(ConflictingAdapter {
                repo: repo.to_path_buf(),
                files: vec!["a.txt", "b.txt", "c.txt"],
            })
        });

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::Queued);
        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        assert!(detail.decisions.is_empty());
        let event = detail
            .events
            .iter()
            .find(|row| row.kind == "rebase_retry")
            .expect("rebase_retry event");
        let payload: serde_json::Value = serde_json::from_str(&event.payload_json).unwrap();
        assert_eq!(
            payload["files"].as_array().map(|files| files.len()),
            Some(3)
        );
    }

    /// 冲突文件超过 3 个：转人工，决策里保留完整文件清单。
    #[test]
    fn blocks_rebase_conflict_with_four_files() {
        let files = ["a.txt", "b.txt", "c.txt", "d.txt"];
        let (ctx, task_id) = setup_with("true", |repo| {
            Arc::new(ConflictingAdapter {
                repo: repo.to_path_buf(),
                files: files.to_vec(),
            })
        });

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::NeedsHuman);
        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        let decision = detail
            .decisions
            .iter()
            .find(|row| row.kind == "rebase_failed" && row.status == "open")
            .expect("rebase_failed decision");
        for path in files {
            assert!(
                decision.prompt.contains(path),
                "决策缺少冲突文件 {path}：{}",
                decision.prompt
            );
        }
    }

    /// 依赖清单属于高风险路径：即使只有 1 个冲突文件也必须转人工。
    #[test]
    fn blocks_high_risk_rebase_conflict() {
        let (ctx, task_id) = setup_with("true", |repo| {
            Arc::new(ConflictingAdapter {
                repo: repo.to_path_buf(),
                files: vec!["Cargo.toml"],
            })
        });

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::NeedsHuman);
        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        let decision = detail
            .decisions
            .iter()
            .find(|row| row.kind == "rebase_failed" && row.status == "open")
            .expect("rebase_failed decision");
        assert!(
            decision.prompt.contains("Cargo.toml"),
            "prompt: {}",
            decision.prompt
        );
    }

    /// 重试次数已用尽：即使冲突很小也转人工并保留文件清单。
    #[test]
    fn blocks_rebase_conflict_when_attempts_exhausted() {
        let (ctx, task_id) = setup_with("true", |repo| {
            Arc::new(ConflictingAdapter {
                repo: repo.to_path_buf(),
                files: vec!["README.md"],
            })
        });
        {
            let guard = ctx.db.lock().unwrap();
            for _ in 0..(store::MAX_ATTEMPTS - 1) {
                store::start_run(
                    guard.conn(),
                    NewRun {
                        id: None,
                        task_id: task_id.clone(),
                        adapter_id: "seed".into(),
                        adapter_version: None,
                        model: None,
                        branch: None,
                        worktree_path: None,
                        base_sha: None,
                    },
                )
                .unwrap();
            }
        }

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::NeedsHuman);
        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        assert_eq!(detail.task.attempts, store::MAX_ATTEMPTS);
        let decision = detail
            .decisions
            .iter()
            .find(|row| row.kind == "rebase_failed" && row.status == "open")
            .expect("rebase_failed decision");
        assert!(
            decision.prompt.contains("README.md"),
            "prompt: {}",
            decision.prompt
        );
        let event = detail
            .events
            .iter()
            .find(|row| row.kind == "rebase_failed")
            .expect("rebase_failed event");
        let payload: serde_json::Value = serde_json::from_str(&event.payload_json).unwrap();
        assert_eq!(payload["attempts"], serde_json::json!(store::MAX_ATTEMPTS));
        assert_eq!(payload["auto_retry"], serde_json::json!(false));
    }

    /// rebase 因非冲突原因失败（pre-rebase hook 拒绝）时没有任何冲突文件：不能
    /// 换基线自动重排，必须立即打开 `rebase_failed` 决策转人工且不消耗重试。
    #[test]
    fn blocks_rebase_failure_without_conflict_files() {
        let (ctx, task_id) = setup_with("test -f done.txt", |repo| {
            let hook = repo.join(".git/hooks/pre-rebase");
            fs::create_dir_all(hook.parent().unwrap()).unwrap();
            fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&hook).unwrap().permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&hook, perms).unwrap();
            }
            Arc::new(MainAdvancingAdapter {
                repo: repo.to_path_buf(),
            })
        });
        let repo = repo_of(&ctx, &task_id);
        let before = gitops::rev_parse(&repo, "main").unwrap();

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::NeedsHuman);
        assert_ne!(
            gitops::rev_parse(&repo, "main").unwrap(),
            before,
            "并发任务的提交必须保留在默认分支上"
        );

        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        assert_eq!(detail.task.attempts, 1, "非冲突失败不能消耗自动重试");
        assert!(
            !detail.events.iter().any(|row| row.kind == "rebase_retry"),
            "非冲突失败不得自动重排"
        );
        let event = detail
            .events
            .iter()
            .find(|row| row.kind == "rebase_failed")
            .expect("rebase_failed event");
        let payload: serde_json::Value = serde_json::from_str(&event.payload_json).unwrap();
        assert_eq!(payload["files"], serde_json::json!([]));
        assert_eq!(payload["auto_retry"], serde_json::json!(false));
        let decision = detail
            .decisions
            .iter()
            .find(|row| row.kind == "rebase_failed" && row.status == "open")
            .expect("rebase_failed decision");
        assert!(
            decision.prompt.contains("（无）"),
            "prompt: {}",
            decision.prompt
        );
    }

    #[test]
    fn times_out_and_opens_timeout_decision() {
        let (ctx, task_id) = setup_with("true", |_| Arc::new(SilentAdapter::default()));
        {
            let guard = ctx.db.lock().unwrap();
            let changed = guard
                .conn()
                .execute(
                    "UPDATE tasks SET max_minutes = 0 WHERE id = ?1",
                    rusqlite::params![task_id.as_str()],
                )
                .unwrap();
            assert_eq!(changed, 1);
        }

        let started = Instant::now();
        run_task_blocking(&ctx, &task_id).unwrap();
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(5), "时限路径耗时 {elapsed:?}");

        assert_eq!(status(&ctx, &task_id), TaskStatus::NeedsHuman);

        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        assert!(
            detail
                .decisions
                .iter()
                .any(|row| row.kind == "timeout" && row.status == "open"),
            "decisions: {:?}",
            detail
                .decisions
                .iter()
                .map(|row| row.kind.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn skips_rollback_when_target_tip_moved_after_merge() {
        let (ctx, task_id) = setup(
            "if [ -d .temli ]; then git -c user.name=t -c user.email=t@t commit -q --allow-empty -m external; false; else true; fi",
        );
        let repo = repo_of(&ctx, &task_id);
        // `.temli/` 只在目标仓库存在：验收先在 worktree 通过，合并后重跑时
        // 制造一个外部提交再失败。
        fs::create_dir_all(repo.join(".temli")).unwrap();
        let pre_merge = gitops::rev_parse(&repo, "main").unwrap();

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::NeedsHuman);
        // tip 已越过本次合并：外部提交必须保留，不能被硬回滚丢弃。
        let head = gitops::head_sha(&repo).unwrap();
        assert_ne!(head, pre_merge, "tip 已前移时不应回滚");
        let subject = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["log", "-1", "--format=%s"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&subject.stdout).trim(),
            "external",
            "外部提交必须保留"
        );

        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        let decision = detail
            .decisions
            .iter()
            .find(|row| row.kind == "post_merge_failed")
            .expect("post_merge_failed decision");
        assert!(
            decision.prompt.contains("目标分支已被外部修改，跳过回滚"),
            "prompt: {}",
            decision.prompt
        );
    }

    #[test]
    fn cancel_during_handshake_interrupts_without_evidence() {
        let (ctx, task_id) = setup("true");
        // 模拟取消请求落在开发者适配器 start 的握手窗口内：会话尚未登记。
        ctx.cancelled.lock().unwrap().insert(task_id.clone());

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::Interrupted);
        assert!(
            !ctx.cancelled.lock().unwrap().contains(&task_id),
            "退出路径必须清除取消意图"
        );
        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        assert!(detail.evidence.is_empty(), "握手取消不应收集证据");
        assert!(detail.review.is_none(), "握手取消不应进入审查");
        assert_eq!(detail.runs.len(), 1);
        assert_eq!(detail.runs[0].status, "cancelled");
    }

    fn green_check(name: &str, bucket: &str) -> CheckRun {
        CheckRun {
            name: name.into(),
            state: "completed".into(),
            bucket: bucket.into(),
        }
    }

    fn run_pr_fields(
        ctx: &RunContext,
        task_id: &str,
    ) -> (Option<i64>, Option<String>, Option<String>) {
        let guard = ctx.db.lock().unwrap();
        guard
            .conn()
            .query_row(
                "SELECT pr_number, pr_url, pr_state FROM runs
                 WHERE task_id = ?1 ORDER BY attempt DESC LIMIT 1",
                rusqlite::params![task_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
    }

    #[test]
    fn checks_passed_respects_require_checks() {
        assert!(
            checks_passed(&[], false),
            "未要求检查时空检查列表应视为通过"
        );
        assert!(
            !checks_passed(&[], true),
            "要求检查时空检查列表不能视为通过"
        );
        for require_checks in [false, true] {
            assert!(checks_passed(
                &[
                    green_check("ci", "success"),
                    green_check("lint", "skipped"),
                    green_check("optional", "neutral"),
                ],
                require_checks
            ));
            assert!(!checks_passed(
                &[
                    green_check("ci", "success"),
                    green_check("deploy", "pending"),
                ],
                require_checks
            ));
            assert!(!checks_passed(
                &[green_check("ci", "success"), green_check("test", "failure"),],
                require_checks
            ));
        }
        assert!(checks_failed_terminally(&[green_check("test", "failure")]));
        assert!(!checks_failed_terminally(&[green_check(
            "deploy", "pending"
        )]));
    }

    #[test]
    fn auto_merge_pushes_pr_and_merges_when_checks_pass() {
        let ops = Arc::new(FakeGitHubOps::new(vec![
            green_check("ci", "success"),
            green_check("lint", "success"),
        ]));
        let (ctx, task_id) = setup_full(
            "test -f done.txt",
            |_| Arc::new(ScriptedAdapter),
            Some(ops.clone()),
            true,
        );
        let repo = repo_of(&ctx, &task_id);
        ops.simulate_merge_in(repo.clone());
        let before = gitops::head_sha(&repo).unwrap();

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::Merged);
        let merged = ops.merged.lock().unwrap().clone();
        assert_eq!(merged.len(), 1, "PR 必须且只能合并一次");
        assert_eq!(merged[0].0, 42);
        let branch = {
            let guard = ctx.db.lock().unwrap();
            let detail = store::get_task(guard.conn(), &task_id).unwrap();
            detail.runs[0].branch.clone().expect("run branch")
        };
        assert_eq!(
            merged[0].1,
            gitops::rev_parse(&repo, &branch).unwrap(),
            "合并 PR 必须钉在被检查的 head 上"
        );
        {
            let created = ops.created.lock().unwrap();
            assert_eq!(created.len(), 1);
            assert_eq!(created[0].1, "main");
            assert!(
                created[0].0.starts_with("temli/"),
                "PR head: {}",
                created[0].0
            );
        }
        let (number, url, state) = run_pr_fields(&ctx, &task_id);
        assert_eq!(number, Some(42));
        assert_eq!(url.as_deref(), Some("https://github.com/acme/demo/pull/42"));
        assert_eq!(state.as_deref(), Some("merged"));
        assert_ne!(
            gitops::head_sha(&repo).unwrap(),
            before,
            "拉取后默认分支必须前进"
        );

        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        assert!(detail.decisions.is_empty());
        assert_eq!(detail.runs[0].status, "completed");
    }

    /// 项目未要求检查时空检查列表直接通过：不轮询、不等超时，正常走合并。
    #[test]
    fn auto_merge_without_checks_merges_when_checks_not_required() {
        let ops = Arc::new(FakeGitHubOps::new(Vec::new()));
        let (ctx, task_id) = setup_full(
            "test -f done.txt",
            |_| Arc::new(ScriptedAdapter),
            Some(ops.clone()),
            true,
        );
        let repo = repo_of(&ctx, &task_id);
        ops.simulate_merge_in(repo.clone());
        let before = gitops::head_sha(&repo).unwrap();

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::Merged);
        assert_eq!(
            ops.calls()
                .iter()
                .filter(|call| **call == "pull_request_checks")
                .count(),
            1,
            "空检查列表必须在首次轮询就通过，不能等待：{:?}",
            ops.calls()
        );
        assert_eq!(ops.merged.lock().unwrap().len(), 1, "PR 必须合并一次");
        assert_ne!(gitops::head_sha(&repo).unwrap(), before);
        let (_, url, state) = run_pr_fields(&ctx, &task_id);
        assert_eq!(url.as_deref(), Some("https://github.com/acme/demo/pull/42"));
        assert_eq!(state.as_deref(), Some("merged"));

        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        assert!(detail.decisions.is_empty());
        assert_eq!(detail.runs[0].status, "completed");
    }

    /// 项目要求检查时空检查列表立即进入 checks_failed 人工决策，绝不能合并。
    #[test]
    fn auto_merge_blocks_empty_checks_when_checks_required() {
        let ops = Arc::new(FakeGitHubOps::new(Vec::new()));
        let (ctx, task_id) = setup_full(
            "test -f done.txt",
            |_| Arc::new(ScriptedAdapter),
            Some(ops.clone()),
            true,
        );
        set_require_checks(&ctx, &task_id, true);
        let repo = repo_of(&ctx, &task_id);
        ops.simulate_merge_in(repo.clone());
        let before = gitops::head_sha(&repo).unwrap();

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::NeedsHuman);
        assert!(
            ops.merged.lock().unwrap().is_empty(),
            "要求检查时无检查结果的 PR 绝不能合并"
        );
        assert_eq!(
            ops.calls()
                .iter()
                .filter(|call| **call == "pull_request_checks")
                .count(),
            1,
            "空检查列表应立即失败，不能轮询到超时：{:?}",
            ops.calls()
        );
        assert_eq!(gitops::head_sha(&repo).unwrap(), before);
        let (number, url, state) = run_pr_fields(&ctx, &task_id);
        assert_eq!(number, Some(42));
        assert_eq!(url.as_deref(), Some("https://github.com/acme/demo/pull/42"));
        assert_eq!(state.as_deref(), Some("open"));

        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        let decision = detail
            .decisions
            .iter()
            .find(|row| row.kind == "checks_failed")
            .expect("checks_failed decision");
        assert!(
            decision.prompt.contains("CI 检查"),
            "prompt 必须说明要求 CI 检查: {}",
            decision.prompt
        );
        assert!(
            decision
                .prompt
                .contains("https://github.com/acme/demo/pull/42"),
            "prompt 必须带 PR 链接: {}",
            decision.prompt
        );
        assert_eq!(detail.runs[0].status, "failed");
    }

    /// 取消发生在检查轮询期间：PR 已创建但绝不能合并，运行按取消收尾。
    #[test]
    fn auto_merge_cancel_during_checks_never_merges() {
        let ops = Arc::new(FakeGitHubOps::new(vec![green_check("ci", "pending")]));
        let (ctx, task_id) = setup_full(
            "test -f done.txt",
            |_| Arc::new(ScriptedAdapter),
            Some(ops.clone()),
            true,
        );
        let repo = repo_of(&ctx, &task_id);
        ops.simulate_merge_in(repo.clone());
        ops.cancel_on_checks(ctx.cancelled.clone(), task_id.clone());
        let before = gitops::head_sha(&repo).unwrap();

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::Interrupted);
        assert!(ops.merged.lock().unwrap().is_empty(), "取消后绝不能合并 PR");
        assert!(
            !ops.calls().contains(&"merge_pull_request"),
            "取消后不得调用合并接口: {:?}",
            ops.calls()
        );
        assert_eq!(
            gitops::head_sha(&repo).unwrap(),
            before,
            "取消后本地默认分支不得变化"
        );
        let (number, url, state) = run_pr_fields(&ctx, &task_id);
        assert_eq!(number, Some(42));
        assert_eq!(url.as_deref(), Some("https://github.com/acme/demo/pull/42"));
        assert_eq!(state.as_deref(), Some("cancelled"));

        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        assert_eq!(detail.runs[0].status, "cancelled");
        assert!(
            detail.decisions.is_empty(),
            "取消不应打开合并失败决策: {:?}",
            detail
                .decisions
                .iter()
                .map(|row| row.kind.as_str())
                .collect::<Vec<_>>()
        );
        drop(guard);
        assert!(
            !ctx.cancelled.lock().unwrap().contains(&task_id),
            "退出路径必须清除取消意图"
        );
    }

    /// 远端已合并但本地 `pull --ff-only` 失败：`pr_state` 必须保持 merged，
    /// 只记录 `local_sync_failed` 决策，且绝不重试合并。
    #[test]
    fn auto_merge_keeps_merged_state_when_local_sync_fails() {
        let ops = Arc::new(FakeGitHubOps::new(vec![green_check("ci", "success")]));
        let (ctx, task_id) = setup_full(
            "test -f done.txt",
            |_| Arc::new(ScriptedAdapter),
            Some(ops.clone()),
            true,
        );
        let repo = repo_of(&ctx, &task_id);
        ops.simulate_merge_in(repo.clone());
        ops.diverge_local_after_merge();

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::NeedsHuman);
        let merged = ops.merged.lock().unwrap().clone();
        assert_eq!(merged.len(), 1, "本地同步失败不能重试 PR 合并");
        assert_eq!(merged[0].0, 42);
        let branch = {
            let guard = ctx.db.lock().unwrap();
            let detail = store::get_task(guard.conn(), &task_id).unwrap();
            detail.runs[0].branch.clone().expect("run branch")
        };
        assert_eq!(
            merged[0].1,
            gitops::rev_parse(&repo, &branch).unwrap(),
            "合并 PR 必须钉在被检查的 head 上"
        );
        assert_eq!(
            ops.calls()
                .iter()
                .filter(|call| **call == "merge_pull_request")
                .count(),
            1,
            "本地同步失败后不得再次合并 PR"
        );

        let (number, url, state) = run_pr_fields(&ctx, &task_id);
        assert_eq!(number, Some(42));
        assert_eq!(url.as_deref(), Some("https://github.com/acme/demo/pull/42"));
        assert_eq!(
            state.as_deref(),
            Some("merged"),
            "本地同步失败不能覆盖已合并状态"
        );

        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        let decision = detail
            .decisions
            .iter()
            .find(|row| row.kind == "local_sync_failed")
            .expect("local_sync_failed decision");
        assert_eq!(decision.status, "open");
        assert!(
            decision
                .prompt
                .contains("https://github.com/acme/demo/pull/42"),
            "prompt 必须带 PR 链接: {}",
            decision.prompt
        );
        assert_eq!(detail.runs[0].status, "failed");
    }

    #[test]
    fn auto_merge_blocks_on_failed_checks_without_merging() {
        let ops = Arc::new(FakeGitHubOps::new(vec![green_check("ci", "failure")]));
        let (ctx, task_id) = setup_full(
            "test -f done.txt",
            |_| Arc::new(ScriptedAdapter),
            Some(ops.clone()),
            true,
        );
        let repo = repo_of(&ctx, &task_id);
        ops.simulate_merge_in(repo.clone());
        let before = gitops::head_sha(&repo).unwrap();

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::NeedsHuman);
        assert!(
            ops.merged.lock().unwrap().is_empty(),
            "检查失败时绝不能合并 PR"
        );
        assert_eq!(gitops::head_sha(&repo).unwrap(), before);
        let (number, url, state) = run_pr_fields(&ctx, &task_id);
        assert_eq!(number, Some(42));
        assert_eq!(url.as_deref(), Some("https://github.com/acme/demo/pull/42"));
        assert_eq!(state.as_deref(), Some("open"));

        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        let decision = detail
            .decisions
            .iter()
            .find(|row| row.kind == "checks_failed")
            .expect("checks_failed decision");
        assert!(
            decision
                .prompt
                .contains("https://github.com/acme/demo/pull/42"),
            "prompt 必须带 PR 链接: {}",
            decision.prompt
        );
        assert_eq!(detail.runs[0].status, "failed");
    }

    #[test]
    fn auto_merge_off_keeps_local_merge_and_skips_github() {
        let ops = Arc::new(FakeGitHubOps::new(vec![green_check("ci", "success")]));
        let (ctx, task_id) = setup_full(
            "test -f done.txt",
            |_| Arc::new(ScriptedAdapter),
            Some(ops.clone()),
            false,
        );
        let repo = repo_of(&ctx, &task_id);
        let before = gitops::head_sha(&repo).unwrap();

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::Merged);
        assert!(
            ops.calls().is_empty(),
            "auto_merge 关闭时不得调用 GitHub: {:?}",
            ops.calls()
        );
        assert!(ops.merged.lock().unwrap().is_empty());
        assert_ne!(gitops::head_sha(&repo).unwrap(), before);
        assert_eq!(run_pr_fields(&ctx, &task_id), (None, None, None));
    }

    fn context_item(title: &str, body: &str) -> store::ContextItem {
        store::ContextItem {
            id: title.to_string(),
            kind: "note".into(),
            title: title.to_string(),
            body: body.to_string(),
        }
    }

    #[test]
    fn render_context_is_empty_without_items() {
        assert_eq!(render_context(&[]), "");
    }

    #[test]
    fn render_context_truncates_item_bodies_and_section() {
        let long_body = "长".repeat(CONTEXT_ITEM_CHARS + 1_000);
        let rendered = render_context(&[context_item("长笔记", &long_body)]);
        let (_, body) = rendered.split_once("### 长笔记\n").expect("标题与正文");
        assert_eq!(body.chars().count(), CONTEXT_ITEM_CHARS);
        assert!(rendered.chars().count() <= CONTEXT_SECTION_CHARS);

        // 多个超长条目也不能让整段超过 12k 字符。
        let items: Vec<_> = (0..5)
            .map(|index| context_item(&format!("笔记{index}"), &"x".repeat(CONTEXT_ITEM_CHARS + 1)))
            .collect();
        let rendered = render_context(&items);
        assert!(rendered.chars().count() <= CONTEXT_SECTION_CHARS);
        assert!(rendered.chars().count() > CONTEXT_ITEM_CHARS);
    }

    #[test]
    fn render_context_preserves_item_order() {
        let rendered = render_context(&[
            context_item("第一条", "正文一"),
            context_item("第二条", "正文二"),
        ]);
        let first = rendered.find("第一条").expect("第一条");
        let second = rendered.find("第二条").expect("第二条");
        assert!(first < second);
        assert!(rendered.starts_with("## 项目上下文\n\n### 第一条\n正文一"));
    }

    #[test]
    fn assigned_developer_and_reviewer_run_their_own_sessions() {
        let developer = Arc::new(DeveloperFake::new());
        let reviewer = Arc::new(ReviewerFake::new());
        let dev_agent: Arc<dyn AgentAdapter> = developer.clone();
        let rev_agent: Arc<dyn AgentAdapter> = reviewer.clone();
        let default_agent: Arc<dyn AgentAdapter> = Arc::new(ScriptedAdapter);
        let (ctx, task_id) = setup_agents(
            "test -f done.txt",
            move |_| vec![default_agent, dev_agent, rev_agent],
            None,
            false,
        );
        {
            let guard = ctx.db.lock().unwrap();
            store::set_task_agents(
                guard.conn(),
                &task_id,
                Some("dev-fake"),
                Some("review-fake"),
            )
            .unwrap();
        }

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(developer.starts(), 1, "开发者适配器执行实现");
        assert_eq!(reviewer.starts(), 1, "审查必须交给任务指定的审查者");
        let detail = {
            let guard = ctx.db.lock().unwrap();
            store::get_task(guard.conn(), &task_id).unwrap()
        };
        assert_eq!(detail.task.status, TaskStatus::Merged);
        let review = detail.review.expect("review");
        assert_eq!(review.verdict, "pass");
        assert_eq!(
            review.summary.as_deref(),
            Some("reviewed by review-fake"),
            "裁决必须来自指定的审查适配器"
        );
        let run = detail.runs.last().expect("run");
        assert_eq!(run.adapter_id, "dev-fake", "运行记录归属开发者适配器");
    }

    #[test]
    fn reviewer_falls_back_to_the_assigned_developer() {
        let developer = Arc::new(DeveloperFake::new());
        let dev_agent: Arc<dyn AgentAdapter> = developer.clone();
        let default_agent: Arc<dyn AgentAdapter> = Arc::new(ScriptedAdapter);
        let (ctx, task_id) = setup_agents(
            "test -f done.txt",
            move |_| vec![default_agent, dev_agent],
            None,
            false,
        );
        {
            let guard = ctx.db.lock().unwrap();
            store::set_task_agents(guard.conn(), &task_id, Some("dev-fake"), None).unwrap();
        }

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(developer.starts(), 2, "实现与审查都使用该开发者适配器");
        assert_eq!(developer.reviewer_starts(), 1);
        let detail = {
            let guard = ctx.db.lock().unwrap();
            store::get_task(guard.conn(), &task_id).unwrap()
        };
        assert_eq!(detail.task.status, TaskStatus::Merged);
        let review = detail.review.expect("review");
        assert_eq!(review.summary.as_deref(), Some("reviewed by dev-fake"));
    }

    #[test]
    fn task_model_override_reaches_worker_and_reviewer_specs() {
        let recorder = Arc::new(ModelRecordingAdapter::new());
        let agent: Arc<dyn AgentAdapter> = recorder.clone();
        let (ctx, task_id) = setup_agents("test -f done.txt", move |_| vec![agent], None, false);
        {
            let guard = ctx.db.lock().unwrap();
            store::set_task_model(guard.conn(), &task_id, Some("gpt-5.2-codex")).unwrap();
        }

        run_task_blocking(&ctx, &task_id).unwrap();

        assert_eq!(status(&ctx, &task_id), TaskStatus::Merged);
        let models = recorder.models();
        assert_eq!(models.len(), 2, "worker 与 reviewer 各启动一次");
        assert_eq!(
            models,
            vec![
                Some("gpt-5.2-codex".to_string()),
                Some("gpt-5.2-codex".to_string())
            ],
            "worker 与 reviewer 的 RunSpec 都应带上任务模型"
        );
    }

    #[test]
    fn unregistered_assigned_adapter_is_rejected_without_side_effects() {
        let developer = Arc::new(DeveloperFake::new());
        let dev_agent: Arc<dyn AgentAdapter> = developer.clone();
        let (ctx, task_id) =
            setup_agents("test -f done.txt", move |_| vec![dev_agent], None, false);

        {
            let guard = ctx.db.lock().unwrap();
            store::set_task_agents(guard.conn(), &task_id, Some("ghost"), None).unwrap();
        }
        let err = run_task_blocking(&ctx, &task_id).unwrap_err();
        assert!(err.contains("未注册") && err.contains("ghost"), "{err}");

        {
            let guard = ctx.db.lock().unwrap();
            store::set_task_agents(guard.conn(), &task_id, None, Some("ghost-review")).unwrap();
        }
        let err = run_task_blocking(&ctx, &task_id).unwrap_err();
        assert!(
            err.contains("审查者") && err.contains("ghost-review"),
            "{err}"
        );

        assert_eq!(status(&ctx, &task_id), TaskStatus::Draft);
        let run_count = {
            let guard = ctx.db.lock().unwrap();
            store::get_task(guard.conn(), &task_id).unwrap().runs.len()
        };
        assert_eq!(run_count, 0, "配置错误不能留下运行记录");
        assert_eq!(developer.starts(), 0, "未注册的指派不能落到默认适配器");
    }

    #[test]
    fn worker_prompt_injects_linked_doc_and_skips_notes() {
        let (ctx, task_id) = setup("test -f done.txt");
        let guard = ctx.db.lock().unwrap();
        let conn = guard.conn();
        let task = store::get_task(conn, &task_id).unwrap().task;
        let prompt_without_context = worker_prompt(conn, &task);
        assert!(!prompt_without_context.contains("## 项目上下文"));

        let doc_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO items (id, project_id, kind, title, body)
             VALUES (?1, ?2, 'doc', ?3, ?4)",
            rusqlite::params![doc_id, task.project_id, "设计文档", "关联文档的正文"],
        )
        .unwrap();
        let note_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO items (id, project_id, kind, title, body)
             VALUES (?1, ?2, 'note', ?3, ?4)",
            rusqlite::params![note_id, task.project_id, "架构笔记", "笔记不应注入"],
        )
        .unwrap();
        let task_item = store::task_item_id(conn, &task.id).unwrap();
        store::link_item(conn, &task_item, &doc_id, "relates").unwrap();
        store::link_item(conn, &task_item, &note_id, "relates").unwrap();

        let prompt = worker_prompt(conn, &task);
        assert_eq!(
            prompt,
            format!("{prompt_without_context}\n\n## 项目上下文\n\n### 设计文档\n关联文档的正文")
        );
        assert!(!prompt.contains("笔记不应注入"));
    }
}
