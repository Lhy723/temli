//! 笔记库（vault）相关命令。

use crate::vaults;
use crate::{AppError, AppResult, AppState};
use serde::Deserialize;
use tauri::State;

#[derive(Debug, Deserialize)]
pub(crate) struct AddVaultInput {
    name: String,
    root_path: String,
    #[serde(default)]
    create: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateVaultNoteInput {
    vault_id: String,
    path: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SaveVaultNoteInput {
    #[serde(alias = "note_id")]
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

// ---------------------------------------------------------------------------
// 笔记库（vault）
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) fn add_vault(state: State<AppState>, input: AddVaultInput) -> AppResult<vaults::Vault> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::add_vault(db.conn(), &input.name, &input.root_path, input.create)
        .map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn list_vaults(state: State<AppState>) -> AppResult<Vec<vaults::Vault>> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::list_vaults(db.conn()).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn remove_vault(state: State<AppState>, vault_id: String) -> AppResult<()> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::remove_vault(db.conn(), &vault_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn index_vault(state: State<AppState>, vault_id: String) -> AppResult<usize> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let vault = vaults::get_vault(db.conn(), &vault_id)?;
    vaults::index_vault(db.conn(), &vault).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn vault_tree(
    state: State<AppState>,
    vault_id: String,
) -> AppResult<Vec<vaults::VaultEntry>> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::vault_tree(db.conn(), &vault_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn get_vault_note(
    state: State<AppState>,
    note_id: String,
) -> AppResult<vaults::VaultNote> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::get_vault_note(db.conn(), &note_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn create_vault_note(
    state: State<AppState>,
    input: CreateVaultNoteInput,
) -> AppResult<vaults::VaultNote> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let vault = vaults::get_vault(db.conn(), &input.vault_id)?;
    vaults::create_vault_note(
        db.conn(),
        &vault,
        &input.path,
        input.title.as_deref(),
        input.body.as_deref(),
    )
    .map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn save_vault_note(
    state: State<AppState>,
    input: SaveVaultNoteInput,
) -> AppResult<vaults::VaultNote> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::save_vault_note(
        db.conn(),
        &input.id,
        input.title.as_deref(),
        input.body.as_deref(),
        input.path.as_deref(),
    )
    .map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn delete_vault_note(state: State<AppState>, note_id: String) -> AppResult<()> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::delete_vault_note(db.conn(), &note_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn search_vault(
    state: State<AppState>,
    vault_id: String,
    query: String,
) -> AppResult<Vec<vaults::VaultHit>> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::search_vault(db.conn(), &vault_id, &query).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn note_backlinks(
    state: State<AppState>,
    note_id: String,
) -> AppResult<Vec<vaults::Backlink>> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::note_backlinks(db.conn(), &note_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn vault_graph(state: State<AppState>, vault_id: String) -> AppResult<vaults::Graph> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::vault_graph(db.conn(), &vault_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn vault_tags(
    state: State<AppState>,
    vault_id: String,
) -> AppResult<Vec<vaults::VaultTag>> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::vault_tags(db.conn(), &vault_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn notes_by_tag(
    state: State<AppState>,
    vault_id: String,
    tag: String,
) -> AppResult<Vec<vaults::VaultEntry>> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::notes_by_tag(db.conn(), &vault_id, &tag).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn save_vault_attachment(
    state: State<AppState>,
    vault_id: String,
    note_id: String,
    file_name: String,
    data_base64: String,
) -> AppResult<String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::save_vault_attachment(db.conn(), &vault_id, &note_id, &file_name, &data_base64)
        .map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn read_vault_attachment(
    state: State<AppState>,
    vault_id: String,
    relative_path: String,
) -> AppResult<String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::read_vault_attachment(db.conn(), &vault_id, &relative_path).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn list_vault_templates(
    state: State<AppState>,
    vault_id: String,
) -> AppResult<Vec<vaults::VaultEntry>> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::list_vault_templates(db.conn(), &vault_id).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn create_vault_note_with_content(
    state: State<AppState>,
    vault_id: String,
    path: String,
    content: String,
) -> AppResult<vaults::VaultNote> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let vault = vaults::get_vault(db.conn(), &vault_id)?;
    vaults::create_vault_note_with_content(db.conn(), &vault, &path, &content)
        .map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn set_note_properties(
    state: State<AppState>,
    note_id: String,
    properties: std::collections::BTreeMap<String, String>,
) -> AppResult<vaults::VaultNote> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::set_note_properties(db.conn(), &note_id, &properties).map_err(AppError::from)
}

#[tauri::command]
pub(crate) fn rename_vault_note(
    state: State<AppState>,
    note_id: String,
    new_path: String,
) -> AppResult<vaults::VaultNote> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    vaults::rename_vault_note(db.conn(), &note_id, &new_path).map_err(AppError::from)
}
