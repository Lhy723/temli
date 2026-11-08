use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

pub mod fake;
pub mod omp;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub capabilities: Capabilities,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectResult {
    pub available: bool,
    pub version: Option<String>,
    pub auth: AuthState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    Unknown,
    Ok,
    Missing,
}

#[derive(Debug, Clone)]
pub struct RunSpec {
    pub run_id: String,
    pub cwd: PathBuf,
    pub prompt: String,
    pub model: Option<String>,
    pub max_minutes: u64,
    pub max_cost_usd: Option<f64>,
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
