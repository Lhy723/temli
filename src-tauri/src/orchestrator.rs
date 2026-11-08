use crate::agent::{
    AgentAdapter, AgentEvent, Decision, EventSink, RunSpec, RunStatus, SessionHandle,
};
use crate::db::Db;
use crate::evidence::{self, RiskKind};
use crate::gitops;
use crate::prompts;
use crate::store::{
    self, NewDecision, NewEvent, NewEvidence, NewReview, NewRun, TaskRow, TaskStatus,
};
use rusqlite::params;
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
    pub adapter: Arc<dyn AgentAdapter>,
    pub data_dir: PathBuf,
    /// 已启动但尚未结束的任务 id，防止同一任务被并发启动两次。
    pub running: Arc<Mutex<HashSet<String>>>,
    /// 运行中的适配器会话，供取消操作使用。
    pub cancels: CancelRegistry,
    /// 目标分支合并写锁（M2 合并队列）。
    pub merge_lock: Arc<Mutex<()>>,
    /// 已请求取消但尚未拿到会话的任务（取消意图）。
    pub cancelled: Arc<Mutex<HashSet<String>>>,
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

    if task.status == TaskStatus::Draft {
        transition(&ctx.db, task_id, TaskStatus::Ready)?;
    }
    if !matches!(
        task.status,
        TaskStatus::Draft | TaskStatus::Ready | TaskStatus::Queued
    ) {
        return Err(format!(
            "任务当前状态不可执行：{:?}（需要 draft / ready / queued）",
            task.status
        ));
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
                adapter_id: ctx.adapter.manifest().id,
                adapter_version: Some(ctx.adapter.manifest().version),
                model: None,
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
    let spec = RunSpec {
        run_id: run_id.clone(),
        cwd: worktree.clone(),
        prompt: worker_prompt(&task),
        model: None,
        max_minutes: u64::from(max_minutes),
        max_cost_usd: None,
        env_allowlist: Vec::new(),
    };

    let handle = ctx.adapter.start(spec, sink)?;

    // 取消请求可能落在 start 的握手窗口内：此时会话尚未登记，事件流不会收到取消，
    // 必须在驱动前消费取消意图并主动终止刚启动的会话。
    if cancel_requested(ctx, task_id) {
        let _ = ctx.adapter.cancel(&handle);
        let _ = ctx.adapter.close(&handle);
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
        .register(task_id, ctx.adapter.clone(), handle.clone());
    let session_saved = lock_db(&ctx.db).and_then(|guard| match ctx.adapter.session_id(&handle) {
        Some(sid) => store::set_run_session(guard.conn(), &run_id, &sid),
        None => Ok(()),
    });
    if let Err(err) = session_saved {
        let _ = ctx.adapter.close(&handle);
        return Err(err);
    }

    let session = drive_session(
        ctx,
        &handle,
        &rx,
        &decisions,
        u64::from(max_minutes).saturating_mul(60),
    );
    let _ = ctx.adapter.close(&handle);
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
        ctx,
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
    let approvals_supported = ctx.adapter.manifest().capabilities.approvals;

    if verdict == "pass" && ready && !has_high_risk && on_default_branch && approvals_supported {
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

fn worker_prompt(task: &TaskRow) -> String {
    format!(
        "你在隔离的 git worktree 中开发。\n\n任务：{}\n\n{}\n\n要求：\n- 只修改完成任务所需的文件，不要触碰 .temli/。\n- 完成后运行仓库既有测试。\n- 不要执行 force push、删除仓库或修改 CI 密钥等破坏性操作。\n- 保持改动可审查、可回滚。",
        task.title, task.prompt
    )
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
                if !wait_for_approval(ctx, handle, &request_id, decisions, deadline)? {
                    let _ = ctx.adapter.cancel(handle);
                    return Ok(SessionEnd::TimedOut);
                }
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err("Agent 事件流中断".to_string()),
        }
        if Instant::now() >= deadline {
            let _ = ctx.adapter.cancel(handle);
            return Ok(SessionEnd::TimedOut);
        }
    }
}

/// 等待审批决策；返回 `Ok(true)` 表示已答复，`Ok(false)` 表示到达执行时限。
fn wait_for_approval(
    ctx: &RunContext,
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
        ctx.adapter.respond(handle, request_id, Decision::Deny)?;
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
                ctx.adapter.respond(handle, request_id, decision)?;
                return Ok(true);
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    let _ = ctx.adapter.respond(handle, request_id, Decision::Deny);
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
    ctx: &RunContext,
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
        model: None,
        max_minutes: 10,
        max_cost_usd: None,
        env_allowlist: Vec::new(),
    };
    let handle = ctx.adapter.start(spec, sink)?;
    let collected = drive_reviewer(ctx, &handle, &rx);
    let _ = ctx.adapter.close(&handle);
    let text = collected?;
    prompts::parse_review(&text)
}

/// 只接受 `Ended { status: Completed }` 作为成功终态：其余终态、断流、超时都是失败，
/// 由调用方转入 needs_human，绝不让未完成的审查被当成通过。
fn drive_reviewer(
    ctx: &RunContext,
    handle: &SessionHandle,
    rx: &Receiver<AgentEvent>,
) -> Result<String, String> {
    let mut text = String::new();
    let deadline = Instant::now() + Duration::from_secs(10 * 60);
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(AgentEvent::TextDelta { text: delta }) => text.push_str(&delta),
            Ok(AgentEvent::ApprovalRequest { request_id, .. }) => {
                let _ = ctx.adapter.respond(handle, &request_id, Decision::Deny);
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
            let _ = ctx.adapter.cancel(handle);
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
    use crate::agent::{AgentManifest, AuthState, Capabilities, DetectResult, EventSink};
    use crate::store::{Criterion, NewRun, NewTask};
    use std::fs;

    fn test_manifest(id: &str) -> AgentManifest {
        AgentManifest {
            id: id.into(),
            name: "Scripted".into(),
            version: "0.0.1".into(),
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

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "temli-orch-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
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
        let repo = init_repo();
        let db_dir = temp_dir();
        let db = Db::open(&db_dir.join("temli.db")).unwrap();
        let project = db
            .insert_project(
                crate::db::RegisterProjectInput {
                    repo_path: repo.to_string_lossy().into_owned(),
                    name: Some("demo".into()),
                    github_owner: None,
                    github_repo: None,
                    default_branch: Some("main".into()),
                },
                repo.canonicalize().unwrap(),
            )
            .unwrap();
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
        let ctx = RunContext {
            db: Arc::new(Mutex::new(db)),
            adapter: adapter(&repo),
            data_dir: db_dir.join("data"),
            running: Arc::new(Mutex::new(std::collections::HashSet::new())),
            cancels: CancelRegistry::new(),
            merge_lock: Arc::new(Mutex::new(())),
            cancelled: Arc::new(Mutex::new(HashSet::new())),
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
        // 模拟取消请求落在 adapter.start 握手窗口内：会话尚未登记。
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
}
