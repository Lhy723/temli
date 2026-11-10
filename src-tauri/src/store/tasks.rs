use super::*;
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// 任务
// ---------------------------------------------------------------------------

/// 新建任务：一个事务内写入 `items(kind='issue', status='draft')` 与 `tasks` 行。
pub fn create_task(conn: &Connection, input: NewTask) -> Result<TaskRow, String> {
    let NewTask {
        project_id,
        title,
        prompt,
        criteria,
    } = input;
    validate_criteria(&criteria)?;
    ensure_project(conn, &project_id)?;

    let item_id = Uuid::new_v4().to_string();
    let task_id = Uuid::new_v4().to_string();
    let acceptance_json =
        serde_json::to_string(&criteria).map_err(|e| format!("验收标准序列化失败：{e}"))?;

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO items (id, project_id, kind, title, status)
         VALUES (?1, ?2, 'issue', ?3, 'draft')",
        params![item_id, project_id, title],
    )
    .map_err(db_err)?;
    tx.execute(
        "INSERT INTO tasks (id, item_id, acceptance_json, prompt)
         VALUES (?1, ?2, ?3, ?4)",
        params![task_id, item_id, acceptance_json, prompt],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;

    conn.query_row(
        "SELECT t.id, i.project_id, i.title, i.status, t.prompt, t.attempts, t.max_minutes,
                t.created_at, t.developer_adapter, t.reviewer_adapter, t.model
         FROM tasks t JOIN items i ON i.id = t.item_id
         WHERE t.id = ?1",
        params![task_id],
        read_task_row,
    )
    .map_err(db_err)
}

/// 把已有的 issue item 转换为任务：item 行保持原样，只把 `status` 置为
/// `draft`，并在同一事务内写入 `tasks` 行。
///
/// 与 [`create_task`] 不同，这里复用同步进来的 item（`github_number` / `body`
/// 等字段不受影响），因此要求 item 存在、`kind='issue'` 且尚未关联任务。
pub fn create_task_for_item(
    conn: &Connection,
    item_id: &str,
    prompt: &str,
    criteria: Vec<Criterion>,
) -> Result<TaskRow, String> {
    validate_criteria(&criteria)?;

    let (project_id, kind) = conn
        .query_row(
            "SELECT project_id, kind FROM items WHERE id = ?1",
            params![item_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("条目不存在：{item_id}"))?;
    if kind != "issue" {
        return Err(format!("只能将 issue 条目转换为任务：{item_id}"));
    }
    ensure_project(conn, &project_id)?;

    let existing: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE item_id = ?1)",
            params![item_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if existing {
        return Err(format!("条目已有任务：{item_id}"));
    }

    let task_id = Uuid::new_v4().to_string();
    let acceptance_json =
        serde_json::to_string(&criteria).map_err(|e| format!("验收标准序列化失败：{e}"))?;

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO tasks (id, item_id, acceptance_json, prompt)
         VALUES (?1, ?2, ?3, ?4)",
        params![task_id, item_id, acceptance_json, prompt],
    )
    .map_err(db_err)?;
    tx.execute(
        "UPDATE items
         SET status = 'draft', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        params![item_id],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;

    conn.query_row(
        "SELECT t.id, i.project_id, i.title, i.status, t.prompt, t.attempts, t.max_minutes,
                t.created_at, t.developer_adapter, t.reviewer_adapter, t.model
         FROM tasks t JOIN items i ON i.id = t.item_id
         WHERE t.id = ?1",
        params![task_id],
        read_task_row,
    )
    .map_err(db_err)
}

/// 为已合并任务排队一个自动文档任务。
///
/// 与普通任务不同：新 item 直接以 `queued` 落库（无需经过 draft/ready），任务行
/// 标记 `auto_docs=1`（合并后不再递归调度），并通过 `derived_from`
/// （父任务 item → 新 item）记录来源。父任务、项目或非空验收标准任一缺失都拒绝。
pub fn create_docs_task(
    conn: &Connection,
    project_id: &str,
    parent_task_id: &str,
    title: &str,
    prompt: &str,
    criteria: Vec<Criterion>,
) -> Result<TaskDetail, String> {
    if criteria.is_empty() {
        return Err("自动文档任务必须包含验收标准".to_string());
    }
    validate_criteria(&criteria)?;
    ensure_project(conn, project_id)?;

    let (parent_item, parent_project) = conn
        .query_row(
            "SELECT t.item_id, i.project_id
             FROM tasks t JOIN items i ON i.id = t.item_id
             WHERE t.id = ?1",
            params![parent_task_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("任务不存在：{parent_task_id}"))?;
    if parent_project != project_id {
        return Err(format!("父任务不属于该项目：{parent_task_id}"));
    }

    let item_id = Uuid::new_v4().to_string();
    let task_id = Uuid::new_v4().to_string();
    let acceptance_json =
        serde_json::to_string(&criteria).map_err(|e| format!("验收标准序列化失败：{e}"))?;

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    tx.execute(
        "INSERT INTO items (id, project_id, kind, title, status)
         VALUES (?1, ?2, 'issue', ?3, 'queued')",
        params![item_id, project_id, title],
    )
    .map_err(db_err)?;
    tx.execute(
        "INSERT INTO tasks (id, item_id, acceptance_json, prompt, auto_docs)
         VALUES (?1, ?2, ?3, ?4, 1)",
        params![task_id, item_id, acceptance_json, prompt],
    )
    .map_err(db_err)?;
    tx.execute(
        "INSERT INTO links (id, from_item, to_item, rel)
         VALUES (?1, ?2, ?3, 'derived_from')",
        params![Uuid::new_v4().to_string(), parent_item, item_id],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;

    get_task(conn, &task_id)
}

/// 全项目任务汇总；仅测试使用（生产走 [`list_tasks_for_project`]）。
#[cfg(test)]
pub fn list_tasks(conn: &Connection) -> Result<Vec<TaskSummary>, String> {
    query_task_summaries(conn, None)
}

pub fn list_tasks_for_project(
    conn: &Connection,
    project_id: &str,
) -> Result<Vec<TaskSummary>, String> {
    query_task_summaries(conn, Some(project_id))
}

/// 读取任务当前状态，供调度器做轻量轮询（避免拉取完整 [`TaskDetail`]）。
pub fn task_status(conn: &Connection, task_id: &str) -> Result<TaskStatus, String> {
    let raw = conn
        .query_row(
            "SELECT i.status FROM tasks t JOIN items i ON i.id = t.item_id WHERE t.id = ?1",
            params![task_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("任务不存在：{task_id}"))?;
    TaskStatus::parse(&raw)
}

/// 任务对应的 issue item id；依赖边存放在 `items` 之间，写入前需先做映射。
pub fn task_item_id(conn: &Connection, task_id: &str) -> Result<String, String> {
    conn.query_row(
        "SELECT item_id FROM tasks WHERE id = ?1",
        params![task_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("任务不存在：{task_id}"))
}

/// 任务是否为自动文档任务；默认 `false`。
///
/// 自动文档任务合并后绝不能再调度新的文档任务，否则会形成无限递归。
pub fn task_auto_docs(conn: &Connection, task_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT COALESCE(auto_docs, 0) FROM tasks WHERE id = ?1",
        params![task_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("任务不存在：{task_id}"))
}

/// 项目下的 issue item 列表（含本地尚未发布的 issue）。
pub fn list_project_issues(
    conn: &Connection,
    project_id: &str,
) -> Result<Vec<ProjectIssue>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT i.id, i.title, i.status, i.github_state, i.github_number, i.ghost, t.id
             FROM items i LEFT JOIN tasks t ON t.item_id = i.id
             WHERE i.project_id = ?1 AND i.kind = 'issue'
             ORDER BY CASE WHEN i.github_number IS NULL THEN 1 ELSE 0 END,
                      i.github_number ASC, i.created_at ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![project_id], |row| {
            Ok(ProjectIssue {
                id: row.get(0)?,
                title: row.get(1)?,
                status: row.get(2)?,
                github_state: row.get(3)?,
                github_number: row.get(4)?,
                ghost: row.get(5)?,
                task_id: row.get(6)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

pub fn get_task(conn: &Connection, task_id: &str) -> Result<TaskDetail, String> {
    let (task, acceptance_json) = conn
        .query_row(
            "SELECT t.id, i.project_id, i.title, i.status, t.prompt, t.attempts, t.max_minutes,
                    t.created_at, t.developer_adapter, t.reviewer_adapter, t.model,
                    t.acceptance_json
             FROM tasks t JOIN items i ON i.id = t.item_id
             WHERE t.id = ?1",
            params![task_id],
            |row| Ok((read_task_row(row)?, row.get::<_, String>(11)?)),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("任务不存在：{task_id}"))?;

    let criteria = parse_criteria(&acceptance_json)?;

    let mut run_stmt = conn
        .prepare(
            "SELECT id, status, adapter_id, branch, worktree_path, started_at, ended_at, error,
                    pr_number, pr_url, pr_state
             FROM runs WHERE task_id = ?1 ORDER BY attempt ASC",
        )
        .map_err(db_err)?;
    let runs = run_stmt
        .query_map(params![task_id], read_run_row)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    let mut evidence_stmt = conn
        .prepare(
            "SELECT e.id, e.criterion_id, e.kind, e.cmd, e.exit_code, e.duration_ms,
                    e.output_path, e.collected_at
             FROM evidence e JOIN runs r ON r.id = e.run_id
             WHERE r.task_id = ?1
             ORDER BY e.collected_at ASC, e.rowid ASC",
        )
        .map_err(db_err)?;
    let evidence = evidence_stmt
        .query_map(params![task_id], read_evidence_row)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    let review = conn
        .query_row(
            "SELECT rev.verdict, rev.summary, rev.risk_flags_json, rev.test_diff_json
             FROM reviews rev JOIN runs r ON r.id = rev.run_id
             WHERE r.task_id = ?1
               AND r.attempt = (SELECT MAX(attempt) FROM runs WHERE task_id = ?1)
             LIMIT 1",
            params![task_id],
            read_review_row,
        )
        .optional()
        .map_err(db_err)?;

    let mut decision_stmt = conn
        .prepare(
            "SELECT d.id, d.task_id, d.kind, d.prompt, d.status, d.created_at, d.resolution
             FROM decisions d
             WHERE d.task_id = ?1 OR d.run_id IN (SELECT id FROM runs WHERE task_id = ?1)
             ORDER BY d.created_at ASC, d.rowid ASC",
        )
        .map_err(db_err)?;
    let decisions = decision_stmt
        .query_map(params![task_id], read_decision_row)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    let mut event_stmt = conn
        .prepare(
            "SELECT id, type, payload_json, at FROM events
             WHERE entity_id = ?1
                OR entity_id IN (SELECT id FROM runs WHERE task_id = ?1)
             ORDER BY at ASC, rowid ASC",
        )
        .map_err(db_err)?;
    let events = event_stmt
        .query_map(params![task_id], read_event_row)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    Ok(TaskDetail {
        task,
        criteria,
        runs,
        evidence,
        review,
        decisions,
        events,
    })
}

/// 任务状态迁移，唯一写入口；非法迁移返回 `Err`。
///
/// 边集合即 `docs/data-model.md` §3：`merged` 无出边；`needs_human` / `interrupted`
/// 只能迁回 `queued` / `draft`，`queued` 可取消回 `draft`（由人工触发，但走同一函数）。
/// 进入 `queued` 必须已有验收标准；`failed -> queued` 要求尝试次数未用尽。`merged`
/// 的证据门由 [`merge_ready`] 负责，调用方须先确认再迁移。
pub fn transition(conn: &Connection, task_id: &str, to: TaskStatus) -> Result<TaskStatus, String> {
    let (item_id, raw_status, attempts, acceptance_json) = conn
        .query_row(
            "SELECT t.item_id, i.status, t.attempts, t.acceptance_json
             FROM tasks t JOIN items i ON i.id = t.item_id
             WHERE t.id = ?1",
            params![task_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("任务不存在：{task_id}"))?;

    let from = TaskStatus::parse(&raw_status)?;
    if !allowed_transition(from, to) {
        return Err(format!("非法状态迁移：{from} → {to}"));
    }

    if to == TaskStatus::Queued {
        let criteria = parse_criteria(&acceptance_json)?;
        if criteria.is_empty() {
            return Err("任务缺少验收标准，不能进入 queued".to_string());
        }
    }

    if from == TaskStatus::Failed && to == TaskStatus::Queued && attempts >= MAX_ATTEMPTS {
        return Err(format!(
            "尝试次数已用尽（{attempts}/{MAX_ATTEMPTS}），需要人工处理"
        ));
    }

    let changed = conn
        .execute(
            "UPDATE items
             SET status = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?1",
            params![item_id, to.as_str()],
        )
        .map_err(db_err)?;
    if changed == 0 {
        return Err(format!("任务项不存在：{task_id}"));
    }

    Ok(to)
}

/// §3 的迁移表；`Merged` 是终态，没有出边。
fn allowed_transition(from: TaskStatus, to: TaskStatus) -> bool {
    use TaskStatus::*;
    matches!(
        (from, to),
        (Draft, Ready)
            | (Ready, Queued)
            | (Queued, Running)
            | (Queued, Draft)
            | (Running, Verifying)
            | (Running, Failed)
            | (Running, Interrupted)
            | (Verifying, Merged)
            | (Verifying, NeedsHuman)
            | (Verifying, Failed)
            | (Verifying, Interrupted)
            | (Failed, Queued)
            | (Failed, NeedsHuman)
            | (Interrupted, Queued)
            | (Interrupted, Draft)
            | (NeedsHuman, Queued)
            | (NeedsHuman, Draft)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::*;

    #[test]
    fn create_task_inserts_issue_item_and_criteria() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        assert_eq!(task.status, TaskStatus::Draft);
        assert_eq!(task.attempts, 0);
        assert_eq!(task.max_minutes, 30);
        assert_eq!(task.project_id, project_id);
        assert_eq!(task.prompt, "实现证据门并跑通验收");

        let detail = get_task(conn, &task.id).unwrap();
        assert_eq!(detail.criteria.len(), 2);
        assert_eq!(detail.criteria[0].id, "c1");
        assert_eq!(detail.criteria[1].expect_match.as_deref(), Some("Finished"));
        assert!(detail.runs.is_empty());
        assert!(detail.evidence.is_empty());
        assert!(detail.review.is_none());
        assert!(detail.decisions.is_empty());
        assert!(detail.events.is_empty());

        let summaries = list_tasks(conn).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, task.id);
        assert_eq!(summaries[0].criterion_count, 2);
        assert_eq!(list_tasks_for_project(conn, &project_id).unwrap().len(), 1);
        assert!(
            list_tasks_for_project(conn, "missing-project")
                .unwrap()
                .is_empty()
        );

        let kind: String = conn
            .query_row(
                "SELECT kind FROM items WHERE id = (SELECT item_id FROM tasks WHERE id = ?1)",
                [task.id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(kind, "issue");
    }

    #[test]
    fn create_task_writes_no_partial_rows_for_missing_project() {
        let db = open_db();
        let conn = db.conn();

        let err = create_task(conn, new_task("missing-project", criteria())).unwrap_err();
        assert!(err.contains("项目不存在"), "{err}");

        let items: i64 = conn
            .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
            .unwrap();
        let tasks: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
            .unwrap();
        assert_eq!((items, tasks), (0, 0));
    }

    #[test]
    fn create_task_for_item_reuses_issue_row() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();
        let item_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO items (id, project_id, kind, title, body, status, github_number)
             VALUES (?1, ?2, 'issue', '同步问题', '复现步骤', 'open', 42)",
            params![item_id, project_id],
        )
        .unwrap();

        let task = create_task_for_item(conn, &item_id, "处理同步问题", criteria()).unwrap();
        assert_eq!(task.status, TaskStatus::Draft);
        assert_eq!(task.title, "同步问题");
        assert_eq!(task.project_id, project_id);
        assert_eq!(task.prompt, "处理同步问题");
        assert_eq!(get_task(conn, &task.id).unwrap().criteria.len(), 2);

        let (status, body, github_number): (String, String, i64) = conn
            .query_row(
                "SELECT status, body, github_number FROM items WHERE id = ?1",
                params![item_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "draft");
        assert_eq!(body, "复现步骤");
        assert_eq!(github_number, 42);

        // 条目已有关联任务时必须报错，而不是再插入一条 tasks 行。
        let err = create_task_for_item(conn, &item_id, "再次转换", criteria()).unwrap_err();
        assert!(err.contains("已有任务"), "{err}");
        let tasks: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tasks, 1);
    }

    #[test]
    fn create_docs_task_queues_linked_auto_docs_task() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();
        let parent = create_task(conn, new_task(&project_id, criteria())).unwrap();

        let docs = create_docs_task(
            conn,
            &project_id,
            &parent.id,
            "文档：实现证据门",
            "更新文档",
            criteria(),
        )
        .unwrap();

        assert_eq!(docs.task.status, TaskStatus::Queued);
        assert_eq!(docs.task.project_id, project_id);
        assert_eq!(docs.task.title, "文档：实现证据门");
        assert_eq!(docs.task.prompt, "更新文档");
        assert_eq!(docs.criteria.len(), 2);
        assert!(task_auto_docs(conn, &docs.task.id).unwrap());

        let (kind, status): (String, String) = conn
            .query_row(
                "SELECT kind, status FROM items
                 WHERE id = (SELECT item_id FROM tasks WHERE id = ?1)",
                params![docs.task.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((kind.as_str(), status.as_str()), ("issue", "queued"));

        let linked: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM links l
                     JOIN tasks parent ON parent.item_id = l.from_item
                     JOIN tasks docs ON docs.item_id = l.to_item
                     WHERE parent.id = ?1 AND docs.id = ?2 AND l.rel = 'derived_from'
                 )",
                params![parent.id, docs.task.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            linked,
            "父任务 item 到文档任务 item 必须有 derived_from 链接"
        );
        assert!(docs.events.is_empty());
    }

    #[test]
    fn list_tasks_carries_auto_docs_flag() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();
        let parent = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let docs = create_docs_task(
            conn,
            &project_id,
            &parent.id,
            "文档：实现证据门",
            "更新文档",
            criteria(),
        )
        .unwrap();

        let summaries = list_tasks_for_project(conn, &project_id).unwrap();
        assert_eq!(summaries.len(), 2);
        let parent_summary = summaries
            .iter()
            .find(|summary| summary.id == parent.id)
            .unwrap();
        assert!(!parent_summary.auto_docs);
        let docs_summary = summaries
            .iter()
            .find(|summary| summary.id == docs.task.id)
            .unwrap();
        assert!(docs_summary.auto_docs);

        // 依赖视图与列表共用行读取器，同样必须读到 auto_docs 列。
        add_dependency(conn, &docs.task.id, &parent.id).unwrap();
        let (blockers, _) = task_dependencies(conn, &docs.task.id).unwrap();
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].id, parent.id);
        assert!(!blockers[0].auto_docs);
        let (_, blocks) = task_dependencies(conn, &parent.id).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].id, docs.task.id);
        assert!(blocks[0].auto_docs);
    }

    #[test]
    fn create_docs_task_rejects_missing_inputs_and_empty_criteria() {
        let db = open_db();
        let project_id = project(&db);
        let other_project = project(&db);
        let conn = db.conn();
        let parent = create_task(conn, new_task(&project_id, criteria())).unwrap();

        let missing_project =
            create_docs_task(conn, "missing-project", &parent.id, "文档", "p", criteria())
                .unwrap_err();
        assert!(missing_project.contains("项目不存在"), "{missing_project}");

        let missing_parent =
            create_docs_task(conn, &project_id, "missing-task", "文档", "p", criteria())
                .unwrap_err();
        assert!(missing_parent.contains("任务不存在"), "{missing_parent}");

        let mismatched =
            create_docs_task(conn, &other_project, &parent.id, "文档", "p", criteria())
                .unwrap_err();
        assert!(mismatched.contains("父任务不属于该项目"), "{mismatched}");

        let empty =
            create_docs_task(conn, &project_id, &parent.id, "文档", "p", Vec::new()).unwrap_err();
        assert!(empty.contains("验收标准"), "{empty}");

        let mut invalid = criteria();
        invalid[0].id = "bad id".into();
        let invalid =
            create_docs_task(conn, &project_id, &parent.id, "文档", "p", invalid).unwrap_err();
        assert!(invalid.contains("id 非法"), "{invalid}");

        let (items, tasks, links): (i64, i64, i64) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM items),
                        (SELECT COUNT(*) FROM tasks),
                        (SELECT COUNT(*) FROM links)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((items, tasks, links), (1, 1, 0), "失败时不得写入任何行");
    }

    #[test]
    fn create_task_for_item_rejects_non_issue_and_invalid_criteria() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let note = insert_item(conn, &project_id, "note", "笔记", "正文");
        let err = create_task_for_item(conn, &note, "p", criteria()).unwrap_err();
        assert!(err.contains("issue"), "{err}");

        let issue = insert_item(conn, &project_id, "issue", "问题", "正文");
        let mut invalid = criteria();
        invalid[0].id = "bad id".into();
        let err = create_task_for_item(conn, &issue, "p", invalid).unwrap_err();
        assert!(err.contains("id 非法"), "{err}");

        let missing = create_task_for_item(conn, "missing-item", "p", criteria()).unwrap_err();
        assert!(missing.contains("条目不存在"), "{missing}");

        let (tasks, status): (i64, String) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM tasks), status FROM items WHERE id = ?1",
                params![issue],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(tasks, 0);
        assert_eq!(status, "open", "校验失败时不应改写 item 状态");
    }

    #[test]
    fn queued_requires_acceptance_criteria() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let empty = create_task(conn, new_task(&project_id, vec![])).unwrap();
        // draft -> queued 本身非法
        let err = transition(conn, &empty.id, TaskStatus::Queued).unwrap_err();
        assert!(err.contains("非法状态迁移"), "{err}");

        transition(conn, &empty.id, TaskStatus::Ready).unwrap();
        let err = transition(conn, &empty.id, TaskStatus::Queued).unwrap_err();
        assert!(err.contains("验收标准"), "{err}");
        assert_eq!(
            get_task(conn, &empty.id).unwrap().task.status,
            TaskStatus::Ready
        );

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        transition(conn, &task.id, TaskStatus::Ready).unwrap();
        assert_eq!(
            transition(conn, &task.id, TaskStatus::Queued).unwrap(),
            TaskStatus::Queued
        );
    }

    #[test]
    fn queued_can_cancel_to_draft_but_merged_stays_terminal() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let id = task.id.as_str();
        transition(conn, id, TaskStatus::Ready).unwrap();
        transition(conn, id, TaskStatus::Queued).unwrap();
        assert_eq!(
            transition(conn, id, TaskStatus::Draft).unwrap(),
            TaskStatus::Draft
        );
        assert_eq!(get_task(conn, id).unwrap().task.status, TaskStatus::Draft);
        assert_eq!(next_queued_task(conn).unwrap(), None);

        // 取消后可重新入队并走到终态
        ready_to_running(conn, id);
        transition(conn, id, TaskStatus::Verifying).unwrap();
        transition(conn, id, TaskStatus::Merged).unwrap();
        let err = transition(conn, id, TaskStatus::Queued).unwrap_err();
        assert!(err.contains("非法状态迁移"), "{err}");
        assert!(transition(conn, id, TaskStatus::Draft).is_err());
    }

    #[test]
    fn transition_edges_match_documented_state_machine() {
        use TaskStatus::*;
        const ALL: [TaskStatus; 9] = [
            Draft,
            Ready,
            Queued,
            Running,
            Verifying,
            Merged,
            Failed,
            NeedsHuman,
            Interrupted,
        ];
        let allowed = [
            (Draft, Ready),
            (Ready, Queued),
            (Queued, Running),
            (Queued, Draft),
            (Running, Verifying),
            (Running, Failed),
            (Running, Interrupted),
            (Verifying, Merged),
            (Verifying, NeedsHuman),
            (Verifying, Failed),
            (Verifying, Interrupted),
            (Failed, Queued),
            (Failed, NeedsHuman),
            (Interrupted, Queued),
            (Interrupted, Draft),
            (NeedsHuman, Queued),
            (NeedsHuman, Draft),
        ];
        for from in ALL {
            for to in ALL {
                assert_eq!(
                    allowed_transition(from, to),
                    allowed.contains(&(from, to)),
                    "{from} -> {to}"
                );
            }
        }
        // merged 是终态：没有任何出边
        assert!(ALL.iter().all(|to| !allowed_transition(Merged, *to)));
    }

    #[test]
    fn valid_transitions_pass_and_merged_is_terminal() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let id = task.id.as_str();
        assert_eq!(
            transition(conn, id, TaskStatus::Ready).unwrap(),
            TaskStatus::Ready
        );
        assert_eq!(
            transition(conn, id, TaskStatus::Queued).unwrap(),
            TaskStatus::Queued
        );
        assert_eq!(
            transition(conn, id, TaskStatus::Running).unwrap(),
            TaskStatus::Running
        );
        assert_eq!(
            transition(conn, id, TaskStatus::Verifying).unwrap(),
            TaskStatus::Verifying
        );
        assert_eq!(
            transition(conn, id, TaskStatus::Merged).unwrap(),
            TaskStatus::Merged
        );

        let err = transition(conn, id, TaskStatus::Queued).unwrap_err();
        assert!(err.contains("非法状态迁移"), "{err}");
        assert!(transition(conn, id, TaskStatus::Draft).is_err());
        assert_eq!(get_task(conn, id).unwrap().task.status, TaskStatus::Merged);
    }

    #[test]
    fn needs_human_and_interrupted_only_leave_for_queued_or_draft() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let id = task.id.as_str();
        ready_to_running(conn, id);
        transition(conn, id, TaskStatus::Interrupted).unwrap();
        assert!(transition(conn, id, TaskStatus::Running).is_err());
        assert_eq!(
            transition(conn, id, TaskStatus::Draft).unwrap(),
            TaskStatus::Draft
        );

        ready_to_running(conn, id);
        transition(conn, id, TaskStatus::Failed).unwrap();
        transition(conn, id, TaskStatus::NeedsHuman).unwrap();
        assert!(transition(conn, id, TaskStatus::Failed).is_err());
        assert_eq!(
            transition(conn, id, TaskStatus::Queued).unwrap(),
            TaskStatus::Queued
        );
    }

    #[test]
    fn failed_retry_is_limited_to_three_attempts() {
        assert_eq!(MAX_ATTEMPTS, 3);
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let id = task.id.as_str();

        // 第一次尝试后可重试
        transition(conn, id, TaskStatus::Ready).unwrap();
        transition(conn, id, TaskStatus::Queued).unwrap();
        transition(conn, id, TaskStatus::Running).unwrap();
        start_task_run(&db, id);
        transition(conn, id, TaskStatus::Failed).unwrap();
        assert_eq!(
            transition(conn, id, TaskStatus::Queued).unwrap(),
            TaskStatus::Queued
        );

        // 用完三次尝试后禁止自动重试
        start_task_run(&db, id);
        start_task_run(&db, id);
        assert_eq!(get_task(conn, id).unwrap().task.attempts, MAX_ATTEMPTS);
        transition(conn, id, TaskStatus::Running).unwrap();
        transition(conn, id, TaskStatus::Failed).unwrap();
        let err = transition(conn, id, TaskStatus::Queued).unwrap_err();
        assert!(err.contains("尝试次数"), "{err}");
        assert_eq!(
            transition(conn, id, TaskStatus::NeedsHuman).unwrap(),
            TaskStatus::NeedsHuman
        );
    }

    #[test]
    fn validate_criteria_rejects_invalid_ids_and_empty_cmd() {
        let valid = Criterion {
            id: "C_2-ok".into(),
            cmd: "cargo test".into(),
            expect_exit: 0,
            expect_match: None,
        };
        assert!(validate_criteria(std::slice::from_ref(&valid)).is_ok());

        let empty_cmd = Criterion {
            cmd: "   ".into(),
            ..valid.clone()
        };
        let err = validate_criteria(&[empty_cmd]).unwrap_err();
        assert!(err.contains("cmd"), "{err}");

        let empty_id = Criterion {
            id: String::new(),
            ..valid.clone()
        };
        assert!(validate_criteria(&[empty_id]).is_err());

        let spaced_id = Criterion {
            id: "bad id".into(),
            ..valid.clone()
        };
        assert!(validate_criteria(&[spaced_id]).is_err());

        let long_id = Criterion {
            id: "a".repeat(41),
            ..valid.clone()
        };
        assert!(validate_criteria(&[long_id]).is_err());

        let duplicate = Criterion {
            id: "dup".into(),
            ..valid.clone()
        };
        let err = validate_criteria(&[duplicate.clone(), duplicate]).unwrap_err();
        assert!(err.contains("重复"), "{err}");
    }
}
