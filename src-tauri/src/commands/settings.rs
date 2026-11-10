//! 应用设置、默认适配器与适配器列表相关命令。

use crate::AppState;
use crate::agent::{AdapterRegistry, AgentManifest};
use crate::logging;
use crate::store;
use serde::Serialize;
use tauri::State;

/// 设置页读取的全局应用设置。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AppSettings {
    default_agent: Option<String>,
    worktree_retention_days: u32,
    data_dir: String,
    version: String,
}

#[tauri::command]
pub(crate) fn get_app_settings(state: State<AppState>) -> Result<AppSettings, String> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    Ok(AppSettings {
        default_agent: store::get_setting(db.conn(), store::SETTING_DEFAULT_AGENT)?,
        worktree_retention_days: store::worktree_retention_days(db.conn()),
        data_dir: state.data_dir.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

/// 校验适配器 id 已注册；未知 id 直接拒绝，避免写入无法运行的默认设置。
fn ensure_adapter_registered(adapters: &AdapterRegistry, agent: &str) -> Result<(), String> {
    if adapters
        .manifests()
        .iter()
        .any(|manifest| manifest.id == agent)
    {
        Ok(())
    } else {
        Err(format!("适配器未注册：{agent}"))
    }
}

/// 写入默认适配器设置；`None` 删除该键，回到启动默认值。
pub(crate) fn apply_default_agent(
    conn: &rusqlite::Connection,
    adapters: &AdapterRegistry,
    agent: Option<&str>,
) -> Result<(), String> {
    match agent {
        Some(id) => {
            ensure_adapter_registered(adapters, id)?;
            store::set_setting(conn, store::SETTING_DEFAULT_AGENT, id)
        }
        None => store::delete_setting(conn, store::SETTING_DEFAULT_AGENT),
    }
}

/// 环境变量与设置都不可用时的兜底适配器。
const FALLBACK_DEFAULT_AGENT: &str = "omp";

#[tauri::command]
pub(crate) fn set_default_agent(
    state: State<AppState>,
    agent: Option<String>,
) -> Result<(), String> {
    {
        let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
        apply_default_agent(db.conn(), &state.adapters, agent.as_deref())?;
    }

    let env_agent = std::env::var("TEMLI_AGENT").ok();
    let note = match runtime_default_agent(env_agent.as_deref(), agent.as_deref()) {
        Some(runtime) => {
            *state
                .default_agent
                .write()
                .map_err(|_| "默认适配器锁被占用".to_string())? = runtime;
            "已立即生效"
        }
        None => "TEMLI_AGENT 优先，运行时保持不变",
    };
    logging::log(format!(
        "默认 agent 设置变更：{}（{note}）",
        agent.as_deref().unwrap_or("<未设置>")
    ));
    Ok(())
}

/// 设置保存成功后运行时默认适配器应切换到的值。
///
/// `TEMLI_AGENT`（去空白后非空）存在时由环境变量锁定运行值，返回 `None` 表示
/// 不覆盖；否则跟随设置，清空设置回落到 `omp`。
pub(crate) fn runtime_default_agent(
    env_agent: Option<&str>,
    setting_agent: Option<&str>,
) -> Option<String> {
    if env_agent.map(str::trim).is_some_and(|id| !id.is_empty()) {
        return None;
    }
    Some(
        setting_agent
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .unwrap_or(FALLBACK_DEFAULT_AGENT)
            .to_string(),
    )
}

/// 校验保留天数在 `1..=90` 内；越界直接报错，不静默收敛。
pub(crate) fn validate_retention_days(days: u32) -> Result<(), String> {
    if (1..=90).contains(&days) {
        Ok(())
    } else {
        Err("保留天数需在 1 到 90 之间".to_string())
    }
}

#[tauri::command]
pub(crate) fn set_worktree_retention_days(state: State<AppState>, days: u32) -> Result<(), String> {
    validate_retention_days(days)?;
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    store::set_setting(
        db.conn(),
        store::SETTING_WORKTREE_RETENTION_DAYS,
        &days.to_string(),
    )?;
    logging::log(format!("worktree 保留天数变更：{days} 天"));
    Ok(())
}

/// 启动时的默认适配器解析：`TEMLI_AGENT` > 设置项 > `"omp"`。
///
/// 环境变量或设置里的 id 未注册时回退 `"omp"`，并在 stderr 提示；空字符串
/// 视为未设置。
pub(crate) fn resolve_default_agent(
    env_agent: Option<&str>,
    setting_agent: Option<&str>,
    manifests: &[AgentManifest],
) -> String {
    let registered = |id: &str| manifests.iter().any(|manifest| manifest.id == id);

    if let Some(id) = env_agent.map(str::trim).filter(|id| !id.is_empty()) {
        if registered(id) {
            return id.to_string();
        }
        eprintln!("TEMLI_AGENT 指定的适配器未注册，回退到 {FALLBACK_DEFAULT_AGENT}：{id}");
        return FALLBACK_DEFAULT_AGENT.to_string();
    }

    match setting_agent.map(str::trim).filter(|id| !id.is_empty()) {
        Some(id) if registered(id) => id.to_string(),
        Some(id) => {
            eprintln!("设置中的默认适配器未注册，回退到 {FALLBACK_DEFAULT_AGENT}：{id}");
            FALLBACK_DEFAULT_AGENT.to_string()
        }
        None => FALLBACK_DEFAULT_AGENT.to_string(),
    }
}

#[tauri::command]
pub(crate) fn list_adapters(state: State<AppState>) -> Vec<AgentManifest> {
    state.adapters.manifests()
}
