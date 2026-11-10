//! 项目与任务依赖相关命令。

use crate::AppState;
use crate::db::{Project, RegisterProjectInput, canonical_repo};
use crate::store::{self, TaskSummary};
use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Debug, Deserialize)]
pub(crate) struct DependencyInput {
    task_id: String,
    depends_on_task_id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProjectConcurrencyInput {
    project_id: String,
    limit: i64,
}

#[derive(Debug, Serialize)]
pub(crate) struct DependencyList {
    blockers: Vec<TaskSummary>,
    blocks: Vec<TaskSummary>,
}

#[tauri::command]
pub(crate) fn list_projects(state: State<AppState>) -> Result<Vec<Project>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    db.list_projects()
}

#[tauri::command]
pub(crate) fn register_project(
    state: State<AppState>,
    input: RegisterProjectInput,
) -> Result<Project, String> {
    let canonical = canonical_repo(&input.repo_path)?;
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    db.insert_project(input, canonical)
}

#[tauri::command]
pub(crate) fn add_dependency(state: State<AppState>, input: DependencyInput) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::add_dependency(db.conn(), &input.task_id, &input.depends_on_task_id)
}

#[tauri::command]
pub(crate) fn remove_dependency(
    state: State<AppState>,
    input: DependencyInput,
) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::remove_dependency(db.conn(), &input.task_id, &input.depends_on_task_id)
}

#[tauri::command]
pub(crate) fn list_dependencies(
    state: State<AppState>,
    task_id: String,
) -> Result<DependencyList, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let (blockers, blocks) = store::task_dependencies(db.conn(), &task_id)?;
    Ok(DependencyList { blockers, blocks })
}

#[tauri::command]
pub(crate) fn get_project_concurrency(
    state: State<AppState>,
    project_id: String,
) -> Result<i64, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::project_concurrency(db.conn(), &project_id)
}

#[tauri::command]
pub(crate) fn set_project_concurrency(
    state: State<AppState>,
    input: ProjectConcurrencyInput,
) -> Result<(), String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::set_project_concurrency(db.conn(), &input.project_id, input.limit)
}
