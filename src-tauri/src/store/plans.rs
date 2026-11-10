use super::*;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// 计划
// ---------------------------------------------------------------------------

/// 创建计划：一个事务内写入计划、阶段与任务引用。
///
/// 校验：至少一个阶段、每阶段至少一个任务、任务存在且属于同一项目、
/// 任务尚未属于任何计划（`plan_tasks.task_id` 全局唯一）。
pub fn create_plan(conn: &Connection, input: NewPlan) -> Result<Plan, String> {
    let NewPlan {
        project_id,
        title,
        goal,
        stages,
    } = input;
    ensure_project(conn, &project_id)?;
    if stages.is_empty() {
        return Err("计划至少需要一个阶段".to_string());
    }

    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let plan_id = Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO plans (id, project_id, title, goal, status)
         VALUES (?1, ?2, ?3, ?4, 'draft')",
        params![plan_id, project_id, title, goal],
    )
    .map_err(db_err)?;

    let mut seen: HashSet<&str> = HashSet::new();
    for (index, stage) in stages.iter().enumerate() {
        if stage.task_ids.is_empty() {
            return Err(format!("阶段至少需要一个任务：{}", stage.title));
        }
        let stage_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO plan_stages (id, plan_id, seq, title) VALUES (?1, ?2, ?3, ?4)",
            params![stage_id, plan_id, index as i64 + 1, stage.title],
        )
        .map_err(db_err)?;

        for (position, task_id) in stage.task_ids.iter().enumerate() {
            if !seen.insert(task_id.as_str()) {
                return Err(format!("任务重复出现在计划中：{task_id}"));
            }
            let task_project: Option<String> = tx
                .query_row(
                    "SELECT i.project_id
                     FROM tasks t JOIN items i ON i.id = t.item_id
                     WHERE t.id = ?1",
                    params![task_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(db_err)?;
            let Some(task_project) = task_project else {
                return Err(format!("任务不存在：{task_id}"));
            };
            if task_project != project_id {
                return Err(format!("任务不属于该项目：{task_id}"));
            }
            let assigned: Option<String> = tx
                .query_row(
                    "SELECT stage_id FROM plan_tasks WHERE task_id = ?1",
                    params![task_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(db_err)?;
            if assigned.is_some() {
                return Err(format!("任务已属于其他计划：{task_id}"));
            }
            tx.execute(
                "INSERT INTO plan_tasks (stage_id, task_id, position) VALUES (?1, ?2, ?3)",
                params![stage_id, task_id, position as i64 + 1],
            )
            .map_err(db_err)?;
        }
    }

    tx.commit().map_err(db_err)?;
    get_plan(conn, &plan_id)
}

/// 项目下的计划列表（按创建时间升序），阶段状态按当前任务状态实时推导。
pub fn list_plans(conn: &Connection, project_id: &str) -> Result<Vec<Plan>, String> {
    let mut stmt = conn
        .prepare("SELECT id FROM plans WHERE project_id = ?1 ORDER BY created_at ASC, id ASC")
        .map_err(db_err)?;
    let ids = stmt
        .query_map(params![project_id], |row| row.get::<_, String>(0))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    ids.iter().map(|id| read_plan(conn, id)).collect()
}

pub fn get_plan(conn: &Connection, plan_id: &str) -> Result<Plan, String> {
    read_plan(conn, plan_id)
}

/// 开始计划：仅 `draft` 可开始，置为 `active`。
pub fn start_plan(conn: &Connection, plan_id: &str) -> Result<Plan, String> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT status FROM plans WHERE id = ?1",
            params![plan_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    let Some(raw) = raw else {
        return Err(format!("计划不存在：{plan_id}"));
    };
    if PlanStatus::parse(&raw)? != PlanStatus::Draft {
        return Err(format!("只有草稿计划可以开始：{plan_id}"));
    }
    conn.execute(
        "UPDATE plans
         SET status = 'active', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        params![plan_id],
    )
    .map_err(db_err)?;
    read_plan(conn, plan_id)
}

/// 验收阶段（不记录验收意见）；保留给调度器等既有调用方。
pub fn accept_stage(conn: &Connection, stage_id: &str) -> Result<Plan, String> {
    accept_stage_with_note(conn, stage_id, None)
}

/// 验收阶段：仅 `active` 计划的 `review` 阶段可验收（任务全部 `merged`），
/// 写入 `passed_at`；最后一个阶段通过后计划置为 `done`，否则保持 `active`。
/// 草稿计划必须先 `start_plan`，已完成或草稿计划一律拒绝。
///
/// `note` 去除首尾空白后非空时写入 `summary` / `summary_at`；`None` 或空白
/// 字符串不写入（也不清空既有意见）。
pub fn accept_stage_with_note(
    conn: &Connection,
    stage_id: &str,
    note: Option<&str>,
) -> Result<Plan, String> {
    let note = note.map(str::trim).filter(|note| !note.is_empty());
    let tx = conn.unchecked_transaction().map_err(db_err)?;
    let plan_id: Option<String> = tx
        .query_row(
            "SELECT plan_id FROM plan_stages WHERE id = ?1",
            params![stage_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    let Some(plan_id) = plan_id else {
        return Err(format!("阶段不存在：{stage_id}"));
    };

    let plan = read_plan(&tx, &plan_id)?;
    match plan.status {
        PlanStatus::Active => {}
        PlanStatus::Draft => return Err(format!("计划尚未启动：{plan_id}")),
        PlanStatus::Done => return Err(format!("计划已完成：{plan_id}")),
    }
    let stage = plan
        .stages
        .iter()
        .find(|stage| stage.id == stage_id)
        .ok_or_else(|| format!("阶段不存在：{stage_id}"))?;
    if stage.status != StageStatus::Review {
        return Err(format!("阶段未处于待验收状态：{stage_id}"));
    }

    tx.execute(
        "UPDATE plan_stages
         SET passed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
             summary = COALESCE(?2, summary),
             summary_at = CASE
                 WHEN ?2 IS NULL THEN summary_at
                 ELSE strftime('%Y-%m-%dT%H:%M:%fZ','now')
             END
         WHERE id = ?1",
        params![stage_id, note],
    )
    .map_err(db_err)?;

    let remaining: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM plan_stages WHERE plan_id = ?1 AND passed_at IS NULL",
            params![plan_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    let status = if remaining == 0 {
        PlanStatus::Done.as_str()
    } else {
        PlanStatus::Active.as_str()
    };
    tx.execute(
        "UPDATE plans
         SET status = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        params![plan_id, status],
    )
    .map_err(db_err)?;
    tx.commit().map_err(db_err)?;

    read_plan(conn, &plan_id)
}

fn read_plan(conn: &Connection, plan_id: &str) -> Result<Plan, String> {
    let (id, project_id, title, goal, raw_status, created_at, updated_at) = conn
        .query_row(
            "SELECT id, project_id, title, goal, status, created_at, updated_at
             FROM plans WHERE id = ?1",
            params![plan_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("计划不存在：{plan_id}"))?;

    let status = PlanStatus::parse(&raw_status)?;
    Ok(Plan {
        id,
        project_id,
        title,
        goal,
        status,
        stages: read_plan_stages(conn, plan_id, status)?,
        created_at,
        updated_at,
    })
}

/// 按 `seq` 读取阶段并推导状态：草稿计划一律 `locked`；开始后前序阶段
/// 全部 `passed` 才解锁当前阶段。
fn read_plan_stages(
    conn: &Connection,
    plan_id: &str,
    plan_status: PlanStatus,
) -> Result<Vec<PlanStage>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, seq, title, passed_at, summary, summary_at
             FROM plan_stages WHERE plan_id = ?1 ORDER BY seq ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![plan_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;

    let mut previous_passed = true;
    let mut stages = Vec::with_capacity(rows.len());
    for (id, seq, title, passed_at, summary, summary_at) in rows {
        let tasks = read_stage_tasks(conn, &id)?;
        let all_merged =
            !tasks.is_empty() && tasks.iter().all(|task| task.status == TaskStatus::Merged);
        let status = if passed_at.is_some() {
            StageStatus::Passed
        } else if plan_status == PlanStatus::Draft || !previous_passed {
            StageStatus::Locked
        } else if all_merged {
            StageStatus::Review
        } else {
            StageStatus::Active
        };
        previous_passed &= passed_at.is_some();
        stages.push(PlanStage {
            id,
            seq,
            title,
            status,
            tasks,
            passed_at,
            summary,
            summary_at,
        });
    }
    Ok(stages)
}

fn read_stage_tasks(conn: &Connection, stage_id: &str) -> Result<Vec<PlanTask>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT t.id, i.title, i.status
             FROM plan_tasks pt
             JOIN tasks t ON t.id = pt.task_id
             JOIN items i ON i.id = t.item_id
             WHERE pt.stage_id = ?1
             ORDER BY pt.position ASC",
        )
        .map_err(db_err)?;
    let tasks = stmt
        .query_map(params![stage_id], |row| {
            Ok(PlanTask {
                id: row.get(0)?,
                title: row.get(1)?,
                status: read_status(row, 2)?,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::*;

    fn plan_input(project_id: &str, stages: Vec<Vec<&str>>) -> NewPlan {
        NewPlan {
            project_id: project_id.to_string(),
            title: "交付 v1".into(),
            goal: "按阶段完成".into(),
            stages: stages
                .into_iter()
                .enumerate()
                .map(|(index, task_ids)| NewPlanStage {
                    title: format!("阶段 {}", index + 1),
                    task_ids: task_ids.into_iter().map(str::to_string).collect(),
                })
                .collect(),
        }
    }

    #[test]
    fn plan_creation_validates_stages_tasks_and_duplicates() {
        let db = open_db();
        let conn = db.conn();
        let project_id = project(&db);
        let other_project = project(&db);
        let first = create_queued_task(conn, &project_id, "阶段一任务");
        let second = create_queued_task(conn, &project_id, "阶段二任务");
        let foreign = create_queued_task(conn, &other_project, "其他项目任务");

        let err = create_plan(conn, plan_input(&project_id, Vec::new())).unwrap_err();
        assert!(err.contains("至少需要一个阶段"), "{err}");

        let err = create_plan(conn, plan_input(&project_id, vec![vec![]])).unwrap_err();
        assert!(err.contains("至少需要一个任务"), "{err}");

        let err = create_plan(conn, plan_input(&project_id, vec![vec!["missing"]])).unwrap_err();
        assert!(err.contains("任务不存在"), "{err}");

        let err = create_plan(
            conn,
            plan_input(&project_id, vec![vec![foreign.id.as_str()]]),
        )
        .unwrap_err();
        assert!(err.contains("不属于该项目"), "{err}");

        let err = create_plan(
            conn,
            plan_input(
                &project_id,
                vec![vec![first.id.as_str()], vec![first.id.as_str()]],
            ),
        )
        .unwrap_err();
        assert!(err.contains("重复"), "{err}");

        let plan = create_plan(
            conn,
            plan_input(
                &project_id,
                vec![vec![first.id.as_str()], vec![second.id.as_str()]],
            ),
        )
        .unwrap();
        assert_eq!(plan.status, PlanStatus::Draft);
        assert_eq!(plan.stages.len(), 2);
        assert_eq!(plan.stages[0].seq, 1);
        assert_eq!(plan.stages[0].tasks.len(), 1);
        assert_eq!(plan.stages[0].tasks[0].id, first.id);
        assert_eq!(plan.stages[1].tasks[0].id, second.id);

        // 同一任务不能同时属于两个计划。
        let err =
            create_plan(conn, plan_input(&project_id, vec![vec![first.id.as_str()]])).unwrap_err();
        assert!(err.contains("已属于其他计划"), "{err}");

        let plans = list_plans(conn, &project_id).unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].id, plan.id);
        assert_eq!(get_plan(conn, &plan.id).unwrap().id, plan.id);
        assert!(list_plans(conn, &other_project).unwrap().is_empty());
        assert!(get_plan(conn, "missing").is_err());
    }

    #[test]
    fn plan_stage_input_accepts_camel_case_task_ids() {
        let stage: NewPlanStage =
            serde_json::from_str(r#"{"title":"阶段","taskIds":["t1","t2"]}"#).unwrap();
        assert_eq!(stage.task_ids, vec!["t1".to_string(), "t2".to_string()]);

        let stage: NewPlanStage =
            serde_json::from_str(r#"{"title":"阶段","task_ids":["t1"]}"#).unwrap();
        assert_eq!(stage.task_ids, vec!["t1".to_string()]);
    }

    #[test]
    fn plan_stage_lifecycle_unlocks_reviews_and_completes() {
        let db = open_db();
        let conn = db.conn();
        let project_id = project(&db);
        let first = create_queued_task(conn, &project_id, "阶段一任务");
        let second = create_queued_task(conn, &project_id, "阶段二任务");
        let plan = create_plan(
            conn,
            plan_input(
                &project_id,
                vec![vec![first.id.as_str()], vec![second.id.as_str()]],
            ),
        )
        .unwrap();

        // 未开始时计划为 draft，所有阶段锁定。
        assert_eq!(plan.stages[0].status, StageStatus::Locked);
        assert_eq!(plan.stages[1].status, StageStatus::Locked);

        let plan = start_plan(conn, &plan.id).unwrap();
        assert_eq!(plan.status, PlanStatus::Active);
        assert!(start_plan(conn, &plan.id).is_err(), "重复开始应被拒绝");

        // 任务未全部合并时阶段不接受验收。
        let err = accept_stage(conn, &plan.stages[0].id).unwrap_err();
        assert!(err.contains("待验收"), "{err}");

        merge_task(conn, &first.id);
        let plan = get_plan(conn, &plan.id).unwrap();
        assert_eq!(plan.stages[0].status, StageStatus::Review);
        assert_eq!(plan.stages[1].status, StageStatus::Locked);

        let plan = accept_stage(conn, &plan.stages[0].id).unwrap();
        assert_eq!(plan.status, PlanStatus::Active);
        assert_eq!(plan.stages[0].status, StageStatus::Passed);
        assert!(plan.stages[0].passed_at.is_some());
        assert_eq!(plan.stages[1].status, StageStatus::Active);
        assert!(
            accept_stage(conn, &plan.stages[0].id).is_err(),
            "重复验收应被拒绝"
        );

        merge_task(conn, &second.id);
        let plan = get_plan(conn, &plan.id).unwrap();
        assert_eq!(plan.stages[1].status, StageStatus::Review);

        let plan = accept_stage(conn, &plan.stages[1].id).unwrap();
        assert_eq!(plan.status, PlanStatus::Done);
        assert_eq!(plan.stages[1].status, StageStatus::Passed);

        assert!(
            start_plan(conn, &plan.id).is_err(),
            "已完成的计划不能重新开始"
        );
        assert!(accept_stage(conn, "missing").is_err());
        assert!(start_plan(conn, "missing").is_err());
    }

    #[test]
    fn plan_draft_cannot_accept_stage_until_started() {
        let db = open_db();
        let conn = db.conn();
        let project_id = project(&db);
        let task = create_queued_task(conn, &project_id, "草稿阶段任务");
        let plan =
            create_plan(conn, plan_input(&project_id, vec![vec![task.id.as_str()]])).unwrap();

        // 任务全部合并，但计划仍是草稿：阶段保持锁定，验收被拒绝。
        merge_task(conn, &task.id);
        let plan = get_plan(conn, &plan.id).unwrap();
        assert_eq!(plan.status, PlanStatus::Draft);
        assert_eq!(plan.stages[0].status, StageStatus::Locked);

        let err = accept_stage(conn, &plan.stages[0].id).unwrap_err();
        assert!(err.contains("计划尚未启动"), "{err}");
        let unchanged = get_plan(conn, &plan.id).unwrap();
        assert_eq!(unchanged.status, PlanStatus::Draft);
        assert!(unchanged.stages[0].passed_at.is_none());

        // 启动后同一阶段进入待验收，可正常验收并完成计划。
        let plan = start_plan(conn, &plan.id).unwrap();
        assert_eq!(plan.stages[0].status, StageStatus::Review);
        let plan = accept_stage(conn, &plan.stages[0].id).unwrap();
        assert_eq!(plan.status, PlanStatus::Done);
        assert_eq!(plan.stages[0].status, StageStatus::Passed);
        assert!(
            accept_stage(conn, &plan.stages[0].id).is_err(),
            "已完成计划的阶段不能再次验收"
        );
    }

    #[test]
    fn accept_stage_records_note_once() {
        let db = open_db();
        let conn = db.conn();
        let project_id = project(&db);
        let first = create_queued_task(conn, &project_id, "阶段一任务");
        let second = create_queued_task(conn, &project_id, "阶段二任务");
        let plan = create_plan(
            conn,
            plan_input(
                &project_id,
                vec![vec![first.id.as_str()], vec![second.id.as_str()]],
            ),
        )
        .unwrap();
        let plan = start_plan(conn, &plan.id).unwrap();

        // 阶段尚未全部合并时拒绝验收，意见也不能落库。
        let err = accept_stage_with_note(conn, &plan.stages[0].id, Some("不应写入")).unwrap_err();
        assert!(err.contains("待验收"), "{err}");
        let unchanged = get_plan(conn, &plan.id).unwrap();
        assert!(unchanged.stages[0].summary.is_none());
        assert!(unchanged.stages[0].summary_at.is_none());

        merge_task(conn, &first.id);
        let plan =
            accept_stage_with_note(conn, &plan.stages[0].id, Some("  证据齐全，通过  ")).unwrap();
        assert_eq!(
            plan.stages[0].summary.as_deref(),
            Some("证据齐全，通过"),
            "意见应去除首尾空白后落库"
        );
        assert!(plan.stages[0].summary_at.is_some());
        let stored = get_plan(conn, &plan.id).unwrap();
        assert_eq!(stored.stages[0].summary.as_deref(), Some("证据齐全，通过"));
        assert_eq!(stored.stages[0].summary_at, plan.stages[0].summary_at);

        // 空白意见视为未填写：阶段仍通过，但不写 summary / summary_at。
        merge_task(conn, &second.id);
        let plan = get_plan(conn, &plan.id).unwrap();
        assert_eq!(plan.stages[1].status, StageStatus::Review);
        let plan = accept_stage_with_note(conn, &plan.stages[1].id, Some("   ")).unwrap();
        assert_eq!(plan.status, PlanStatus::Done);
        assert!(plan.stages[1].summary.is_none());
        assert!(plan.stages[1].summary_at.is_none());
    }
}
