use super::*;
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

/// 读取任务级 agent 指派：`(developer_adapter, reviewer_adapter)`。
///
/// `None` 表示该角色未指派，由运行器回退到全局默认（审查角色回退到开发者）。
pub fn task_agents(
    conn: &Connection,
    task_id: &str,
) -> Result<(Option<String>, Option<String>), String> {
    conn.query_row(
        "SELECT developer_adapter, reviewer_adapter FROM tasks WHERE id = ?1",
        params![task_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("任务不存在：{task_id}"))
}

/// 写入任务级 agent 指派；`None` 清除指派并恢复默认。
///
/// 只校验 id 非空——适配器是否注册由运行器按注册表判断，存储层不持有注册表。
pub fn set_task_agents(
    conn: &Connection,
    task_id: &str,
    developer: Option<&str>,
    reviewer: Option<&str>,
) -> Result<(), String> {
    let developer = normalize_agent_id("开发者", developer)?;
    let reviewer = normalize_agent_id("审查者", reviewer)?;
    ensure_task(conn, task_id)?;
    conn.execute(
        "UPDATE tasks SET developer_adapter = ?2, reviewer_adapter = ?3 WHERE id = ?1",
        params![task_id, developer, reviewer],
    )
    .map_err(db_err)?;
    Ok(())
}

/// 写入任务级模型覆盖；`None` 或空白串清除覆盖，回退到适配器默认模型。
///
/// 只校验任务存在——模型名是否被具体适配器接受由运行器/适配器判断。
pub fn set_task_model(conn: &Connection, task_id: &str, model: Option<&str>) -> Result<(), String> {
    let model = match model {
        None => None,
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
    };
    ensure_task(conn, task_id)?;
    conn.execute(
        "UPDATE tasks SET model = ?2 WHERE id = ?1",
        params![task_id, model],
    )
    .map_err(db_err)?;
    Ok(())
}

fn normalize_agent_id(role: &str, id: Option<&str>) -> Result<Option<String>, String> {
    match id {
        None => Ok(None),
        Some(raw) => {
            let id = raw.trim();
            if id.is_empty() {
                Err(format!("{role}适配器 id 不能为空"))
            } else {
                Ok(Some(id.to_string()))
            }
        }
    }
}

/// 读取全局设置；键不存在时返回 `None`。
pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT value FROM app_settings WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)
}

/// 写入全局设置（存在则覆盖并刷新 `updated_at`）。
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET
             value = excluded.value,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        params![key, value],
    )
    .map_err(db_err)?;
    Ok(())
}

/// 删除全局设置；键不存在时视为成功。
pub fn delete_setting(conn: &Connection, key: &str) -> Result<(), String> {
    conn.execute("DELETE FROM app_settings WHERE key = ?1", params![key])
        .map_err(db_err)?;
    Ok(())
}

/// 孤儿 worktree 的保留天数：读不到或非法时回退 7 天，并收敛到 `1..=90`。
pub fn worktree_retention_days(conn: &Connection) -> u32 {
    get_setting(conn, SETTING_WORKTREE_RETENTION_DAYS)
        .ok()
        .flatten()
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .unwrap_or(DEFAULT_WORKTREE_RETENTION_DAYS)
        .clamp(1, 90)
}

/// 开发者模式是否开启；只有精确的 `"1"` 视为开启。
pub fn dev_mode_enabled(conn: &Connection) -> bool {
    get_setting(conn, SETTING_DEV_MODE)
        .ok()
        .flatten()
        .is_some_and(|raw| raw.trim() == "1")
}

/// 开发者 API 端口：缺失或非法时回退默认端口。
pub fn dev_port(conn: &Connection) -> u16 {
    get_setting(conn, SETTING_DEV_PORT)
        .ok()
        .flatten()
        .and_then(|raw| raw.trim().parse::<u16>().ok())
        .filter(|port| *port >= MIN_DEV_PORT)
        .unwrap_or(DEFAULT_DEV_PORT)
}

/// 读取开发者 API token；不存在时生成 UUID v4 并持久化。
pub fn ensure_dev_api_token(conn: &Connection) -> Result<String, String> {
    if let Some(token) = get_setting(conn, SETTING_DEV_API_TOKEN)?
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
    {
        return Ok(token);
    }
    let token = Uuid::new_v4().to_string();
    set_setting(conn, SETTING_DEV_API_TOKEN, &token)?;
    Ok(token)
}

/// 全库任务按状态计数，按状态名排序；没有任务时返回空列表。
pub fn task_status_counts(conn: &Connection) -> Result<Vec<(String, u32)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT i.status, COUNT(*)
             FROM tasks t JOIN items i ON i.id = t.item_id
             GROUP BY i.status
             ORDER BY i.status ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

/// 项目的并发上限；列为 NULL 时回落为 1。
pub fn project_concurrency(conn: &Connection, project_id: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(concurrency_limit, 1) FROM projects WHERE id = ?1",
        params![project_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("项目不存在：{project_id}"))
}

/// 设置项目并发上限，越界值收敛到 `1..=16`。
pub fn set_project_concurrency(
    conn: &Connection,
    project_id: &str,
    limit: i64,
) -> Result<(), String> {
    let limit = limit.clamp(1, 16);
    let changed = conn
        .execute(
            "UPDATE projects SET concurrency_limit = ?2 WHERE id = ?1",
            params![project_id, limit],
        )
        .map_err(db_err)?;
    if changed == 0 {
        return Err(format!("项目不存在：{project_id}"));
    }
    Ok(())
}

/// 项目是否启用 GitHub 自动合并；默认关闭。
pub fn project_auto_merge(conn: &Connection, project_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT COALESCE(auto_merge, 0) FROM projects WHERE id = ?1",
        params![project_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("项目不存在：{project_id}"))
}

pub fn set_project_auto_merge(
    conn: &Connection,
    project_id: &str,
    enabled: bool,
) -> Result<(), String> {
    let changed = conn
        .execute(
            "UPDATE projects SET auto_merge = ?2 WHERE id = ?1",
            params![project_id, enabled],
        )
        .map_err(db_err)?;
    if changed == 0 {
        return Err(format!("项目不存在：{project_id}"));
    }
    Ok(())
}

/// 项目是否在任务合并后自动排队文档更新任务；默认关闭。
pub fn project_auto_docs(conn: &Connection, project_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT COALESCE(auto_docs, 0) FROM projects WHERE id = ?1",
        params![project_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("项目不存在：{project_id}"))
}

pub fn set_project_auto_docs(
    conn: &Connection,
    project_id: &str,
    enabled: bool,
) -> Result<(), String> {
    let changed = conn
        .execute(
            "UPDATE projects SET auto_docs = ?2 WHERE id = ?1",
            params![project_id, enabled],
        )
        .map_err(db_err)?;
    if changed == 0 {
        return Err(format!("项目不存在：{project_id}"));
    }
    Ok(())
}

/// 项目是否强制要求 PR 检查全部通过；默认关闭，此时空检查列表视为通过。
pub fn project_require_checks(conn: &Connection, project_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT COALESCE(require_checks, 0) FROM projects WHERE id = ?1",
        params![project_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_err)?
    .ok_or_else(|| format!("项目不存在：{project_id}"))
}

pub fn set_project_require_checks(
    conn: &Connection,
    project_id: &str,
    enabled: bool,
) -> Result<(), String> {
    let changed = conn
        .execute(
            "UPDATE projects SET require_checks = ?2 WHERE id = ?1",
            params![project_id, enabled],
        )
        .map_err(db_err)?;
    if changed == 0 {
        return Err(format!("项目不存在：{project_id}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::*;

    #[test]
    fn auto_docs_settings_default_off_and_roundtrip() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        assert!(!project_auto_docs(conn, &project_id).unwrap());
        set_project_auto_docs(conn, &project_id, true).unwrap();
        assert!(project_auto_docs(conn, &project_id).unwrap());
        set_project_auto_docs(conn, &project_id, false).unwrap();
        assert!(!project_auto_docs(conn, &project_id).unwrap());

        assert!(project_auto_docs(conn, "missing-project").is_err());
        assert!(set_project_auto_docs(conn, "missing-project", true).is_err());

        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        assert!(!task_auto_docs(conn, &task.id).unwrap());
        assert!(task_auto_docs(conn, "missing-task").is_err());
    }

    #[test]
    fn task_agents_roundtrip_and_validation() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();
        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();

        // 新任务默认未指派，回退到运行器的全局默认。
        assert_eq!(task_agents(conn, &task.id).unwrap(), (None, None));
        assert_eq!(task.developer_adapter, None);
        assert_eq!(task.reviewer_adapter, None);

        set_task_agents(conn, &task.id, Some("omp"), Some("claude")).unwrap();
        assert_eq!(
            task_agents(conn, &task.id).unwrap(),
            (Some("omp".to_string()), Some("claude".to_string()))
        );
        let detail = get_task(conn, &task.id).unwrap();
        assert_eq!(detail.task.developer_adapter.as_deref(), Some("omp"));
        assert_eq!(detail.task.reviewer_adapter.as_deref(), Some("claude"));
        assert_eq!(
            list_tasks(conn).unwrap()[0].developer_adapter.as_deref(),
            Some("omp")
        );

        // 每次写入覆盖两个角色：传入 None 即清除该角色的指派。
        set_task_agents(conn, &task.id, Some("other"), None).unwrap();
        assert_eq!(
            task_agents(conn, &task.id).unwrap(),
            (Some("other".to_string()), None)
        );

        // 显式清除恢复默认。
        set_task_agents(conn, &task.id, None, None).unwrap();
        assert_eq!(task_agents(conn, &task.id).unwrap(), (None, None));

        // 空 id 不能被写入。
        assert!(set_task_agents(conn, &task.id, Some("  "), None).is_err());
        assert!(set_task_agents(conn, &task.id, None, Some("")).is_err());
        assert!(set_task_agents(conn, "missing-task", Some("omp"), None).is_err());
        assert!(task_agents(conn, "missing-task").is_err());
        assert_eq!(task_agents(conn, &task.id).unwrap(), (None, None));
    }

    #[test]
    fn task_model_roundtrip_and_clear() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();
        let task = create_task(conn, new_task(&project_id, criteria())).unwrap();
        let blocker = create_task(conn, new_task(&project_id, criteria())).unwrap();
        add_dependency(conn, &task.id, &blocker.id).unwrap();

        // 新任务默认使用适配器默认模型。
        assert_eq!(task.model, None);
        assert_eq!(get_task(conn, &task.id).unwrap().task.model, None);
        assert_eq!(list_tasks(conn).unwrap()[0].model, None);

        // 写入时去掉首尾空白；任务详情、列表与依赖摘要都能读回。
        set_task_model(conn, &blocker.id, Some("  gpt-5.2-codex  ")).unwrap();
        assert_eq!(
            get_task(conn, &blocker.id).unwrap().task.model.as_deref(),
            Some("gpt-5.2-codex")
        );
        let listed = list_tasks(conn).unwrap();
        let listed_blocker = listed.iter().find(|row| row.id == blocker.id).unwrap();
        assert_eq!(listed_blocker.model.as_deref(), Some("gpt-5.2-codex"));
        let (blockers, blocks) = task_dependencies(conn, &task.id).unwrap();
        assert!(blocks.is_empty());
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].model.as_deref(), Some("gpt-5.2-codex"));

        // 空串与 None 都清除覆盖。
        set_task_model(conn, &blocker.id, Some("   ")).unwrap();
        assert_eq!(get_task(conn, &blocker.id).unwrap().task.model, None);
        set_task_model(conn, &blocker.id, Some("claude-sonnet-4-5")).unwrap();
        set_task_model(conn, &blocker.id, None).unwrap();
        assert_eq!(get_task(conn, &blocker.id).unwrap().task.model, None);
        assert_eq!(
            list_tasks(conn)
                .unwrap()
                .iter()
                .find(|row| row.id == blocker.id)
                .unwrap()
                .model,
            None
        );

        assert!(set_task_model(conn, "missing-task", Some("m")).is_err());
    }

    #[test]
    fn concurrency_get_set_clamps_to_supported_range() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        assert_eq!(project_concurrency(conn, &project_id).unwrap(), 1);

        set_project_concurrency(conn, &project_id, 4).unwrap();
        assert_eq!(project_concurrency(conn, &project_id).unwrap(), 4);

        for invalid in [0, -7] {
            set_project_concurrency(conn, &project_id, invalid).unwrap();
            assert_eq!(project_concurrency(conn, &project_id).unwrap(), 1);
        }

        set_project_concurrency(conn, &project_id, 99).unwrap();
        assert_eq!(project_concurrency(conn, &project_id).unwrap(), 16);

        set_project_concurrency(conn, &project_id, 16).unwrap();
        assert_eq!(project_concurrency(conn, &project_id).unwrap(), 16);

        assert!(set_project_concurrency(conn, "missing-project", 2).is_err());
        assert!(project_concurrency(conn, "missing-project").is_err());
    }

    #[test]
    fn require_checks_defaults_false_and_roundtrips() {
        let db = open_db();
        let project_id = project(&db);
        let conn = db.conn();

        assert!(!project_require_checks(conn, &project_id).unwrap());

        set_project_require_checks(conn, &project_id, true).unwrap();
        assert!(project_require_checks(conn, &project_id).unwrap());

        set_project_require_checks(conn, &project_id, false).unwrap();
        assert!(!project_require_checks(conn, &project_id).unwrap());

        assert!(set_project_require_checks(conn, "missing-project", true).is_err());
        assert!(project_require_checks(conn, "missing-project").is_err());
    }

    #[test]
    fn settings_roundtrip_overwrite_and_delete() {
        let db = open_db();
        let conn = db.conn();

        assert_eq!(get_setting(conn, SETTING_DEFAULT_AGENT).unwrap(), None);

        set_setting(conn, SETTING_DEFAULT_AGENT, "omp").unwrap();
        assert_eq!(
            get_setting(conn, SETTING_DEFAULT_AGENT).unwrap().as_deref(),
            Some("omp")
        );

        // 覆盖写入并刷新 updated_at（先伪造一个旧时间戳，避免同毫秒竞争）。
        conn.execute(
            "UPDATE app_settings SET updated_at = '2000-01-01T00:00:00.000Z' WHERE key = ?1",
            params![SETTING_DEFAULT_AGENT],
        )
        .unwrap();
        set_setting(conn, SETTING_DEFAULT_AGENT, "codex").unwrap();
        assert_eq!(
            get_setting(conn, SETTING_DEFAULT_AGENT).unwrap().as_deref(),
            Some("codex")
        );
        let updated_at: String = conn
            .query_row(
                "SELECT updated_at FROM app_settings WHERE key = ?1",
                params![SETTING_DEFAULT_AGENT],
                |row| row.get(0),
            )
            .unwrap();
        assert_ne!(updated_at, "2000-01-01T00:00:00.000Z");

        delete_setting(conn, SETTING_DEFAULT_AGENT).unwrap();
        assert_eq!(get_setting(conn, SETTING_DEFAULT_AGENT).unwrap(), None);
        // 重复删除不报错。
        delete_setting(conn, SETTING_DEFAULT_AGENT).unwrap();
    }

    #[test]
    fn worktree_retention_days_defaults_and_clamps() {
        let db = open_db();
        let conn = db.conn();

        assert_eq!(worktree_retention_days(conn), 7, "未设置时回退 7 天");

        for raw in ["", "abc", "1.5", "-3"] {
            set_setting(conn, SETTING_WORKTREE_RETENTION_DAYS, raw).unwrap();
            assert_eq!(worktree_retention_days(conn), 7, "非法值回退 7 天：{raw}");
        }

        set_setting(conn, SETTING_WORKTREE_RETENTION_DAYS, "0").unwrap();
        assert_eq!(worktree_retention_days(conn), 1, "下界收敛到 1");

        set_setting(conn, SETTING_WORKTREE_RETENTION_DAYS, "365").unwrap();
        assert_eq!(worktree_retention_days(conn), 90, "上界收敛到 90");

        set_setting(conn, SETTING_WORKTREE_RETENTION_DAYS, "30").unwrap();
        assert_eq!(worktree_retention_days(conn), 30);
    }
}
