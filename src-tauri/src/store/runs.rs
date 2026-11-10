use super::*;
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

/// 项目下最近的审查列表，按审查时间倒序，最多 100 条。
///
/// 证据统计只统计该次 run 的验收证据（`criterion_id IS NOT NULL`）：artifact /
/// review 这类辅助证据不挂在验收标准上，不计入分母。通过数在同一集合上按
/// `exit_code = 0` 计，未取到 exit code 的证据不算通过。
pub fn list_reviews(conn: &Connection, project_id: &str) -> Result<Vec<ReviewListItem>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT rev.run_id, t.id, i.title, rev.verdict, rev.summary,
                    rev.risk_flags_json, rev.test_diff_json, rev.created_at,
                    (SELECT COUNT(*) FROM evidence e
                     WHERE e.run_id = rev.run_id AND e.criterion_id IS NOT NULL),
                    (SELECT COALESCE(SUM(e.exit_code = 0), 0)
                     FROM evidence e
                     WHERE e.run_id = rev.run_id AND e.criterion_id IS NOT NULL),
                    r.pr_url
             FROM reviews rev
             JOIN runs r ON r.id = rev.run_id
             JOIN tasks t ON t.id = r.task_id
             JOIN items i ON i.id = t.item_id
             WHERE i.project_id = ?1
             ORDER BY rev.created_at DESC, rev.rowid DESC
             LIMIT 100",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![project_id], read_review_list_item)
        .map_err(db_err)?;

    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

// ---------------------------------------------------------------------------
// 运行
// ---------------------------------------------------------------------------

/// 建立一次运行：attempt 自增，`tasks.attempts` 同步为本次 attempt。
///
/// `NewRun.id` 可传入调用方预先生成的 id（worktree、分支名都以它为准）；
/// 未提供时由存储层生成。
pub fn start_run(conn: &Connection, input: NewRun) -> Result<RunRow, String> {
    let NewRun {
        id,
        task_id,
        adapter_id,
        adapter_version,
        model,
        branch,
        worktree_path,
        base_sha,
    } = input;
    ensure_task(conn, &task_id)?;

    let run_id = match id {
        Some(id) => {
            if id.trim().is_empty() {
                return Err("运行 id 不能为空".to_string());
            }
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM runs WHERE id = ?1)",
                    params![id],
                    |row| row.get(0),
                )
                .map_err(db_err)?;
            if exists {
                return Err(format!("运行 id 已存在：{id}"));
            }
            id
        }
        None => Uuid::new_v4().to_string(),
    };

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let attempt: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(attempt), 0) + 1 FROM runs WHERE task_id = ?1",
            params![task_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    tx.execute(
        "INSERT INTO runs (id, task_id, attempt, adapter_id, adapter_version, model,
                           worktree_path, branch, base_sha, status, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'running',
                 strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
        params![
            run_id,
            task_id,
            attempt,
            adapter_id,
            adapter_version,
            model,
            worktree_path,
            branch,
            base_sha
        ],
    )
    .map_err(db_err)?;
    tx.execute(
        "UPDATE tasks SET attempts = ?2 WHERE id = ?1",
        params![task_id, attempt],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;

    read_run(conn, &run_id)
}

/// 结束一次运行；`status` 仅接受 `completed` / `failed` / `cancelled`。
pub fn finish_run(
    conn: &Connection,
    run_id: &str,
    status: &str,
    error: Option<&str>,
) -> Result<RunRow, String> {
    if !matches!(status, "completed" | "failed" | "cancelled") {
        return Err(format!(
            "未知的运行状态：{status}（可选 completed / failed / cancelled）"
        ));
    }
    let changed = conn
        .execute(
            "UPDATE runs
             SET status = ?2,
                 ended_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                 error = ?3
             WHERE id = ?1",
            params![run_id, status, error],
        )
        .map_err(db_err)?;
    if changed == 0 {
        return Err(format!("运行不存在：{run_id}"));
    }
    read_run(conn, run_id)
}

/// 记录适配器会话 id（`runs.session_id`）。
pub fn set_run_session(conn: &Connection, run_id: &str, session_id: &str) -> Result<(), String> {
    let changed = conn
        .execute(
            "UPDATE runs SET session_id = ?2 WHERE id = ?1",
            params![run_id, session_id],
        )
        .map_err(db_err)?;
    if changed == 0 {
        return Err(format!("运行不存在：{run_id}"));
    }
    Ok(())
}

/// 记录运行关联的 GitHub PR（自动合并路径）。
pub fn set_run_pr(
    conn: &Connection,
    run_id: &str,
    number: u64,
    url: &str,
    state: &str,
) -> Result<(), String> {
    let changed = conn
        .execute(
            "UPDATE runs SET pr_number = ?2, pr_url = ?3, pr_state = ?4 WHERE id = ?1",
            params![run_id, number as i64, url, state],
        )
        .map_err(db_err)?;
    if changed == 0 {
        return Err(format!("运行不存在：{run_id}"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 证据 / 审查
// ---------------------------------------------------------------------------

pub fn add_evidence(conn: &Connection, input: NewEvidence) -> Result<EvidenceRow, String> {
    if !matches!(
        input.kind.as_str(),
        "test" | "build" | "lint" | "review" | "artifact"
    ) {
        return Err(format!("未知的证据类型：{}", input.kind));
    }
    ensure_run(conn, &input.run_id)?;

    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO evidence (id, run_id, criterion_id, kind, cmd, exit_code, duration_ms,
                               output_path)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id,
            input.run_id,
            input.criterion_id,
            input.kind,
            input.cmd,
            input.exit_code,
            input.duration_ms,
            input.output_path
        ],
    )
    .map_err(db_err)?;

    conn.query_row(
        "SELECT id, criterion_id, kind, cmd, exit_code, duration_ms, output_path, collected_at
         FROM evidence WHERE id = ?1",
        params![id],
        read_evidence_row,
    )
    .map_err(db_err)
}

/// 写入或覆盖 run 的审查结论（`reviews.run_id` 唯一）。
pub fn set_review(conn: &Connection, input: NewReview) -> Result<ReviewRow, String> {
    if !matches!(input.verdict.as_str(), "pass" | "fail" | "needs_human") {
        return Err(format!("未知的审查结论：{}", input.verdict));
    }
    ensure_run(conn, &input.run_id)?;

    let risk_flags_json =
        serde_json::to_string(&input.risk_flags).map_err(|e| format!("风险标记序列化失败：{e}"))?;
    let test_diff_json =
        serde_json::to_string(&input.test_diff).map_err(|e| format!("测试改动序列化失败：{e}"))?;

    conn.execute(
        "INSERT INTO reviews (id, run_id, verdict, summary, risk_flags_json, test_diff_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(run_id) DO UPDATE SET
             verdict = excluded.verdict,
             summary = excluded.summary,
             risk_flags_json = excluded.risk_flags_json,
             test_diff_json = excluded.test_diff_json,
             created_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        params![
            Uuid::new_v4().to_string(),
            input.run_id,
            input.verdict,
            input.summary,
            risk_flags_json,
            test_diff_json
        ],
    )
    .map_err(db_err)?;

    conn.query_row(
        "SELECT verdict, summary, risk_flags_json, test_diff_json
         FROM reviews WHERE run_id = ?1",
        params![input.run_id],
        read_review_row,
    )
    .map_err(db_err)
}

// ---------------------------------------------------------------------------
// 决策 / 事件
// ---------------------------------------------------------------------------

pub fn open_decision(conn: &Connection, input: NewDecision) -> Result<DecisionRow, String> {
    ensure_project(conn, &input.project_id)?;
    if let Some(task_id) = input.task_id.as_deref() {
        ensure_task(conn, task_id)?;
    }
    if let Some(run_id) = input.run_id.as_deref() {
        ensure_run(conn, run_id)?;
    }

    let id = Uuid::new_v4().to_string();
    let options_json =
        serde_json::to_string(&input.options).map_err(|e| format!("选项序列化失败：{e}"))?;

    conn.execute(
        "INSERT INTO decisions (id, project_id, task_id, run_id, kind, prompt, options_json,
                                status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'open')",
        params![
            id,
            input.project_id,
            input.task_id,
            input.run_id,
            input.kind,
            input.prompt,
            options_json
        ],
    )
    .map_err(db_err)?;

    read_decision(conn, &id)
}

/// 解决一条未决决策；已解决/已过期再调用返回 `Err`。
pub fn resolve_decision(
    conn: &Connection,
    decision_id: &str,
    resolution: &str,
) -> Result<DecisionRow, String> {
    let status: Option<String> = conn
        .query_row(
            "SELECT status FROM decisions WHERE id = ?1",
            params![decision_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    match status.as_deref() {
        None => return Err(format!("决策不存在：{decision_id}")),
        Some("open") => {}
        Some(other) => {
            return Err(format!("决策已处理（{other}），不能再解决：{decision_id}"));
        }
    }

    conn.execute(
        "UPDATE decisions
         SET status = 'resolved',
             resolution = ?2,
             resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        params![decision_id, resolution],
    )
    .map_err(db_err)?;

    read_decision(conn, decision_id)
}

/// 追加事件（只追加，不改写）；id 自动生成，payload 存为 JSON 文本。
pub fn append_event(conn: &Connection, input: NewEvent) -> Result<EventRow, String> {
    ensure_project(conn, &input.project_id)?;

    let id = Uuid::new_v4().to_string();
    let payload_json = input.payload.to_string();
    conn.execute(
        "INSERT INTO events (id, project_id, entity, entity_id, type, payload_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            id,
            input.project_id,
            input.entity,
            input.entity_id,
            input.kind,
            payload_json
        ],
    )
    .map_err(db_err)?;

    conn.query_row(
        "SELECT id, type, payload_json, at FROM events WHERE id = ?1",
        params![id],
        read_event_row,
    )
    .map_err(db_err)
}

// ---------------------------------------------------------------------------
// 证据门 / 崩溃恢复
// ---------------------------------------------------------------------------

/// 证据门（§4.2）：最新 run 的每条验收标准都要有最新 evidence 满足
/// `exit_code == criterion.expect_exit`，且（当 `expect_match` 存在时）其
/// `output_path` 文件内容包含该片段；文件缺失或不可读视为失败。
/// 此外该 run 的审查必须为 `pass`，且没有未决决策时，才允许
/// `verifying -> merged`。
pub fn merge_ready(conn: &Connection, task_id: &str) -> Result<bool, String> {
    let acceptance_json: String = conn
        .query_row(
            "SELECT acceptance_json FROM tasks WHERE id = ?1",
            params![task_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("任务不存在：{task_id}"))?;
    let criteria = parse_criteria(&acceptance_json)?;
    if criteria.is_empty() {
        return Ok(false);
    }

    let run_id: Option<String> = conn
        .query_row(
            "SELECT id FROM runs WHERE task_id = ?1 ORDER BY attempt DESC LIMIT 1",
            params![task_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    let Some(run_id) = run_id else {
        return Ok(false);
    };

    for criterion in &criteria {
        if !criterion_satisfied_by_latest_evidence(conn, &run_id, criterion)? {
            return Ok(false);
        }
    }

    let review_passed: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM reviews WHERE run_id = ?1 AND verdict = 'pass')",
            params![run_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if !review_passed {
        return Ok(false);
    }

    let open_decisions: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM decisions
             WHERE status = 'open'
               AND (task_id = ?1 OR run_id IN (SELECT id FROM runs WHERE task_id = ?1))",
            params![task_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;

    Ok(open_decisions == 0)
}

/// 判定某条验收标准是否被 run 内最新一条证据满足：退出码必须等于
/// `expect_exit`；存在 `expect_match` 时，证据输出文件必须可读且包含该片段。
fn criterion_satisfied_by_latest_evidence(
    conn: &Connection,
    run_id: &str,
    criterion: &Criterion,
) -> Result<bool, String> {
    let latest: Option<(Option<i32>, Option<String>)> = conn
        .query_row(
            "SELECT exit_code, output_path FROM evidence
             WHERE run_id = ?1 AND criterion_id = ?2
             ORDER BY collected_at DESC, rowid DESC
             LIMIT 1",
            params![run_id, criterion.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(db_err)?;

    let Some((exit_code, output_path)) = latest else {
        return Ok(false);
    };
    if exit_code != Some(criterion.expect_exit) {
        return Ok(false);
    }

    if let Some(fragment) = criterion.expect_match.as_deref() {
        let Some(path) = output_path else {
            return Ok(false);
        };
        let Ok(output) = std::fs::read_to_string(&path) else {
            return Ok(false);
        };
        if !output.contains(fragment) {
            return Ok(false);
        }
    }

    Ok(true)
}

/// 启动恢复（§4.8）：把残留的 `running` / `verifying` 任务标记为 `interrupted`，
/// 同时把其未结束的 run 收尾为 `cancelled`；返回被中断的任务数。
pub fn interrupt_running(conn: &Connection) -> Result<usize, String> {
    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let interrupted = tx
        .execute(
            "UPDATE items
             SET status = 'interrupted',
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE status IN ('running', 'verifying')
               AND id IN (SELECT item_id FROM tasks)",
            [],
        )
        .map_err(db_err)?;
    tx.execute(
        "UPDATE runs
         SET status = 'cancelled',
             ended_at = COALESCE(ended_at, strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             error = COALESCE(error, '应用重启，运行中断')
         WHERE status = 'running'
           AND task_id IN (
               SELECT t.id FROM tasks t JOIN items i ON i.id = t.item_id
               WHERE i.status = 'interrupted'
           )",
        [],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;
    Ok(interrupted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::*;

    #[test]
    fn merge_ready_requires_green_evidence_pass_review_and_no_open_decision() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let id = task.id.as_str();
        ready_to_running(conn, id);
        transition(conn, id, TaskStatus::Verifying).unwrap();
        let run = start_task_run(&db, id);
        assert_eq!(run.status, "running");

        assert!(!merge_ready(conn, id).unwrap());

        let first = add_evidence(conn, evidence(&run.id, "c1", 0)).unwrap();
        assert_eq!(first.criterion_id.as_deref(), Some("c1"));
        assert_eq!(first.exit_code, Some(0));
        assert_eq!(first.duration_ms, Some(42));
        assert!(!merge_ready(conn, id).unwrap());

        add_evidence(conn, evidence(&run.id, "c2", 1)).unwrap();
        assert!(!merge_ready(conn, id).unwrap());

        add_evidence(conn, evidence(&run.id, "c2", 0)).unwrap();
        // 证据全绿，但缺审查
        assert!(!merge_ready(conn, id).unwrap());

        let review = set_review(
            conn,
            NewReview {
                run_id: run.id.clone(),
                verdict: "pass".into(),
                summary: Some("证据齐全".into()),
                risk_flags: vec!["依赖清单".into()],
                test_diff: vec!["tests/gate.rs +12/-3".into()],
            },
        )
        .unwrap();
        assert_eq!(review.verdict, "pass");
        assert_eq!(review.risk_flags, vec!["依赖清单".to_string()]);
        assert_eq!(review.test_diff, vec!["tests/gate.rs +12/-3".to_string()]);
        assert!(merge_ready(conn, id).unwrap());

        // 审查结论可覆盖
        let failed_review = set_review(
            conn,
            NewReview {
                run_id: run.id.clone(),
                verdict: "fail".into(),
                summary: Some("存在回归".into()),
                risk_flags: vec![],
                test_diff: vec![],
            },
        )
        .unwrap();
        assert_eq!(failed_review.verdict, "fail");
        assert!(!merge_ready(conn, id).unwrap());
        assert!(
            set_review(
                conn,
                NewReview {
                    run_id: run.id.clone(),
                    verdict: "maybe".into(),
                    summary: None,
                    risk_flags: vec![],
                    test_diff: vec![],
                },
            )
            .is_err()
        );

        set_review(
            conn,
            NewReview {
                run_id: run.id.clone(),
                verdict: "pass".into(),
                summary: Some("证据齐全".into()),
                risk_flags: vec![],
                test_diff: vec!["tests/gate.rs +12/-3".into()],
            },
        )
        .unwrap();
        assert!(merge_ready(conn, id).unwrap());

        // 未决决策阻塞合并
        let decision = open_decision(
            conn,
            NewDecision {
                project_id: project_id.clone(),
                task_id: Some(task.id.clone()),
                run_id: Some(run.id.clone()),
                kind: "approval".into(),
                prompt: "是否允许删除文件？".into(),
                options: vec!["allow".into(), "deny".into()],
            },
        )
        .unwrap();
        assert_eq!(decision.status, "open");
        assert!(decision.resolution.is_none());
        assert!(!merge_ready(conn, id).unwrap());

        let resolved = resolve_decision(conn, &decision.id, "allow").unwrap();
        assert_eq!(resolved.status, "resolved");
        assert_eq!(resolved.resolution.as_deref(), Some("allow"));
        assert!(merge_ready(conn, id).unwrap());
        assert!(resolve_decision(conn, &decision.id, "deny").is_err());

        let detail = get_task(conn, id).unwrap();
        assert_eq!(detail.runs.len(), 1);
        assert_eq!(detail.evidence.len(), 3);
        assert_eq!(detail.decisions.len(), 1);
        assert_eq!(detail.decisions[0].status, "resolved");
        let stored_review = detail.review.expect("review");
        assert_eq!(stored_review.verdict, "pass");
        assert_eq!(
            stored_review.test_diff,
            vec!["tests/gate.rs +12/-3".to_string()]
        );
    }

    #[test]
    fn merge_ready_honors_expected_exit_code() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(
            conn,
            new_task(
                &project_id,
                vec![Criterion {
                    id: "c-1".into(),
                    cmd: "sh -c 'exit 1'".into(),
                    expect_exit: 1,
                    expect_match: None,
                }],
            ),
        )
        .unwrap();
        let id = task.id.as_str();
        ready_to_running(conn, id);
        transition(conn, id, TaskStatus::Verifying).unwrap();
        let run = start_task_run(&db, id);
        set_review(
            conn,
            NewReview {
                run_id: run.id.clone(),
                verdict: "pass".into(),
                summary: None,
                risk_flags: vec![],
                test_diff: vec![],
            },
        )
        .unwrap();

        add_evidence(conn, evidence(&run.id, "c-1", 0)).unwrap();
        assert!(
            !merge_ready(conn, id).unwrap(),
            "exit 0 不满足 expect_exit 1"
        );

        add_evidence(conn, evidence(&run.id, "c-1", 1)).unwrap();
        assert!(
            merge_ready(conn, id).unwrap(),
            "exit 1 应满足 expect_exit 1"
        );
    }

    #[test]
    fn merge_ready_honors_expect_match_against_output_file() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(
            conn,
            new_task(
                &project_id,
                vec![Criterion {
                    id: "c-1".into(),
                    cmd: "echo needle".into(),
                    expect_exit: 0,
                    expect_match: Some("needle".into()),
                }],
            ),
        )
        .unwrap();
        let id = task.id.as_str();
        ready_to_running(conn, id);
        transition(conn, id, TaskStatus::Verifying).unwrap();
        let run = start_task_run(&db, id);
        set_review(
            conn,
            NewReview {
                run_id: run.id.clone(),
                verdict: "pass".into(),
                summary: None,
                risk_flags: vec![],
                test_diff: vec![],
            },
        )
        .unwrap();

        let hit = write_evidence_output("--- stdout ---\nneedle\n");
        add_evidence(conn, evidence_with_output(&run.id, "c-1", 0, Some(hit))).unwrap();
        assert!(merge_ready(conn, id).unwrap(), "输出包含片段应通过");

        let miss = write_evidence_output("--- stdout ---\nhaystack\n");
        add_evidence(conn, evidence_with_output(&run.id, "c-1", 0, Some(miss))).unwrap();
        assert!(!merge_ready(conn, id).unwrap(), "最新证据不含片段应失败");

        let missing = temp_dir().join("missing.log");
        add_evidence(
            conn,
            evidence_with_output(
                &run.id,
                "c-1",
                0,
                Some(missing.to_string_lossy().into_owned()),
            ),
        )
        .unwrap();
        assert!(!merge_ready(conn, id).unwrap(), "输出文件缺失应失败");

        add_evidence(conn, evidence_with_output(&run.id, "c-1", 0, None)).unwrap();
        assert!(!merge_ready(conn, id).unwrap(), "缺少 output_path 应失败");

        let hit_again = write_evidence_output("stdout: needle\n");
        add_evidence(
            conn,
            evidence_with_output(&run.id, "c-1", 0, Some(hit_again)),
        )
        .unwrap();
        assert!(merge_ready(conn, id).unwrap(), "最新证据重新命中应通过");
    }

    #[test]
    fn merge_ready_is_false_for_unknown_task() {
        let db = open_db();
        let conn = db.conn();
        let err = merge_ready(conn, "missing-task").unwrap_err();
        assert!(err.contains("任务不存在"), "{err}");
    }

    #[test]
    fn interrupt_running_marks_tasks_and_closes_runs() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        ready_to_running(conn, &task.id);
        let run = start_task_run(&db, &task.id);
        assert_eq!(run.status, "running");

        assert_eq!(interrupt_running(conn).unwrap(), 1);

        let detail = get_task(conn, &task.id).unwrap();
        assert_eq!(detail.task.status, TaskStatus::Interrupted);
        assert_eq!(detail.runs.len(), 1);
        assert_eq!(detail.runs[0].status, "cancelled");
        assert!(detail.runs[0].ended_at.is_some());
        assert_eq!(detail.runs[0].error.as_deref(), Some("应用重启，运行中断"));

        // 已无运行中的任务
        assert_eq!(interrupt_running(conn).unwrap(), 0);
    }

    #[test]
    fn start_run_uses_caller_supplied_id() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();
        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();

        let mut spec = new_run(&task.id);
        spec.id = Some("run-fixed".into());
        let run = start_run(conn, spec).unwrap();
        assert_eq!(run.id, "run-fixed");

        let mut duplicate = new_run(&task.id);
        duplicate.id = Some("run-fixed".into());
        let err = start_run(conn, duplicate).unwrap_err();
        assert!(err.contains("已存在"), "{err}");
    }

    #[test]
    fn finish_run_and_append_event() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let run = start_task_run(&db, &task.id);
        assert_eq!(run.status, "running");
        assert_eq!(get_task(conn, &task.id).unwrap().task.attempts, 1);

        set_run_session(conn, &run.id, "session-1").unwrap();
        let finished = finish_run(conn, &run.id, "completed", None).unwrap();
        assert_eq!(finished.status, "completed");
        assert!(finished.ended_at.is_some());
        assert!(finished.error.is_none());
        assert!(finish_run(conn, &run.id, "bogus", None).is_err());
        assert!(finish_run(conn, "missing-run", "failed", Some("x")).is_err());

        let event = append_event(
            conn,
            NewEvent {
                project_id: project_id.clone(),
                entity: "task".into(),
                entity_id: task.id.clone(),
                kind: "task.created".into(),
                payload: serde_json::json!({ "attempt": 1 }),
            },
        )
        .unwrap();
        assert!(!event.id.is_empty());
        assert_eq!(event.kind, "task.created");
        assert_eq!(event.payload_json, "{\"attempt\":1}");

        let session_id: Option<String> = conn
            .query_row(
                "SELECT session_id FROM runs WHERE id = ?1",
                [run.id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(session_id.as_deref(), Some("session-1"));

        let detail = get_task(conn, &task.id).unwrap();
        assert_eq!(detail.events.len(), 1);
        assert_eq!(detail.events[0].payload_json, "{\"attempt\":1}");
    }

    #[test]
    fn list_reviews_orders_by_created_at_and_counts_evidence() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();

        // 第一次运行：审查通过、无风险，1 条证据通过。
        let pass_run = start_task_run(&db, &task.id);
        set_review(
            conn,
            NewReview {
                run_id: pass_run.id.clone(),
                verdict: "pass".into(),
                summary: Some("证据齐全".into()),
                risk_flags: vec![],
                test_diff: vec![],
            },
        )
        .unwrap();
        add_evidence(conn, evidence(&pass_run.id, "c1", 0)).unwrap();

        // 第二次运行：待人工、带风险标记，2 条证据其中 1 条失败。
        let human_run = start_task_run(&db, &task.id);
        set_review(
            conn,
            NewReview {
                run_id: human_run.id.clone(),
                verdict: "needs_human".into(),
                summary: Some("测试断言被放宽".into()),
                risk_flags: vec!["test_weakened".into()],
                test_diff: vec!["tests/gate.rs +1/-1".into()],
            },
        )
        .unwrap();
        add_evidence(conn, evidence(&human_run.id, "c1", 0)).unwrap();
        add_evidence(conn, evidence(&human_run.id, "c2", 1)).unwrap();

        // artifact / review 辅助证据的 criterion_id 为 NULL：退出码为 0 也不计入
        // 验收证据的分母或通过数。
        for (run_id, kind) in [
            (pass_run.id.as_str(), "artifact"),
            (pass_run.id.as_str(), "review"),
            (human_run.id.as_str(), "artifact"),
            (human_run.id.as_str(), "review"),
        ] {
            add_evidence(conn, auxiliary_evidence(run_id, kind)).unwrap();
        }

        // 固定时间戳，避免两条 review 落在同一毫秒导致顺序不稳定。
        for (run_id, created_at) in [
            (pass_run.id.as_str(), "2025-01-01T00:00:00.000Z"),
            (human_run.id.as_str(), "2025-01-02T00:00:00.000Z"),
        ] {
            conn.execute(
                "UPDATE reviews SET created_at = ?2 WHERE run_id = ?1",
                params![run_id, created_at],
            )
            .unwrap();
        }

        let rows = list_reviews(conn, &project_id).unwrap();
        assert_eq!(rows.len(), 2);

        let latest = &rows[0];
        assert_eq!(latest.run_id, human_run.id);
        assert_eq!(latest.task_id, task.id);
        assert_eq!(latest.task_title, "实现证据门");
        assert_eq!(latest.verdict, "needs_human");
        assert_eq!(latest.summary.as_deref(), Some("测试断言被放宽"));
        assert_eq!(latest.risk_flags, vec!["test_weakened".to_string()]);
        assert_eq!(latest.test_diff, vec!["tests/gate.rs +1/-1".to_string()]);
        assert_eq!(latest.created_at, "2025-01-02T00:00:00.000Z");
        // 只统计验收证据：2 条挂验收标准的证据，额外 2 条辅助证据被排除。
        assert_eq!(latest.evidence_total, 2);
        assert_eq!(latest.evidence_passed, 1);
        assert_eq!(latest.pr_url, None);

        let previous = &rows[1];
        assert_eq!(previous.run_id, pass_run.id);
        assert_eq!(previous.verdict, "pass");
        assert!(previous.risk_flags.is_empty());
        assert!(previous.test_diff.is_empty());
        // 全绿 run：辅助证据退出码为 0 也不进入统计，因此 total == passed。
        assert_eq!(previous.evidence_total, 1);
        assert_eq!(previous.evidence_passed, 1);

        assert!(list_reviews(conn, "missing-project").unwrap().is_empty());
    }
}
