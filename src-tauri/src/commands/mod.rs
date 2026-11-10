//! Tauri IPC 命令，按领域拆分。

pub(crate) mod plans;
pub(crate) mod projects;
pub(crate) mod settings;
pub(crate) mod sync;
pub(crate) mod tasks;
pub(crate) mod vaults;
pub(crate) mod workspace;

/// 数据库错误转字符串；各命令模块共用。
pub(crate) fn db_err(e: rusqlite::Error) -> String {
    e.to_string()
}
