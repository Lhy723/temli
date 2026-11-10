use super::*;
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

/// 取最旧的排队任务：`items.status='queued'`，按创建时间、任务 id 升序。
///
/// 排序键与调度器的 FIFO 语义一致；同一毫秒创建时用 `t.id` 兜底，保证结果稳定。
/// 仅测试使用（调度器走 [`next_ready_tasks`]）。
#[cfg(test)]
pub fn next_queued_task(conn: &Connection) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT t.id
         FROM tasks t JOIN items i ON i.id = t.item_id
         WHERE i.status = 'queued'
         ORDER BY i.created_at ASC, t.id ASC
         LIMIT 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)
}

// ---------------------------------------------------------------------------
// 依赖与调度
// ---------------------------------------------------------------------------

/// 登记 `task_id` 依赖 `depends_on_task_id`。
///
/// 依赖只存一条 `links(rel='blocks')` 边：`from_item` 为前置任务，
/// `to_item` 为被阻塞任务；不复制状态，读取一律走递归闭包。
pub fn add_dependency(
    conn: &Connection,
    task_id: &str,
    depends_on_task_id: &str,
) -> Result<(), String> {
    if task_id == depends_on_task_id {
        return Err("任务不能依赖自身".to_string());
    }

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let item_id = task_item_id(&tx, task_id)?;
    let depends_on_item_id = task_item_id(&tx, depends_on_task_id)?;

    let (project_id, depends_on_project_id) = tx
        .query_row(
            "SELECT task.project_id, blocker.project_id
             FROM items task, items blocker
             WHERE task.id = ?1 AND blocker.id = ?2",
            params![item_id, depends_on_item_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(db_err)?;
    if project_id != depends_on_project_id {
        return Err("依赖只能建立在同一项目的任务之间".to_string());
    }

    // 沿依赖方向（`to_item` -> `from_item`）求闭包：若前置任务已经（传递地）
    // 依赖当前任务，再加入这条边就会成环。
    let creates_cycle: bool = tx
        .query_row(
            "WITH RECURSIVE closure(item_id) AS (
                 SELECT ?1
                 UNION
                 SELECT l.from_item
                 FROM links l JOIN closure c ON l.to_item = c.item_id
                 WHERE l.rel = 'blocks'
             )
             SELECT EXISTS(SELECT 1 FROM closure WHERE item_id = ?2)",
            params![depends_on_item_id, item_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if creates_cycle {
        return Err("依赖关系会形成环".to_string());
    }

    tx.execute(
        "INSERT INTO links (id, from_item, to_item, rel)
         VALUES (?1, ?2, ?3, 'blocks')
         ON CONFLICT (from_item, to_item, rel) DO NOTHING",
        params![Uuid::new_v4().to_string(), depends_on_item_id, item_id],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)
}

/// 删除依赖边；边不存在时视为已经删除（幂等）。
pub fn remove_dependency(
    conn: &Connection,
    task_id: &str,
    depends_on_task_id: &str,
) -> Result<(), String> {
    let item_id = task_item_id(conn, task_id)?;
    let depends_on_item_id = task_item_id(conn, depends_on_task_id)?;
    conn.execute(
        "DELETE FROM links
         WHERE from_item = ?1 AND to_item = ?2 AND rel = 'blocks'",
        params![depends_on_item_id, item_id],
    )
    .map_err(db_err)?;
    Ok(())
}

/// 返回 `(blockers, blocks)`：前者是当前任务的直接前置，后者是直接依赖当前任务的任务。
pub fn task_dependencies(
    conn: &Connection,
    task_id: &str,
) -> Result<(Vec<TaskSummary>, Vec<TaskSummary>), String> {
    let item_id = task_item_id(conn, task_id)?;
    let blockers = query_related_tasks(
        conn,
        "SELECT t.id, i.title, i.status, t.attempts, t.acceptance_json, t.created_at,
                t.developer_adapter, t.reviewer_adapter, t.model, t.auto_docs
         FROM links l
         JOIN tasks t ON t.item_id = l.from_item
         JOIN items i ON i.id = t.item_id
         WHERE l.to_item = ?1 AND l.rel = 'blocks'
         ORDER BY i.created_at ASC, t.id ASC",
        &item_id,
    )?;
    let blocks = query_related_tasks(
        conn,
        "SELECT t.id, i.title, i.status, t.attempts, t.acceptance_json, t.created_at,
                t.developer_adapter, t.reviewer_adapter, t.model, t.auto_docs
         FROM links l
         JOIN tasks t ON t.item_id = l.to_item
         JOIN items i ON i.id = t.item_id
         WHERE l.from_item = ?1 AND l.rel = 'blocks'
         ORDER BY i.created_at ASC, t.id ASC",
        &item_id,
    )?;
    Ok((blockers, blocks))
}

fn query_related_tasks(
    conn: &Connection,
    sql: &str,
    item_id: &str,
) -> Result<Vec<TaskSummary>, String> {
    let mut stmt = conn.prepare(sql).map_err(db_err)?;
    let rows = stmt
        .query_map(params![item_id], read_task_summary)
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

// ---------------------------------------------------------------------------
// 知识上下文
// ---------------------------------------------------------------------------

/// `links.rel` 的合法取值，与数据库 CHECK 约束保持一致。
const LINK_RELS: &[&str] = &["derived_from", "implements", "blocks", "relates"];

fn item_project(conn: &Connection, item_id: &str) -> Result<String, String> {
    conn.query_row(
        "SELECT project_id FROM items WHERE id = ?1",
        params![item_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("条目不存在：{item_id}"))
}

/// 建立条目链接（`derived_from` / `implements` / `blocks` / `relates`）。
///
/// 只允许同一项目内的条目互相链接；重复链接按幂等处理。任务依赖边请走
/// [`add_dependency`]，那里还会做环检测。
pub fn link_item(
    conn: &Connection,
    from_item: &str,
    to_item: &str,
    rel: &str,
) -> Result<(), String> {
    if !LINK_RELS.contains(&rel) {
        return Err(format!("未知链接类型：{rel}"));
    }

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let from_project = item_project(&tx, from_item)?;
    let to_project = item_project(&tx, to_item)?;
    if from_project != to_project {
        return Err("链接只能建立在同一项目的条目之间".to_string());
    }
    tx.execute(
        "INSERT INTO links (id, from_item, to_item, rel)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (from_item, to_item, rel) DO NOTHING",
        params![Uuid::new_v4().to_string(), from_item, to_item, rel],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)
}

/// 删除条目链接；边不存在时视为已经删除（幂等）。
pub fn unlink_item(
    conn: &Connection,
    from_item: &str,
    to_item: &str,
    rel: &str,
) -> Result<(), String> {
    if !LINK_RELS.contains(&rel) {
        return Err(format!("未知链接类型：{rel}"));
    }
    conn.execute(
        "DELETE FROM links
         WHERE from_item = ?1 AND to_item = ?2 AND rel = ?3",
        params![from_item, to_item, rel],
    )
    .map_err(db_err)?;
    Ok(())
}

/// 任务条目通过 `links(rel='relates')` 关联的文档条目，按创建时间稳定排序。
///
/// 只读取数据库缓存正文，只返回与任务同项目且 `kind='doc'` 的条目；笔记已迁往
/// 独立 vault，不再注入任务上下文。派发任务时由 orchestrator 截断后注入 prompt。
pub fn task_context_meta(conn: &Connection, task_id: &str) -> Result<Vec<ContextMeta>, String> {
    let item_id = task_item_id(conn, task_id)?;
    let mut stmt = conn
        .prepare(
            "SELECT i.id, i.kind, i.title
             FROM links l
             JOIN items i ON i.id = l.to_item
             JOIN items task_item ON task_item.id = l.from_item
             WHERE l.from_item = ?1 AND l.rel = 'relates'
               AND i.project_id = task_item.project_id
               AND i.kind = 'doc'
             ORDER BY i.created_at ASC, i.id ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![item_id], |row| {
            Ok(ContextMeta {
                id: row.get(0)?,
                kind: row.get(1)?,
                title: row.get(2)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

pub fn task_context(conn: &Connection, task_id: &str) -> Result<Vec<ContextItem>, String> {
    let item_id = task_item_id(conn, task_id)?;
    let mut stmt = conn
        .prepare(
            "SELECT i.id, i.kind, i.title, i.body
             FROM links l
             JOIN items i ON i.id = l.to_item
             JOIN items task_item ON task_item.id = l.from_item
             WHERE l.from_item = ?1 AND l.rel = 'relates'
               AND i.project_id = task_item.project_id
               AND i.kind = 'doc'
             ORDER BY i.created_at ASC, i.id ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![item_id], |row| {
            Ok(ContextItem {
                id: row.get(0)?,
                kind: row.get(1)?,
                title: row.get(2)?,
                body: row.get(3)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

/// 就绪任务的共享查询：先按 `queued` 状态和可选的项目/任务范围收窄，再对
/// `links(rel='blocks')` 求传递闭包并排除仍被未 `merged` 任务阻塞的候选。
///
/// `project_id` 与 `task_id` 都是可选的收窄条件；两者都不传时返回全部就绪任务。
/// 依赖闭包直接递归，不读取任何物化副本；没有任何依赖的任务天然满足条件。
/// 排序：创建时间、任务 id 升序，与调度器的 FIFO 语义一致。
fn query_ready_tasks(
    conn: &Connection,
    project_id: Option<&str>,
    task_id: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(
            "WITH RECURSIVE queued_tasks(task_id, item_id, created_at) AS (
                 SELECT t.id, t.item_id, i.created_at
                 FROM tasks t JOIN items i ON i.id = t.item_id
                 WHERE i.status = 'queued'
                   AND (?1 IS NULL OR i.project_id = ?1)
                   AND (?2 IS NULL OR t.id = ?2)
             ),
             blockers(task_id, blocker_item) AS (
                 SELECT q.task_id, l.from_item
                 FROM queued_tasks q
                 JOIN links l ON l.to_item = q.item_id AND l.rel = 'blocks'
                 UNION
                 SELECT b.task_id, l.from_item
                 FROM blockers b
                 JOIN tasks bt ON bt.item_id = b.blocker_item
                 JOIN links l ON l.to_item = bt.item_id AND l.rel = 'blocks'
             )
             SELECT q.task_id
             FROM queued_tasks q
             WHERE NOT EXISTS (
                 SELECT 1
                 FROM blockers b
                 JOIN tasks bt ON bt.item_id = b.blocker_item
                 JOIN items bi ON bi.id = bt.item_id
                 WHERE b.task_id = q.task_id AND bi.status <> 'merged'
             )
             ORDER BY q.created_at ASC, q.task_id ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![project_id, task_id], |row| row.get(0))
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

/// 项目内最旧的已就绪任务；仅测试使用（生产走 [`next_ready_tasks`]）。
#[cfg(test)]
pub fn next_ready_task(conn: &Connection, project_id: &str) -> Result<Option<String>, String> {
    Ok(next_ready_tasks(conn, project_id)?.into_iter().next())
}

/// 项目内全部就绪的排队任务，按创建时间、任务 id 升序。
///
/// 就绪判定：关联 item 状态为 `queued`，且传递依赖闭包中不存在状态不是
/// `merged` 的任务。
pub fn next_ready_tasks(conn: &Connection, project_id: &str) -> Result<Vec<String>, String> {
    query_ready_tasks(conn, Some(project_id), None)
}

/// 判断单个任务当前是否就绪：状态为 `queued` 且传递依赖全部 `merged`。
///
/// 与 [`next_ready_task`] 复用同一谓词；任务不存在或不在 `queued` 状态时返回
/// `false`。
pub fn task_ready(conn: &Connection, task_id: &str) -> Result<bool, String> {
    Ok(!query_ready_tasks(conn, None, Some(task_id))?.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::*;

    #[test]
    fn next_queued_task_returns_oldest_queued_task() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        assert_eq!(next_queued_task(conn).unwrap(), None);

        let older = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let newer = create_task(conn, new_task(&project_id, criteria())).unwrap();

        // draft 不参与调度
        assert_eq!(next_queued_task(conn).unwrap(), None);

        for id in [&older.id, &newer.id] {
            transition(conn, id, TaskStatus::Ready).unwrap();
            transition(conn, id, TaskStatus::Queued).unwrap();
        }
        // 固定创建时间，避免毫秒级抖动影响断言
        conn.execute(
            "UPDATE items SET created_at = '2026-01-02T00:00:00.000Z'
             WHERE id = (SELECT item_id FROM tasks WHERE id = ?1)",
            [newer.id.as_str()],
        )
        .unwrap();
        conn.execute(
            "UPDATE items SET created_at = '2026-01-01T00:00:00.000Z'
             WHERE id = (SELECT item_id FROM tasks WHERE id = ?1)",
            [older.id.as_str()],
        )
        .unwrap();

        assert_eq!(
            next_queued_task(conn).unwrap().as_deref(),
            Some(older.id.as_str())
        );
        assert_eq!(task_status(conn, &older.id).unwrap(), TaskStatus::Queued);
        assert_eq!(task_status(conn, &newer.id).unwrap(), TaskStatus::Queued);
        assert!(task_status(conn, "missing-task").is_err());

        // 同一时刻按任务 id 升序兜底
        conn.execute(
            "UPDATE items SET created_at = '2026-01-01T00:00:00.000Z'",
            [],
        )
        .unwrap();
        let (first, second) = if older.id < newer.id {
            (&older, &newer)
        } else {
            (&newer, &older)
        };
        assert_eq!(
            next_queued_task(conn).unwrap().as_deref(),
            Some(first.id.as_str())
        );

        // 取消首个任务后调度下一个
        transition(conn, &first.id, TaskStatus::Draft).unwrap();
        assert_eq!(
            next_queued_task(conn).unwrap().as_deref(),
            Some(second.id.as_str())
        );
    }

    #[test]
    fn dependency_rejects_self_and_cross_project_edges() {
        let db = open_db();
        let project_id = project(&db);
        let other_project_id = project(&db);
        let conn = db.conn();

        let task = create_queued_task(conn, &project_id, "任务");
        let foreign = create_queued_task(conn, &other_project_id, "外部任务");

        let err = add_dependency(conn, &task.id, &task.id).unwrap_err();
        assert!(err.contains("自身"), "{err}");

        let err = add_dependency(conn, &task.id, &foreign.id).unwrap_err();
        assert!(err.contains("同一项目"), "{err}");

        assert!(add_dependency(conn, &task.id, "missing-task").is_err());
        assert!(add_dependency(conn, "missing-task", &task.id).is_err());
        assert!(remove_dependency(conn, &task.id, "missing-task").is_err());

        // 被拒绝的写入不能留下任何依赖边
        let edge_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM links WHERE rel = 'blocks'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(edge_count, 0);
    }

    #[test]
    fn dependency_cycle_is_rejected_and_duplicate_add_is_noop() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let a = create_queued_task(conn, &project_id, "A");
        let b = create_queued_task(conn, &project_id, "B");

        // A 依赖 B；重复登记同一条边是幂等的。
        add_dependency(conn, &a.id, &b.id).unwrap();
        add_dependency(conn, &a.id, &b.id).unwrap();

        let (blockers, blocks) = task_dependencies(conn, &a.id).unwrap();
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].id, b.id);
        assert_eq!(blockers[0].status, TaskStatus::Queued);
        assert!(blocks.is_empty());

        let (b_blockers, b_blocks) = task_dependencies(conn, &b.id).unwrap();
        assert!(b_blockers.is_empty());
        assert_eq!(b_blocks.len(), 1);
        assert_eq!(b_blocks[0].id, a.id);

        // B 依赖 A 会与已有边形成环。
        let err = add_dependency(conn, &b.id, &a.id).unwrap_err();
        assert!(err.contains("环"), "{err}");

        let edge_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM links WHERE rel = 'blocks'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(edge_count, 1);
    }

    #[test]
    fn next_ready_task_waits_for_dependencies_to_merge() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let dependent = create_queued_task(conn, &project_id, "被阻塞");
        let blocker = create_queued_task(conn, &project_id, "前置");
        add_dependency(conn, &dependent.id, &blocker.id).unwrap();
        // dependent 更旧，但只有 blocker 完成后才可调度。
        set_item_created_at(conn, &dependent.id, "2026-01-01T00:00:00.000Z");
        set_item_created_at(conn, &blocker.id, "2026-01-02T00:00:00.000Z");

        assert_eq!(
            next_ready_task(conn, &project_id).unwrap().as_deref(),
            Some(blocker.id.as_str())
        );

        transition(conn, &blocker.id, TaskStatus::Running).unwrap();
        assert_eq!(next_ready_task(conn, &project_id).unwrap(), None);

        transition(conn, &blocker.id, TaskStatus::Failed).unwrap();
        assert_eq!(next_ready_task(conn, &project_id).unwrap(), None);

        transition(conn, &blocker.id, TaskStatus::Queued).unwrap();
        assert_eq!(
            next_ready_task(conn, &project_id).unwrap().as_deref(),
            Some(blocker.id.as_str())
        );

        transition(conn, &blocker.id, TaskStatus::Running).unwrap();
        transition(conn, &blocker.id, TaskStatus::Verifying).unwrap();
        assert_eq!(next_ready_task(conn, &project_id).unwrap(), None);

        transition(conn, &blocker.id, TaskStatus::Merged).unwrap();
        assert_eq!(
            next_ready_task(conn, &project_id).unwrap().as_deref(),
            Some(dependent.id.as_str())
        );
    }

    #[test]
    fn next_ready_task_waits_for_transitive_chain() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        // C 依赖 B，B 依赖 A：C 必须等 A 和 B 都 merged。
        let c = create_queued_task(conn, &project_id, "C");
        let b = create_queued_task(conn, &project_id, "B");
        let a = create_queued_task(conn, &project_id, "A");
        add_dependency(conn, &c.id, &b.id).unwrap();
        add_dependency(conn, &b.id, &a.id).unwrap();
        set_item_created_at(conn, &c.id, "2026-01-01T00:00:00.000Z");
        set_item_created_at(conn, &b.id, "2026-01-02T00:00:00.000Z");
        set_item_created_at(conn, &a.id, "2026-01-03T00:00:00.000Z");

        assert_eq!(
            next_ready_task(conn, &project_id).unwrap().as_deref(),
            Some(a.id.as_str())
        );

        merge_task(conn, &a.id);
        assert_eq!(
            next_ready_task(conn, &project_id).unwrap().as_deref(),
            Some(b.id.as_str()),
            "C 仍被 B 传递阻塞"
        );

        merge_task(conn, &b.id);
        assert_eq!(
            next_ready_task(conn, &project_id).unwrap().as_deref(),
            Some(c.id.as_str())
        );
    }

    #[test]
    fn task_ready_waits_for_transitive_blockers_to_merge() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let a = create_queued_task(conn, &project_id, "A");
        let b = create_queued_task(conn, &project_id, "B");
        let c = create_queued_task(conn, &project_id, "C");
        add_dependency(conn, &b.id, &a.id).unwrap();
        add_dependency(conn, &c.id, &b.id).unwrap();

        assert!(task_ready(conn, &a.id).unwrap());
        assert!(!task_ready(conn, &b.id).unwrap());
        assert!(!task_ready(conn, &c.id).unwrap(), "C 被 B 传递阻塞");

        merge_task(conn, &a.id);
        assert!(task_ready(conn, &b.id).unwrap());
        assert!(!task_ready(conn, &c.id).unwrap(), "B 尚未合并");

        merge_task(conn, &b.id);
        assert!(task_ready(conn, &c.id).unwrap());

        // 非 queued 状态即使没有依赖也不就绪；不存在的任务同样返回 false。
        let draft = create_task(conn, new_task(&project_id, criteria())).unwrap();
        assert!(!task_ready(conn, &draft.id).unwrap());
        assert!(!task_ready(conn, "missing-task").unwrap());
    }

    #[test]
    fn next_ready_tasks_lists_all_ready_tasks_in_order() {
        let db = open_db();
        let project_id = project(&db);
        let other_project_id = project(&db);
        let conn = db.conn();

        let blocked = create_queued_task(conn, &project_id, "被阻塞");
        let second = create_queued_task(conn, &project_id, "第二");
        let first = create_queued_task(conn, &project_id, "第一");
        create_queued_task(conn, &other_project_id, "外部任务");
        add_dependency(conn, &blocked.id, &second.id).unwrap();
        set_item_created_at(conn, &blocked.id, "2026-01-01T00:00:00.000Z");
        set_item_created_at(conn, &second.id, "2026-01-02T00:00:00.000Z");
        set_item_created_at(conn, &first.id, "2026-01-03T00:00:00.000Z");

        // blocked 最旧但依赖 second，因此只返回两个独立任务，按创建时间排序；
        // 其它项目的任务不参与本项目的就绪列表。
        assert_eq!(
            next_ready_tasks(conn, &project_id).unwrap(),
            vec![second.id.clone(), first.id.clone()]
        );
        assert_eq!(
            next_ready_task(conn, &project_id).unwrap().as_deref(),
            Some(second.id.as_str()),
            "队首与 next_ready_task 保持一致"
        );

        merge_task(conn, &second.id);
        assert_eq!(
            next_ready_tasks(conn, &project_id).unwrap(),
            vec![blocked.id.clone(), first.id.clone()],
            "依赖合并后 blocked 按创建时间回到队首"
        );
    }

    #[test]
    fn remove_dependency_updates_readiness_immediately() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        let dependent = create_queued_task(conn, &project_id, "被阻塞");
        let blocker = create_queued_task(conn, &project_id, "前置");
        add_dependency(conn, &dependent.id, &blocker.id).unwrap();
        set_item_created_at(conn, &dependent.id, "2026-01-01T00:00:00.000Z");
        set_item_created_at(conn, &blocker.id, "2026-01-02T00:00:00.000Z");
        assert_eq!(
            next_ready_task(conn, &project_id).unwrap().as_deref(),
            Some(blocker.id.as_str())
        );

        remove_dependency(conn, &dependent.id, &blocker.id).unwrap();
        assert_eq!(
            next_ready_task(conn, &project_id).unwrap().as_deref(),
            Some(dependent.id.as_str())
        );
        let (blockers, blocks) = task_dependencies(conn, &dependent.id).unwrap();
        assert!(blockers.is_empty());
        assert!(blocks.is_empty());

        // 重复删除是幂等的。
        remove_dependency(conn, &dependent.id, &blocker.id).unwrap();
    }

    #[test]
    fn link_and_unlink_roundtrip() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();
        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let task_item = task_item_id(conn, &task.id).unwrap();
        let note = insert_item(conn, &project_id, "note", "设计笔记", "正文");

        link_item(conn, &task_item, &note, "relates").unwrap();
        // 同一条边重复写入是幂等的。
        link_item(conn, &task_item, &note, "relates").unwrap();
        let edge_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM links WHERE from_item = ?1 AND to_item = ?2",
                params![task_item, note],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(edge_count, 1);

        let err = link_item(conn, &task_item, &note, "unknown").unwrap_err();
        assert!(err.contains("链接类型"), "{err}");

        unlink_item(conn, &task_item, &note, "relates").unwrap();
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM links", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 0);

        // 重复删除同样是幂等的。
        unlink_item(conn, &task_item, &note, "relates").unwrap();
    }

    #[test]
    fn link_rejects_cross_project_and_missing_items() {
        let db = open_db();
        let project_id = project(&db);
        let other_project_id = project(&db);
        let conn = db.conn();
        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let task_item = task_item_id(conn, &task.id).unwrap();
        let foreign = insert_item(conn, &other_project_id, "doc", "外部文档", "正文");

        let err = link_item(conn, &task_item, &foreign, "relates").unwrap_err();
        assert!(err.contains("同一项目"), "{err}");
        assert!(link_item(conn, &task_item, "missing-item", "relates").is_err());
        assert!(link_item(conn, "missing-item", &foreign, "relates").is_err());

        let edge_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM links", [], |row| row.get(0))
            .unwrap();
        assert_eq!(edge_count, 0);
    }

    #[test]
    fn task_context_returns_only_linked_docs_with_bodies() {
        let db = open_db();
        let project_id = project(&db);
        let other_project_id = project(&db);
        let conn = db.conn();
        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let task_item = task_item_id(conn, &task.id).unwrap();

        // 笔记已迁往独立 vault：即使仍有关联边，也不能注入任务上下文。
        let note = insert_item(conn, &project_id, "note", "架构笔记", "笔记正文");
        let doc = insert_item(conn, &project_id, "doc", "设计文档", "文档正文");
        let unrelated = insert_item(conn, &project_id, "doc", "无关文档", "不应出现");
        let reverse = insert_item(conn, &project_id, "doc", "反本文档", "不应出现");
        let _foreign = insert_item(conn, &other_project_id, "doc", "外部文档", "不应出现");

        link_item(conn, &task_item, &note, "relates").unwrap();
        link_item(conn, &task_item, &doc, "relates").unwrap();
        // 其他关系类型、反向边和外部项目都不进入上下文。
        link_item(conn, &task_item, &unrelated, "derived_from").unwrap();
        link_item(conn, &reverse, &task_item, "relates").unwrap();

        let context = task_context(conn, &task.id).unwrap();
        assert_eq!(context.len(), 1);
        assert_eq!(context[0].id, doc);
        assert_eq!(context[0].kind, "doc");
        assert_eq!(context[0].title, "设计文档");
        assert_eq!(context[0].body, "文档正文");

        let meta = task_context_meta(conn, &task.id).unwrap();
        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0].id, doc);

        assert!(task_context(conn, "missing-task").is_err());
    }
}
