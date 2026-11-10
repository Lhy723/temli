//! GitHub issue 同步、发布与项目自动化开关相关命令。

use crate::store;
use crate::{AppError, AppResult, AppState};
use serde::Deserialize;
use tauri::State;

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateIssueInput {
    pub(crate) item_id: String,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) body: Option<String>,
    /// 指派人登录名；不传表示不改动。
    #[serde(default)]
    pub(crate) assignee: Option<String>,
    /// `"open"` / `"closed"`；不传表示不改动。
    #[serde(default)]
    pub(crate) state: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AutoMergeInput {
    project_id: String,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RequireChecksInput {
    project_id: String,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AutoDocsInput {
    project_id: String,
    enabled: bool,
}

#[tauri::command]
pub(crate) fn sync_now(state: State<AppState>, project_id: String) -> AppResult<()> {
    state.sync.sync_project(&project_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn update_issue(state: State<AppState>, input: UpdateIssueInput) -> AppResult<()> {
    state
        .sync
        .update_remote_issue(
            &input.item_id,
            input.title,
            input.body,
            input.assignee,
            input.state,
        )
        .map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn publish_task(state: State<AppState>, task_id: String) -> AppResult<u64> {
    state.sync.publish_task(&task_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn get_project_issues(
    state: State<AppState>,
    project_id: String,
) -> AppResult<Vec<store::ProjectIssue>> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::list_project_issues(db.conn(), &project_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn get_auto_merge(state: State<AppState>, project_id: String) -> AppResult<bool> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::project_auto_merge(db.conn(), &project_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn set_auto_merge(state: State<AppState>, input: AutoMergeInput) -> AppResult<()> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::set_project_auto_merge(db.conn(), &input.project_id, input.enabled)
        .map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn get_require_checks(state: State<AppState>, project_id: String) -> AppResult<bool> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::project_require_checks(db.conn(), &project_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn set_require_checks(
    state: State<AppState>,
    input: RequireChecksInput,
) -> AppResult<()> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::set_project_require_checks(db.conn(), &input.project_id, input.enabled)
        .map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn get_auto_docs(state: State<AppState>, project_id: String) -> AppResult<bool> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::project_auto_docs(db.conn(), &project_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn set_auto_docs(state: State<AppState>, input: AutoDocsInput) -> AppResult<()> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::set_project_auto_docs(db.conn(), &input.project_id, input.enabled)
        .map_err(AppError::from)
}
