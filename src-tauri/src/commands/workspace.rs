//! 工作区文档、全局检索与上下文链接相关命令。

use crate::AppState;
use crate::search;
use crate::store;
use crate::workspace;
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
) -> Result<Vec<workspace::DocMeta>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    workspace::list_docs(db.conn(), &project_id)
}

#[tauri::command]
pub(crate) fn get_doc(state: State<AppState>, doc_id: String) -> Result<workspace::Doc, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    workspace::get_doc(db.conn(), &doc_id)
}

#[tauri::command]
pub(crate) fn reindex_workspace(
    state: State<AppState>,
    project_id: String,
) -> Result<usize, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let project = db
        .get_project(&project_id)?
        .ok_or_else(|| "项目不存在".to_string())?;
    workspace::scan_docs(db.conn(), &project)
}

#[tauri::command]
pub(crate) fn search_workspace(
    state: State<AppState>,
    project_id: String,
    query: String,
) -> Result<Vec<workspace::SearchHit>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    workspace::search(db.conn(), &project_id, &query)
}

#[tauri::command]
pub(crate) fn global_search(
    state: State<AppState>,
    input: GlobalSearchInput,
) -> Result<Vec<search::GlobalHit>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    search::global_search(
        db.conn(),
        &input.query,
        input.project_id.as_deref(),
        input.limit.unwrap_or(search::DEFAULT_LIMIT),
    )
}

#[tauri::command]
pub(crate) fn link_context(state: State<AppState>, input: ContextLinkInput) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let task_item = store::task_item_id(db.conn(), &input.task_id)?;
    store::link_item(db.conn(), &task_item, &input.item_id, "relates")
}

#[tauri::command]
pub(crate) fn unlink_context(
    state: State<AppState>,
    input: ContextLinkInput,
) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let task_item = store::task_item_id(db.conn(), &input.task_id)?;
    store::unlink_item(db.conn(), &task_item, &input.item_id, "relates")
}

#[tauri::command]
pub(crate) fn list_context(
    state: State<AppState>,
    task_id: String,
) -> Result<Vec<store::ContextMeta>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::task_context_meta(db.conn(), &task_id)
}
