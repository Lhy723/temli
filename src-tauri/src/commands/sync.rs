//! GitHub issue 同步、发布与项目自动化开关相关命令。

use crate::AppState;
use crate::store;
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
pub(crate) fn sync_now(state: State<AppState>, project_id: String) -> Result<(), String> {
    state.sync.sync_project(&project_id)
}

#[tauri::command]
pub(crate) fn update_issue(state: State<AppState>, input: UpdateIssueInput) -> Result<(), String> {
    state.sync.update_remote_issue(
        &input.item_id,
        input.title,
        input.body,
        input.assignee,
        input.state,
    )
}

#[tauri::command]
pub(crate) fn publish_task(state: State<AppState>, task_id: String) -> Result<u64, String> {
    state.sync.publish_task(&task_id)
}

#[tauri::command]
pub(crate) fn get_project_issues(
    state: State<AppState>,
    project_id: String,
) -> Result<Vec<store::ProjectIssue>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::list_project_issues(db.conn(), &project_id)
}

#[tauri::command]
pub(crate) fn get_auto_merge(state: State<AppState>, project_id: String) -> Result<bool, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::project_auto_merge(db.conn(), &project_id)
}

#[tauri::command]
pub(crate) fn set_auto_merge(state: State<AppState>, input: AutoMergeInput) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::set_project_auto_merge(db.conn(), &input.project_id, input.enabled)
}

#[tauri::command]
pub(crate) fn get_require_checks(
    state: State<AppState>,
    project_id: String,
) -> Result<bool, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::project_require_checks(db.conn(), &project_id)
}

#[tauri::command]
pub(crate) fn set_require_checks(
    state: State<AppState>,
    input: RequireChecksInput,
) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::set_project_require_checks(db.conn(), &input.project_id, input.enabled)
}

#[tauri::command]
pub(crate) fn get_auto_docs(state: State<AppState>, project_id: String) -> Result<bool, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::project_auto_docs(db.conn(), &project_id)
}

#[tauri::command]
pub(crate) fn set_auto_docs(state: State<AppState>, input: AutoDocsInput) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::set_project_auto_docs(db.conn(), &input.project_id, input.enabled)
}
