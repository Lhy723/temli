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
}

/// 运行线程的作用域守卫：无论正常结束、提前返回还是 panic 展开，都会释放任务占位。
struct RunGuard {
    running: Arc<Mutex<HashSet<String>>>,
    task_id: String,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        if let Ok(mut running) = self.running.lock() {
            running.remove(&self.task_id);
        }
    }
}

pub fn spawn_run(ctx: RunContext, task_id: String) -> Result<(), String> {
    let thread_ctx = ctx.clone();
    let thread_task = task_id.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("temli-run-{}", short_id(&task_id)))
        .spawn(move || {
            let _guard = RunGuard {
                running: thread_ctx.running.clone(),
                task_id: thread_task.clone(),
            };
            if let Err(err) = run_blocking(&thread_ctx, &thread_task) {
                let _ = record_failure(&thread_ctx, &thread_task, &err);
            }
        });
    match spawned {
        Ok(handle) => drop(handle),
        Err(err) => {
            if let Ok(mut running) = ctx.running.lock() {
                running.remove(&task_id);
            }
            return Err(format!("无法启动运行线程：{err}"));
        }
    }
    Ok(())
}

fn lock_db(db: &Arc<Mutex<Db>>) -> Result<std::sync::MutexGuard<'_, Db>, String> {
    db.lock().map_err(|_| "数据库锁被占用".to_string())
}

fn transition(db: &Arc<Mutex<Db>>, task_id: &str, to: TaskStatus) -> Result<(), String> {
    let guard = lock_db(db)?;
    store::transition(guard.conn(), task_id, to).map(|_| ())
}

fn run_blocking(ctx: &RunContext, task_id: &str) -> Result<(), String> {
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
    }
    transition(&ctx.db, task_id, TaskStatus::Running)?;

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

    let spec = RunSpec {
        run_id: run_id.clone(),
        cwd: worktree.clone(),
        prompt: worker_prompt(&task),
        model: None,
        max_minutes: 30,
        max_cost_usd: None,
        env_allowlist: Vec::new(),
    };

    let handle = ctx.adapter.start(spec, sink)?;
    let session_saved = lock_db(&ctx.db).and_then(|guard| match ctx.adapter.session_id(&handle) {
        Some(sid) => store::set_run_session(guard.conn(), &run_id, &sid),
        None => Ok(()),
    });
    if let Err(err) = session_saved {
        let _ = ctx.adapter.close(&handle);
        return Err(err);
    }

    let status = drive_session(ctx, &handle, &rx, &decisions, 30 * 60);
    let _ = ctx.adapter.close(&handle);
    let status = status?;

    if status != RunStatus::Completed {
        close_run(ctx, &run_id, "failed", Some("agent did not complete"));
        transition(&ctx.db, task_id, TaskStatus::Failed)?;
        return Ok(());
    }

    gitops::commit_all(&worktree, &format!("temli: {} ({short})", task.title))?;
    let head = gitops::head_sha(&worktree)?;
    if head == base_sha {
        close_run(ctx, &run_id, "failed", Some("agent produced no changes"));
        transition(&ctx.db, task_id, TaskStatus::Failed)?;
        return Ok(());
    }

    transition(&ctx.db, task_id, TaskStatus::Verifying)?;

    let diff = evidence::collect_diff(&worktree, &base_sha)?;
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

    let evidence_rows = run_acceptance(ctx, &detail, &worktree, &output_dir, &run_id)?;

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
        match merge_reviewed_head(&repo, &branch, &head) {
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
            Err(err) => {
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

fn drive_session(
    ctx: &RunContext,
    handle: &SessionHandle,
    rx: &Receiver<AgentEvent>,
    decisions: &Arc<Mutex<HashMap<String, String>>>,
    timeout_secs: u64,
) -> Result<RunStatus, String> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(AgentEvent::Ended { status }) => return Ok(status),
            Ok(AgentEvent::ApprovalRequest { request_id, .. }) => {
                wait_for_approval(ctx, handle, &request_id, decisions, deadline)?;
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err("Agent 事件流中断".to_string()),
        }
        if Instant::now() >= deadline {
            let _ = ctx.adapter.cancel(handle);
            return Ok(RunStatus::Cancelled);
        }
    }
}

fn wait_for_approval(
    ctx: &RunContext,
    handle: &SessionHandle,
    request_id: &str,
    decisions: &Arc<Mutex<HashMap<String, String>>>,
    deadline: Instant,
) -> Result<(), String> {
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
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    let _ = ctx.adapter.respond(handle, request_id, Decision::Deny);
    Err("审批超时".to_string())
}

fn run_acceptance(
    ctx: &RunContext,
    detail: &store::TaskDetail,
    worktree: &Path,
    output_dir: &Path,
    run_id: &str,
) -> Result<Vec<store::EvidenceRow>, String> {
    let mut rows = Vec::new();
    for criterion in &detail.criteria {
        let outcome = evidence::run_command(worktree, &criterion.cmd, 600);
        let output_path = output_dir.join(format!("{}.log", output_stem(&criterion.id)));
        let body = format!(
            "$ {}\nexit={:?}\ntimed_out={}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            criterion.cmd, outcome.exit_code, outcome.timed_out, outcome.stdout, outcome.stderr
        );
        std::fs::write(&output_path, body).map_err(|e| e.to_string())?;
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
        rows.push(row);
    }
    Ok(rows)
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

fn record_failure(ctx: &RunContext, task_id: &str, err: &str) -> Result<(), String> {
    let (project_id, status) = {
        let guard = lock_db(&ctx.db)?;
        let detail = store::get_task(guard.conn(), task_id)?;
        (detail.task.project_id, detail.task.status)
    };
    {
        let guard = lock_db(&ctx.db)?;
        store::append_event(
            guard.conn(),
            NewEvent {
                project_id,
                entity: "task".into(),
                entity_id: task_id.to_string(),
                kind: "orchestrator_error".into(),
                payload: serde_json::json!({ "message": err }),
            },
        )?;
    }
    if matches!(status, TaskStatus::Running | TaskStatus::Verifying) {
        let _ = transition(&ctx.db, task_id, TaskStatus::Failed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentManifest, AuthState, Capabilities, DetectResult, EventSink};
    use crate::store::{Criterion, NewTask};
    use std::fs;

    struct ScriptedAdapter;

    impl AgentAdapter for ScriptedAdapter {
        fn manifest(&self) -> AgentManifest {
            AgentManifest {
                id: "scripted".into(),
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

        fn detect(&self) -> DetectResult {
            DetectResult {
                available: true,
                version: Some("0.0.1".into()),
                auth: AuthState::Ok,
                detail: None,
            }
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
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {:?}", args);
        };
        git(&["init", "-q", "-b", "main"]);
        fs::write(repo.join(".gitignore"), ".temli/\n").unwrap();
        fs::write(repo.join("README.md"), "demo\n").unwrap();
        git(&["add", "-A"]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "init",
        ]);
        repo
    }

    fn setup(cmd: &str) -> (RunContext, String) {
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
            adapter: Arc::new(ScriptedAdapter),
            data_dir: db_dir.join("data"),
            running: Arc::new(Mutex::new(std::collections::HashSet::new())),
        };
        (ctx, task.id)
    }

    fn status(ctx: &RunContext, task_id: &str) -> TaskStatus {
        let guard = ctx.db.lock().unwrap();
        store::get_task(guard.conn(), task_id).unwrap().task.status
    }

    #[test]
    fn merges_when_evidence_and_review_pass() {
        let (ctx, task_id) = setup("test -f done.txt");
        run_blocking(&ctx, &task_id).unwrap();
        assert_eq!(status(&ctx, &task_id), TaskStatus::Merged);

        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        assert_eq!(detail.evidence.len(), 1);
        assert_eq!(detail.evidence[0].exit_code, Some(0));
        assert_eq!(detail.review.unwrap().verdict, "pass");
        assert!(detail.decisions.is_empty());
    }

    #[test]
    fn blocks_on_failed_evidence_and_opens_decision() {
        let (ctx, task_id) = setup("false");
        run_blocking(&ctx, &task_id).unwrap();
        assert_eq!(status(&ctx, &task_id), TaskStatus::NeedsHuman);

        let guard = ctx.db.lock().unwrap();
        let detail = store::get_task(guard.conn(), &task_id).unwrap();
        assert_eq!(detail.evidence[0].exit_code, Some(1));
        assert_eq!(detail.decisions.len(), 1);
        assert_eq!(detail.decisions[0].status, "open");
    }
}
