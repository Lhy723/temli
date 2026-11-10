//! 工作区文档、全局检索与上下文链接相关命令。

use crate::search;
use crate::store;
use crate::workspace;
use crate::{AppError, AppResult, AppState};
use serde::Deserialize;
use tauri::State;

#[derive(Debug, Deserialize)]
pub(crate) struct GlobalSearchInput {
    query: String,
    /// 缺省 = 跨所有项目检索。
    #[serde(default)]
    project_id: Option<String>,
    /// 缺省 = 20，服务端再夹到 1..=50。
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ContextLinkInput {
    task_id: String,
    item_id: String,
}

#[tauri::command]
pub(crate) fn list_docs(
    state: State<AppState>,
    project_id: String,
) -> AppResult<Vec<workspace::DocMeta>> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    workspace::list_docs(db.conn(), &project_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn get_doc(state: State<AppState>, doc_id: String) -> AppResult<workspace::Doc> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    workspace::get_doc(db.conn(), &doc_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn reindex_workspace(state: State<AppState>, project_id: String) -> AppResult<usize> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let project = db
        .get_project(&project_id)?
        .ok_or_else(|| AppError::not_found("项目不存在"))?;
    workspace::scan_docs(db.conn(), &project).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn search_workspace(
    state: State<AppState>,
    project_id: String,
    query: String,
) -> AppResult<Vec<workspace::SearchHit>> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    workspace::search(db.conn(), &project_id, &query).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn global_search(
    state: State<AppState>,
    input: GlobalSearchInput,
) -> AppResult<Vec<search::GlobalHit>> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    search::global_search(
        db.conn(),
        &input.query,
        input.project_id.as_deref(),
        input.limit.unwrap_or(search::DEFAULT_LIMIT),
    )
    .map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn link_context(state: State<AppState>, input: ContextLinkInput) -> AppResult<()> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let task_item = store::task_item_id(db.conn(), &input.task_id)?;
    store::link_item(db.conn(), &task_item, &input.item_id, "relates").map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn unlink_context(state: State<AppState>, input: ContextLinkInput) -> AppResult<()> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let task_item = store::task_item_id(db.conn(), &input.task_id)?;
    store::unlink_item(db.conn(), &task_item, &input.item_id, "relates").map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn list_context(
    state: State<AppState>,
    task_id: String,
) -> AppResult<Vec<store::ContextMeta>> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::task_context_meta(db.conn(), &task_id).map_err(AppError::from)
}
