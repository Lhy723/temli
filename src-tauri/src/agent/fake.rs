//! 单元测试用的假适配器。
//!
//! 不启动子进程、不调用模型，只按预设脚本在独立线程中回放事件，
//! 供上层状态机与命令层做确定性测试。句柄 id 由进程内原子计数器生成，
//! 不引入 uuid 依赖。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::{
    AgentAdapter, AgentEvent, AgentManifest, AgentState, Capabilities, Decision, EventSink,
    RiskLevel, RunSpec, RunStatus, SessionHandle,
};
#[cfg(test)]
use super::{AuthState, DetectResult};

const APPROVAL_REQUEST_ID: &str = "approval-1";
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(2);
const SUCCESS_TEXT: &str = "fake output";
const FAKE_TOOL: &str = "fake.echo";
/// 脚本开始前的短暂延迟，为“启动后立即取消”留出确定性窗口。
const STARTUP_DELAY: Duration = Duration::from_millis(20);
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 进程内唯一的会话序号，生成 `fake-<n>` 形式的句柄 id。
static NEXT_SESSION_ID: AtomicUsize = AtomicUsize::new(1);

/// 假适配器要回放的脚本。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Script {
    Success,
    Approval,
    /// 仅测试使用：由 [`FakeAdapter::with_failure`] 构造。
    #[cfg(test)]
    Failure,
}

/// 无模型、无子进程的测试适配器。
pub struct FakeAdapter {
    script: Script,
    sessions: Mutex<HashMap<String, Session>>,
}

impl Default for FakeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeAdapter {
    /// 成功脚本：Status(Running) → TextDelta → ToolStart/ToolEnd → Ended(Completed) → Status(Done)。
    pub fn new() -> Self {
        Self::with_script(Script::Success)
    }

    /// 审批脚本：发出一次审批请求并阻塞等待 `respond`，允许则完成，拒绝则取消。
    ///
    /// 仅测试使用。
    #[cfg(test)]
    pub fn with_approval() -> Self {
        Self::with_script(Script::Approval)
    }

    /// 失败脚本：发出可重试错误并以 Ended(Failed) 结束。
    ///
    /// 仅测试使用。
    #[cfg(test)]
    pub fn with_failure() -> Self {
        Self::with_script(Script::Failure)
    }

    fn with_script(script: Script) -> Self {
        Self {
            script,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn sessions(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, Session>>, String> {
        self.sessions
            .lock()
            .map_err(|_| "fake 适配器会话锁已被毒化".to_string())
    }
}

impl AgentAdapter for FakeAdapter {
    fn manifest(&self) -> AgentManifest {
        AgentManifest {
            id: "fake".to_string(),
            name: "Fake Adapter".to_string(),
            version: VERSION.to_string(),
            capabilities: Capabilities {
                streaming: true,
                approvals: true,
                cancel: true,
                usage: false,
                ..Capabilities::default()
            },
            roles: super::AgentRole::all(),
        }
    }

    /// 探测内置适配器；仅测试使用，生产代码不调用。
    #[cfg(test)]
    fn detect(&self) -> DetectResult {
        DetectResult {
            available: true,
            version: Some(VERSION.to_string()),
            auth: AuthState::Ok,
            detail: Some("内置测试适配器，不依赖外部进程".to_string()),
        }
    }

    fn start(&self, _spec: RunSpec, sink: EventSink) -> Result<SessionHandle, String> {
        let id = format!("fake-{}", NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed));
        let runtime = Arc::new(Runtime::new(sink));

        // 审批脚本在注册会话前就建好通道，保证 `respond` 不会早于等待方而丢失决策。
        let (approval, pending) = if self.script == Script::Approval {
            let (sender, receiver) = mpsc::channel();
            (
                Some(receiver),
                Some(PendingApproval {
                    request_id: APPROVAL_REQUEST_ID.to_string(),
                    sender,
                }),
            )
        } else {
            (None, None)
        };

        self.sessions()?.insert(
            id.clone(),
            Session {
                runtime: Arc::clone(&runtime),
                pending: Mutex::new(pending),
            },
        );

        let script = self.script;
        let spawned = thread::Builder::new()
            .name(format!("fake-adapter-{id}"))
            .spawn(move || run_script(script, runtime, approval));

        if let Err(err) = spawned {
            // 线程启动失败时回滚注册，避免留下永不发事件的会话。
            if let Ok(mut sessions) = self.sessions.lock() {
                sessions.remove(&id);
            }
            return Err(format!("无法启动 fake 会话线程: {err}"));
        }

        Ok(SessionHandle { id })
    }

    fn respond(
        &self,
        handle: &SessionHandle,
        request_id: &str,
        decision: Decision,
    ) -> Result<(), String> {
        let sessions = self.sessions()?;
        let session = sessions
            .get(&handle.id)
            .ok_or_else(|| format!("未知会话: {}", handle.id))?;
        let mut pending = session
            .pending
            .lock()
            .map_err(|_| "fake 审批锁已被毒化".to_string())?;

        match pending.as_ref() {
            None => return Err(format!("会话 {} 当前没有待审批请求", handle.id)),
            Some(request) if request.request_id != request_id => {
                return Err(format!(
                    "审批请求 id 不匹配: 期望 {}，收到 {request_id}",
                    request.request_id
                ));
            }
            Some(_) => {}
        }

        let Some(request) = pending.take() else {
            return Err(format!("会话 {} 当前没有待审批请求", handle.id));
        };
        request
            .sender
            .send(decision)
            .map_err(|_| "fake 审批通道已关闭".to_string())
    }

    fn cancel(&self, handle: &SessionHandle) -> Result<(), String> {
        // 先在锁内取出运行时与唤醒用通道，回调在锁外触发，避免重入死锁。
        let (runtime, approval) = {
            let sessions = self.sessions()?;
            let session = sessions
                .get(&handle.id)
                .ok_or_else(|| format!("未知会话: {}", handle.id))?;
            let approval = session
                .pending
                .lock()
                .map_err(|_| "fake 审批锁已被毒化".to_string())?
                .as_ref()
                .map(|pending| pending.sender.clone());
            (Arc::clone(&session.runtime), approval)
        };

        runtime.stop();
        // 重复取消是幂等的：`finish` 只在第一次调用时发出结束事件。
        runtime.finish(RunStatus::Cancelled);

        // 唤醒可能阻塞在审批等待中的脚本线程，让它立刻退出。
        if let Some(sender) = approval {
            let _ = sender.send(Decision::Cancel);
        }
        Ok(())
    }
}

/// 已注册会话的运行时状态。
struct Session {
    runtime: Arc<Runtime>,
    pending: Mutex<Option<PendingApproval>>,
}

struct PendingApproval {
    request_id: String,
    sender: Sender<Decision>,
}

/// 事件闸门：串行化事件发送，并保证停止/结束后不再有事件漏出。
struct Runtime {
    sink: EventSink,
    stopped: AtomicBool,
    ended: AtomicBool,
    emission: Mutex<()>,
}

impl Runtime {
    fn new(sink: EventSink) -> Self {
        Self {
            sink,
            stopped: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            emission: Mutex::new(()),
        }
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    fn should_stop(&self) -> bool {
        self.stopped.load(Ordering::SeqCst) || self.ended.load(Ordering::SeqCst)
    }

    /// 发送普通事件；会话已停止或结束时返回 false。
    fn emit(&self, event: AgentEvent) -> bool {
        let _guard = self
            .emission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.should_stop() {
            return false;
        }
        (self.sink)(event);
        true
    }

    /// 发送结束事件（Ended + 终态 Status），只有第一次调用生效。
    fn finish(&self, status: RunStatus) -> bool {
        let _guard = self
            .emission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.ended.swap(true, Ordering::SeqCst) {
            return false;
        }
        (self.sink)(AgentEvent::Ended { status });
        (self.sink)(AgentEvent::Status {
            state: terminal_state(status),
        });
        true
    }
}

fn terminal_state(status: RunStatus) -> AgentState {
    match status {
        RunStatus::Completed => AgentState::Done,
        RunStatus::Failed => AgentState::Failed,
        RunStatus::Cancelled => AgentState::Cancelled,
    }
}

fn run_script(script: Script, runtime: Arc<Runtime>, approval: Option<Receiver<Decision>>) {
    thread::sleep(STARTUP_DELAY);
    if runtime.should_stop() {
        return;
    }
    match script {
        Script::Success => run_success(&runtime),
        #[cfg(test)]
        Script::Failure => run_failure(&runtime),
        Script::Approval => match approval {
            Some(receiver) => run_approval(&runtime, receiver),
            None => {
                runtime.finish(RunStatus::Failed);
            }
        },
    }
}

fn emit_status(runtime: &Runtime, state: AgentState) -> bool {
    runtime.emit(AgentEvent::Status { state })
}

fn run_success(runtime: &Runtime) {
    if !emit_status(runtime, AgentState::Running) {
        return;
    }
    if !runtime.emit(AgentEvent::TextDelta {
        text: SUCCESS_TEXT.to_string(),
    }) {
        return;
    }
    if !runtime.emit(AgentEvent::ToolStart {
        name: FAKE_TOOL.to_string(),
        summary: "fake tool input".to_string(),
    }) {
        return;
    }
    if !runtime.emit(AgentEvent::ToolEnd {
        name: FAKE_TOOL.to_string(),
        ok: true,
    }) {
        return;
    }
    runtime.finish(RunStatus::Completed);
}

/// 仅测试使用：回放 [`Script::Failure`]。
#[cfg(test)]
fn run_failure(runtime: &Runtime) {
    if !emit_status(runtime, AgentState::Running) {
        return;
    }
    if !runtime.emit(AgentEvent::Error {
        message: "fake failure".to_string(),
        retryable: true,
    }) {
        return;
    }
    runtime.finish(RunStatus::Failed);
}

fn run_approval(runtime: &Runtime, decisions: Receiver<Decision>) {
    if !emit_status(runtime, AgentState::Running) {
        return;
    }
    if !runtime.emit(AgentEvent::ApprovalRequest {
        request_id: APPROVAL_REQUEST_ID.to_string(),
        title: "Fake 工具审批".to_string(),
        detail: "FakeAdapter 请求执行一次假命令".to_string(),
        risk: RiskLevel::Medium,
        options: vec!["allow".to_string(), "deny".to_string()],
    }) {
        return;
    }

    let decision = match decisions.recv_timeout(APPROVAL_TIMEOUT) {
        Ok(decision) => decision,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // 无人应答按取消处理，避免脚本线程永久挂起。
            runtime.finish(RunStatus::Cancelled);
            return;
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => return,
    };

    // cancel 已经通过闸门发出 Ended(Cancelled)，这里不再产生任何事件。
    if runtime.should_stop() {
        return;
    }

    match decision {
        Decision::Allow => {
            runtime.finish(RunStatus::Completed);
        }
        _ => {
            runtime.finish(RunStatus::Cancelled);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// 单次等待上限；正常路径下事件立即到达，仅失败时才会等满。
    const TEST_TIMEOUT: Duration = Duration::from_millis(100);

    fn channel_sink() -> (EventSink, Receiver<AgentEvent>) {
        let (sender, receiver) = mpsc::channel();
        let sender = Mutex::new(sender);
        let sink: EventSink = Arc::new(move |event: AgentEvent| {
            if let Ok(sender) = sender.lock() {
                let _ = sender.send(event);
            }
        });
        (sink, receiver)
    }

    fn run_spec() -> RunSpec {
        RunSpec {
            cwd: std::env::temp_dir(),
            prompt: "hello".to_string(),
            model: None,
            max_minutes: 1,
            env_allowlist: Vec::new(),
        }
    }

    fn next_event(receiver: &Receiver<AgentEvent>) -> AgentEvent {
        receiver
            .recv_timeout(TEST_TIMEOUT)
            .expect("等待 fake 事件超时")
    }

    fn expect_running(receiver: &Receiver<AgentEvent>) {
        match next_event(receiver) {
            AgentEvent::Status { state } => assert_eq!(state, AgentState::Running),
            other => panic!("期望 Status(Running)，实际 {other:?}"),
        }
    }

    fn expect_ended(receiver: &Receiver<AgentEvent>, status: RunStatus) {
        match next_event(receiver) {
            AgentEvent::Ended { status: actual } => assert_eq!(actual, status),
            other => panic!("期望 Ended({status:?})，实际 {other:?}"),
        }
    }

    fn expect_terminal_state(receiver: &Receiver<AgentEvent>, state: AgentState) {
        match next_event(receiver) {
            AgentEvent::Status { state: actual } => assert_eq!(actual, state),
            other => panic!("期望终态 Status({state:?})，实际 {other:?}"),
        }
    }

    fn expect_approval_request(receiver: &Receiver<AgentEvent>) {
        match next_event(receiver) {
            AgentEvent::ApprovalRequest {
                request_id,
                title,
                detail,
                risk,
                options,
            } => {
                assert_eq!(request_id, APPROVAL_REQUEST_ID);
                assert!(!title.is_empty());
                assert!(!detail.is_empty());
                assert_eq!(risk, RiskLevel::Medium);
                assert_eq!(options, vec!["allow".to_string(), "deny".to_string()]);
            }
            other => panic!("期望 ApprovalRequest，实际 {other:?}"),
        }
    }

    #[test]
    fn manifest_and_detect_describe_fake_adapter() {
        let adapter = FakeAdapter::new();
        let manifest = adapter.manifest();
        assert_eq!(manifest.id, "fake");
        assert!(manifest.capabilities.streaming);
        assert!(manifest.capabilities.approvals);
        assert!(manifest.capabilities.cancel);
        assert!(!manifest.capabilities.usage);

        let detect = adapter.detect();
        assert!(detect.available);
        assert_eq!(detect.auth, AuthState::Ok);
    }

    #[test]
    fn success_script_emits_expected_sequence() {
        let adapter = FakeAdapter::new();
        let (sink, receiver) = channel_sink();
        let _handle = adapter.start(run_spec(), sink).unwrap();

        expect_running(&receiver);
        match next_event(&receiver) {
            AgentEvent::TextDelta { text } => assert_eq!(text, "fake output"),
            other => panic!("期望 TextDelta，实际 {other:?}"),
        }
        match next_event(&receiver) {
            AgentEvent::ToolStart { name, summary } => {
                assert_eq!(name, FAKE_TOOL);
                assert!(!summary.is_empty());
            }
            other => panic!("期望 ToolStart，实际 {other:?}"),
        }
        match next_event(&receiver) {
            AgentEvent::ToolEnd { name, ok } => {
                assert_eq!(name, FAKE_TOOL);
                assert!(ok);
            }
            other => panic!("期望 ToolEnd，实际 {other:?}"),
        }
        expect_ended(&receiver, RunStatus::Completed);
        expect_terminal_state(&receiver, AgentState::Done);
    }

    #[test]
    fn failure_script_emits_retryable_error_then_failed() {
        let adapter = FakeAdapter::with_failure();
        let (sink, receiver) = channel_sink();
        let _handle = adapter.start(run_spec(), sink).unwrap();

        expect_running(&receiver);
        match next_event(&receiver) {
            AgentEvent::Error { message, retryable } => {
                assert!(!message.is_empty());
                assert!(retryable);
            }
            other => panic!("期望 Error，实际 {other:?}"),
        }
        expect_ended(&receiver, RunStatus::Failed);
        expect_terminal_state(&receiver, AgentState::Failed);
    }

    #[test]
    fn approval_allow_completes_run() {
        let adapter = FakeAdapter::with_approval();
        let (sink, receiver) = channel_sink();
        let handle = adapter.start(run_spec(), sink).unwrap();

        expect_running(&receiver);
        expect_approval_request(&receiver);

        adapter
            .respond(&handle, APPROVAL_REQUEST_ID, Decision::Allow)
            .unwrap();
        expect_ended(&receiver, RunStatus::Completed);
        expect_terminal_state(&receiver, AgentState::Done);
    }

    #[test]
    fn approval_deny_cancels_run() {
        let adapter = FakeAdapter::with_approval();
        let (sink, receiver) = channel_sink();
        let handle = adapter.start(run_spec(), sink).unwrap();

        expect_running(&receiver);
        expect_approval_request(&receiver);

        adapter
            .respond(&handle, APPROVAL_REQUEST_ID, Decision::Deny)
            .unwrap();
        expect_ended(&receiver, RunStatus::Cancelled);
        expect_terminal_state(&receiver, AgentState::Cancelled);
        assert!(
            receiver.recv_timeout(TEST_TIMEOUT).is_err(),
            "拒绝后不应再有任何事件"
        );
    }

    #[test]
    fn cancel_emits_cancelled_once_and_stops_events() {
        let adapter = FakeAdapter::with_approval();
        let (sink, receiver) = channel_sink();
        let handle = adapter.start(run_spec(), sink).unwrap();

        expect_running(&receiver);
        expect_approval_request(&receiver);

        adapter.cancel(&handle).unwrap();
        expect_ended(&receiver, RunStatus::Cancelled);
        expect_terminal_state(&receiver, AgentState::Cancelled);

        // 幂等：再次取消不应产生任何新事件（包括第二个 Ended）。
        adapter.cancel(&handle).unwrap();
        assert!(
            receiver.recv_timeout(TEST_TIMEOUT).is_err(),
            "取消后不应再有任何事件"
        );
    }

    #[test]
    fn unknown_handle_is_rejected() {
        let adapter = FakeAdapter::new();
        let unknown = SessionHandle {
            id: "missing".to_string(),
        };
        assert!(
            adapter
                .respond(&unknown, APPROVAL_REQUEST_ID, Decision::Allow)
                .is_err()
        );
        assert!(adapter.cancel(&unknown).is_err());
    }

    #[test]
    fn respond_without_pending_approval_is_rejected() {
        let adapter = FakeAdapter::new();
        let (sink, _receiver) = channel_sink();
        let handle = adapter.start(run_spec(), sink).unwrap();
        assert!(
            adapter
                .respond(&handle, APPROVAL_REQUEST_ID, Decision::Allow)
                .is_err()
        );
    }
}
