mod db;

use db::{canonical_repo, Db, Project, RegisterProjectInput};
use std::sync::{Arc, Mutex};
use tauri::{Manager, State};

struct AppState {
    db: Arc<Mutex<Db>>,
}

#[tauri::command]
fn list_projects(state: State<AppState>) -> Result<Vec<Project>, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    db.list_projects()
}

#[tauri::command]
fn register_project(
    state: State<AppState>,
    input: RegisterProjectInput,
) -> Result<Project, String> {
    let canonical = canonical_repo(&input.repo_path)?;
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    db.insert_project(input, canonical)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db = Db::open(&dir.join("temli.db")).map_err(std::io::Error::other)?;
            app.manage(AppState {
                db: Arc::new(Mutex::new(db)),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![list_projects, register_project])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
