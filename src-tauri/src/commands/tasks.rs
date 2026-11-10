//! 任务、决策、事件与附件相关命令。

use crate::commands::db_err;
use crate::commands::plans::generate_acceptance;
use crate::evidence;
use crate::store::{self, DecisionRow, EventRow, TaskDetail, TaskSummary};
use crate::{AppError, AppResult, AppState};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tauri::State;

#[derive(Debug, Deserialize)]
pub(crate) struct CreateTaskInput {
    project_id: String,
    title: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    acceptance: Vec<store::Criterion>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateTaskFromIssueInput {
    pub(crate) item_id: String,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResolveDecisionInput {
    id: String,
    resolution: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SetTaskAgentsInput {
    task_id: String,
    /// 缺省 = 保持不变；null / 空串 = 清除覆盖；字符串 = 指定适配器。
    #[serde(default, deserialize_with = "double_option")]
    developer_adapter: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    reviewer_adapter: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SetTaskModelInput {
    task_id: String,
    /// 缺省 / null / 空串 = 使用适配器默认模型。
    #[serde(default)]
    model: Option<String>,
}

/// 区分 JSON 字段缺省（外层 `None`）与显式 `null`（`Some(None)`）。
fn double_option<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

fn list_project_decisions(
    conn: &rusqlite::Connection,
    project_id: &str,
) -> Result<Vec<DecisionRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, task_id, kind, prompt, status, created_at, resolution
             FROM decisions
             WHERE project_id = ?1 AND status = 'open'
             ORDER BY created_at ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![project_id], |row| {
            Ok(DecisionRow {
                id: row.get(0)?,
                task_id: row.get(1)?,
                kind: row.get(2)?,
                prompt: row.get(3)?,
                status: row.get(4)?,
                created_at: row.get(5)?,
                resolution: row.get(6)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

fn list_task_events(conn: &rusqlite::Connection, task_id: &str) -> Result<Vec<EventRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, type, payload_json, at
             FROM events
             WHERE entity = 'task' AND entity_id = ?1
             ORDER BY at ASC, rowid ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![task_id], |row| {
            Ok(EventRow {
                id: row.get(0)?,
                kind: row.get(1)?,
                payload_json: row.get(2)?,
                at: row.get(3)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

#[tauri::command]
pub(crate) fn list_tasks(
    state: State<AppState>,
    project_id: String,
) -> AppResult<Vec<TaskSummary>> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::list_tasks_for_project(db.conn(), &project_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn create_task(state: State<AppState>, input: CreateTaskInput) -> AppResult<TaskDetail> {
    let project = {
        let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
        db.get_project(&input.project_id)?
            .ok_or_else(|| AppError::not_found("项目不存在"))?
    };

    let criteria = if input.acceptance.is_empty() {
        let repo = PathBuf::from(&project.repo_path);
        let commands = evidence::discover_commands(&repo);
        let default_agent = state
            .default_agent
            .read()
            .map_err(|_| "默认适配器锁被占用".to_string())?
            .clone();
        let adapter = state.adapters.get(&default_agent);
        let generated = generate_acceptance(
            adapter.as_deref(),
            &repo,
            &input.title,
            &input.prompt,
            &commands,
        );
        if generated.is_empty() {
            return Err(AppError::validation(
                "未发现可执行的测试/构建命令，请手动提供验收标准",
            ));
        }
        generated
    } else {
        input.acceptance
    };

    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let task = store::create_task(
        db.conn(),
        store::NewTask {
            project_id: input.project_id,
            title: input.title,
            prompt: input.prompt,
            criteria,
        },
    )?;
    store::get_task(db.conn(), &task.id).map_err(AppError::from)
}

/// `create_task_from_issue` 的数据库逻辑；拆出来以便直接对 `Connection` 做单测。
pub(crate) fn convert_issue_to_task(
    conn: &rusqlite::Connection,
    input: CreateTaskFromIssueInput,
) -> Result<TaskDetail, String> {
    let CreateTaskFromIssueInput {
        item_id,
        title,
        prompt,
    } = input;

    let (project_id, kind, item_title, item_body) = conn
        .query_row(
            "SELECT project_id, kind, title, body FROM items WHERE id = ?1",
            params![item_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("条目不存在：{item_id}"))?;
    if kind != "issue" {
        return Err(format!("只能将 issue 条目转换为任务：{item_id}"));
    }

    let repo_path: String = conn
        .query_row(
            "SELECT repo_path FROM projects WHERE id = ?1",
            params![project_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| format!("项目不存在：{project_id}"))?;

    // 已转换过：直接返回已有任务，保证重复调用幂等。
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM tasks WHERE item_id = ?1",
            params![item_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)?;
    if let Some(task_id) = existing {
        return store::get_task(conn, &task_id);
    }

    let criteria =
        evidence::default_acceptance(&evidence::discover_commands(Path::new(&repo_path)));
    if criteria.is_empty() {
        return Err("未发现可执行的测试/构建命令，请手动提供验收标准".to_string());
    }

    let title = title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if let Some(title) = title.as_deref() {
        conn.execute(
            "UPDATE items
             SET title = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?1",
            params![item_id, title],
        )
        .map_err(db_err)?;
    }

    let prompt = prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let fallback = title.as_deref().unwrap_or(&item_title);
            if item_body.trim().is_empty() {
                fallback.to_string()
            } else {
                item_body
            }
        });

    let task = store::create_task_for_item(conn, &item_id, &prompt, criteria)?;
    store::get_task(conn, &task.id)
}

#[tauri::command]
pub(crate) fn create_task_from_issue(
    state: State<AppState>,
    input: CreateTaskFromIssueInput,
) -> AppResult<TaskDetail> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    convert_issue_to_task(db.conn(), input).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn add_task_attachment(
    state: State<AppState>,
    task_id: String,
    file_name: String,
    data_base64: String,
) -> AppResult<store::Attachment> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::add_task_attachment(
        db.conn(),
        Path::new(&state.data_dir),
        &task_id,
        &file_name,
        &data_base64,
    )
    .map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn list_task_attachments(
    state: State<AppState>,
    task_id: String,
) -> AppResult<Vec<store::Attachment>> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::list_task_attachments(db.conn(), &task_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn read_task_attachment(
    state: State<AppState>,
    attachment_id: String,
) -> AppResult<String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::read_task_attachment(db.conn(), Path::new(&state.data_dir), &attachment_id)
        .map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn delete_task_attachment(
    state: State<AppState>,
    attachment_id: String,
) -> AppResult<()> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::delete_task_attachment(db.conn(), Path::new(&state.data_dir), &attachment_id)
        .map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn get_task(state: State<AppState>, task_id: String) -> AppResult<TaskDetail> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::get_task(db.conn(), &task_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn start_task(state: State<AppState>, task_id: String) -> AppResult<()> {
    state.scheduler.enqueue(&task_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn cancel_task(state: State<AppState>, task_id: String) -> AppResult<()> {
    state.scheduler.cancel(&task_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn list_decisions(
    state: State<AppState>,
    project_id: String,
) -> AppResult<Vec<DecisionRow>> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    list_project_decisions(db.conn(), &project_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn list_reviews(
    state: State<AppState>,
    project_id: String,
) -> AppResult<Vec<store::ReviewListItem>> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::list_reviews(db.conn(), &project_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn resolve_decision(
    state: State<AppState>,
    input: ResolveDecisionInput,
) -> AppResult<()> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let decision = store::resolve_decision(db.conn(), &input.id, &input.resolution)?;
    // Agent approvals only unblock the running session; they must not move the task.
    if decision.kind != "agent_approval"
        && let Some(task_id) = decision.task_id.as_deref()
    {
        let current = store::get_task(db.conn(), task_id)?.task.status;
        if matches!(
            current,
            store::TaskStatus::NeedsHuman | store::TaskStatus::Interrupted
        ) {
            let allow = matches!(input.resolution.as_str(), "allow" | "approve" | "retry");
            let next = if allow {
                store::TaskStatus::Queued
            } else {
                store::TaskStatus::Draft
            };
            store::transition(db.conn(), task_id, next)?;
        }
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn list_events(state: State<AppState>, task_id: String) -> AppResult<Vec<EventRow>> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    list_task_events(db.conn(), &task_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn set_task_agents(state: State<AppState>, input: SetTaskAgentsInput) -> AppResult<()> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let conn = db.conn();
    let (developer, reviewer) = store::task_agents(conn, &input.task_id)?;

    let apply = |label: &str, incoming: Option<Option<String>>, current: Option<String>| {
        match incoming {
            // 字段缺省：保持当前值。
            None => Ok(current),
            // 显式 null / 空串：清除覆盖，回退到默认适配器。
            Some(None) => Ok(None),
            Some(Some(value)) => {
                let value = value.trim();
                if value.is_empty() {
                    return Ok(None);
                }
                if state.adapters.get(value).is_none() {
                    return Err(AppError::validation(format!(
                        "{label}适配器未注册：{value}"
                    )));
                }
                Ok(Some(value.to_string()))
            }
        }
    };

    let developer = apply("开发者", input.developer_adapter, developer)?;
    let reviewer = apply("审查者", input.reviewer_adapter, reviewer)?;
    store::set_task_agents(
        conn,
        &input.task_id,
        developer.as_deref(),
        reviewer.as_deref(),
    )
    .map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn set_task_model(state: State<AppState>, input: SetTaskModelInput) -> AppResult<()> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::set_task_model(db.conn(), &input.task_id, input.model.as_deref()).map_err(AppError::from)
}
