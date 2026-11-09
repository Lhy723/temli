use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
#[cfg(test)]
use std::process::{Command, Stdio};
use std::sync::Arc;
#[cfg(test)]
use std::sync::mpsc;
#[cfg(test)]
use std::thread;
#[cfg(test)]
use std::time::Duration;

pub mod claude;
pub mod codex;
pub mod fake;
pub mod omp;

/// 适配器可承担的编排角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Developer,
    Reviewer,
}

impl AgentRole {
    /// 默认角色集合：同时可做开发者与审查者。
    pub fn all() -> Vec<AgentRole> {
        vec![AgentRole::Developer, AgentRole::Reviewer]
    }
}

fn default_roles() -> Vec<AgentRole> {
    AgentRole::all()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub capabilities: Capabilities,
    /// 该适配器可承担的角色；缺省为两种角色都支持。
    #[serde(default = "default_roles")]
    pub roles: Vec<AgentRole>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    pub streaming: bool,
    pub approvals: bool,
    pub cancel: bool,
    pub usage: bool,
    pub cost_cap: bool,
    pub resume: bool,
}

/// 适配器探测结果；仅测试使用的接口（生产代码不调用 [`AgentAdapter::detect`]）。
#[cfg(test)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectResult {
    pub available: bool,
    pub version: Option<String>,
    pub auth: AuthState,
    pub detail: Option<String>,
}

/// 适配器鉴权状态；仅测试使用。
#[cfg(test)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    Unknown,
    Ok,
    Missing,
}

#[derive(Debug, Clone)]
pub struct RunSpec {
    pub cwd: PathBuf,
    pub prompt: String,
    pub model: Option<String>,
    pub max_minutes: u64,
    pub env_allowlist: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionHandle {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny,
    Cancel,
    Select(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Running,
    WaitingApproval,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Status {
        state: AgentState,
    },
    TextDelta {
        text: String,
    },
    ToolStart {
        name: String,
        summary: String,
    },
    ToolEnd {
        name: String,
        ok: bool,
    },
    ApprovalRequest {
        request_id: String,
        title: String,
        detail: String,
        risk: RiskLevel,
        options: Vec<String>,
    },
    Usage {
        tokens: Option<u64>,
        cost_usd: Option<f64>,
    },
    Error {
        message: String,
        retryable: bool,
    },
    Ended {
        status: RunStatus,
    },
}

pub type EventSink = Arc<dyn Fn(AgentEvent) + Send + Sync>;

pub trait AgentAdapter: Send + Sync {
    fn manifest(&self) -> AgentManifest;
    /// 探测外部 CLI 是否可用；仅测试使用，生产代码不调用。
    #[cfg(test)]
    fn detect(&self) -> DetectResult;
    fn start(&self, spec: RunSpec, sink: EventSink) -> Result<SessionHandle, String>;
    fn respond(
        &self,
        handle: &SessionHandle,
        request_id: &str,
        decision: Decision,
    ) -> Result<(), String>;
    fn cancel(&self, handle: &SessionHandle) -> Result<(), String>;

    /// 结束并释放一个会话；默认无操作（无子进程的适配器可不实现）。
    fn close(&self, _handle: &SessionHandle) -> Result<(), String> {
        Ok(())
    }

    /// 适配器可返回后端会话 id（如 OMP sessionId），用于落库。
    fn session_id(&self, _handle: &SessionHandle) -> Option<String> {
        None
    }
}

#[derive(Default)]
pub struct AdapterRegistry {
    adapters: BTreeMap<String, Arc<dyn AgentAdapter>>,
}

impl AdapterRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, adapter: Arc<dyn AgentAdapter>) {
        let id = adapter.manifest().id;
        self.adapters.insert(id, adapter);
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn AgentAdapter>> {
        self.adapters.get(id).cloned()
    }

    pub fn manifests(&self) -> Vec<AgentManifest> {
        self.adapters.values().map(|a| a.manifest()).collect()
    }
}

/// `--version` 探测超时。
///
/// 仅测试使用。
#[cfg(test)]
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);

/// 在独立线程中执行 `<program> --version`，主线程最多等待 5 秒。
///
/// 成功时返回第一条非空输出行作为版本号；命令可用但无输出时返回 `None`。
///
/// 仅测试使用：生产代码不调用 [`AgentAdapter::detect`]。
#[cfg(test)]
fn probe_version(program: &str) -> Result<Option<String>, String> {
    let (tx, rx) = mpsc::channel::<std::io::Result<std::process::Output>>();
    let command_program = program.to_string();
    let _ = thread::spawn(move || {
        let _ = tx.send(
            Command::new(&command_program)
                .arg("--version")
                .stdin(Stdio::null())
                .output(),
        );
    });

    let output = match rx.recv_timeout(VERSION_TIMEOUT) {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => return Err(format!("未找到 {program} 可执行文件: {error}")),
        Err(_) => {
            return Err(format!(
                "{program} --version 超时（{} 秒）",
                VERSION_TIMEOUT.as_secs()
            ));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("");
        return Err(if detail.is_empty() {
            format!("{program} --version 退出码 {:?}", output.status.code())
        } else {
            format!("{program} --version 失败: {detail}")
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Stub;

    impl AgentAdapter for Stub {
        fn manifest(&self) -> AgentManifest {
            AgentManifest {
                id: "stub".into(),
                name: "Stub".into(),
                version: "0.0.1".into(),
                capabilities: Capabilities::default(),
                roles: AgentRole::all(),
            }
        }
        fn detect(&self) -> DetectResult {
            DetectResult {
                available: true,
                version: None,
                auth: AuthState::Unknown,
                detail: None,
            }
        }
        fn start(&self, _spec: RunSpec, _sink: EventSink) -> Result<SessionHandle, String> {
            Ok(SessionHandle { id: "s".into() })
        }
        fn respond(
            &self,
            _handle: &SessionHandle,
            _request_id: &str,
            _decision: Decision,
        ) -> Result<(), String> {
            Ok(())
        }
        fn cancel(&self, _handle: &SessionHandle) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn registry_registers_and_lists_adapters() {
        let mut registry = AdapterRegistry::new();
        registry.register(Arc::new(Stub));
        assert!(registry.get("stub").is_some());
        assert!(registry.get("missing").is_none());
        assert_eq!(registry.manifests().len(), 1);
        assert_eq!(registry.manifests()[0].id, "stub");
    }
}
