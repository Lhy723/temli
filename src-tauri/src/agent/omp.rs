//! OMP 适配器：以 RPC 模式通过 stdio（JSONL）驱动 `omp` CLI。
//!
//! 帧映射被抽成纯函数 [`map_frame_with_state`]（无状态入口 `map_frame` 仅测试
//! 使用），审批回复编码抽成 [`approval_response_frame`]，便于在不启动子进程的
//! 前提下测试。

use super::{
    AgentAdapter, AgentEvent, AgentManifest, AgentState, Capabilities, Decision, EventSink,
    RiskLevel, RunSpec, RunStatus, SessionHandle,
};
#[cfg(test)]
use super::{AuthState, DetectResult};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// `omp` RPC 协议版本。
const PROTOCOL_VERSION: u32 = 2;
/// `omp --version` 探测超时。
///
/// 仅测试使用。
#[cfg(test)]
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
/// abort 之后强制杀进程前的等待时间。
const KILL_DELAY: Duration = Duration::from_secs(2);
/// 等待握手命令（`negotiate_protocol` / `get_state`）响应的超时。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// 协议 v2 重组后单帧的最大字节数。
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
/// 协议 v2 单个 `rpc_chunk` 的最大解码字节数。
const MAX_CHUNK_BYTES: usize = 256 * 1024;
/// 协议 v2 一个序列的最大 chunk 数。
const MAX_CHUNK_COUNT: u64 = MAX_FRAME_BYTES as u64 / MAX_CHUNK_BYTES as u64;
/// `chunkId` 的最大字符数。
const MAX_CHUNK_ID_CHARS: usize = 128;
/// 审批 detail 的最大字符数。
const DETAIL_LIMIT: usize = 2000;
/// 工具摘要的最大字符数。
const SUMMARY_LIMIT: usize = 200;

#[derive(Default)]
pub struct OmpAdapter {
    sessions: Mutex<HashMap<String, SessionEntry>>,
    /// handle id -> omp `sessionId`
    session_ids: Arc<Mutex<HashMap<String, String>>>,
}

/// 待用户决策的 `extension_ui_request`：回复编码需要知道原始方法与可选项。
#[derive(Debug, Clone)]
pub struct PendingApproval {
    pub method: String,
    pub options: Vec<String>,
}

struct SessionEntry {
    stdin: Arc<Mutex<ChildStdin>>,
    child: Arc<Mutex<Child>>,
    sink: EventSink,
    /// 待审批请求的 request id -> 请求详情
    pending: Arc<Mutex<HashMap<String, PendingApproval>>>,
    /// 是否已经发出过 Ended，避免 cancel 与 EOF 重复上报。
    ended: Arc<AtomicBool>,
}

impl OmpAdapter {
    pub fn new() -> Self {
        Self::default()
    }
}

impl AgentAdapter for OmpAdapter {
    fn manifest(&self) -> AgentManifest {
        AgentManifest {
            id: "omp".to_string(),
            name: "OMP".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities: Capabilities {
                streaming: true,
                approvals: true,
                cancel: true,
                usage: true,
                cost_cap: false,
                resume: false,
            },
            roles: super::AgentRole::all(),
        }
    }

    /// 探测 `omp` 是否可用；仅测试使用，生产代码不调用。
    #[cfg(test)]
    fn detect(&self) -> DetectResult {
        match probe_version() {
            Ok(version) => {
                let detail = if version.is_none() {
                    Some("omp --version 未输出版本号".to_string())
                } else {
                    None
                };
                DetectResult {
                    available: true,
                    version,
                    auth: AuthState::Unknown,
                    detail,
                }
            }
            Err(detail) => DetectResult {
                available: false,
                version: None,
                auth: AuthState::Unknown,
                detail: Some(detail),
            },
        }
    }

    /// 启动一个 OMP RPC 会话，并按 [`RunSpec`] 约束子进程。
    ///
    /// 已应用的字段：
    /// - `cwd`：子进程工作目录；
    /// - `prompt`：握手完成后发送的 prompt 帧；
    /// - `model`：`provider/modelId` 形式时在 prompt 之前发送 `set_model` 帧；
    /// - `max_minutes`：限制等待 `ready` 帧的秒数（1–30 秒）；
    /// - `env_allowlist`：非空时清空环境，只注入白名单中实际存在的变量。
    fn start(&self, spec: RunSpec, sink: EventSink) -> Result<SessionHandle, String> {
        let handle_id = format!("omp-{}", uuid::Uuid::new_v4());
        let mut command = Command::new("omp");
        command
            .args(["--mode", "rpc"])
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if !spec.env_allowlist.is_empty() {
            command.env_clear();
            for name in &spec.env_allowlist {
                if let Ok(value) = std::env::var(name) {
                    command.env(name, value);
                }
            }
        }
        let child = command
            .spawn()
            .map_err(|error| format!("无法启动 omp: {error}"))?;

        let child = Arc::new(Mutex::new(child));
        let (stdin, stdout, stderr) = {
            let mut guard = child.lock().map_err(|_| "omp 进程锁被占用".to_string())?;
            let stdin = guard
                .stdin
                .take()
                .ok_or_else(|| "omp stdin 管道不可用".to_string())?;
            let stdout = guard
                .stdout
                .take()
                .ok_or_else(|| "omp stdout 管道不可用".to_string())?;
            let stderr = guard
                .stderr
                .take()
                .ok_or_else(|| "omp stderr 管道不可用".to_string())?;
            (stdin, stdout, stderr)
        };

        // 持续排空 stderr，避免子进程写满管道后阻塞。
        let _ = thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if line.is_err() {
                    break;
                }
            }
        });

        let pending = Arc::new(Mutex::new(HashMap::new()));
        let ended = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let (handshake_tx, handshake_rx) = mpsc::channel::<Result<HandshakeReply, String>>();

        {
            let session_ids = self.session_ids.clone();
            let reader_handle_id = handle_id.clone();
            let reader_pending = pending.clone();
            let reader_ended = ended.clone();
            let reader_sink = sink.clone();
            let reader_child = child.clone();
            let _ = thread::spawn(move || {
                read_stdout(
                    stdout,
                    reader_handle_id,
                    session_ids,
                    reader_pending,
                    reader_ended,
                    reader_child,
                    reader_sink,
                    ready_tx,
                    handshake_tx,
                );
            });
        }

        // 首帧必须是 ready；等待窗口由本次运行的分钟预算决定，限制在 1–30 秒。
        let ready_timeout = Duration::from_secs(spec.max_minutes.clamp(1, 30));
        match ready_rx.recv_timeout(ready_timeout) {
            Ok(Ok(())) => {}
            Ok(Err(message)) => return fail_start(&child, &ended, message),
            Err(RecvTimeoutError::Timeout) => {
                return fail_start(
                    &child,
                    &ended,
                    format!("omp 在 {} 秒内没有发送 ready 帧", ready_timeout.as_secs()),
                );
            }
            Err(RecvTimeoutError::Disconnected) => {
                return fail_start(
                    &child,
                    &ended,
                    "omp RPC 读取线程在 ready 之前退出".to_string(),
                );
            }
        }

        let stdin = Arc::new(Mutex::new(stdin));
        let negotiate = json!({
            "id": "proto",
            "type": "negotiate_protocol",
            "protocolVersion": PROTOCOL_VERSION,
        });
        if let Err(message) = write_frame(&stdin, &negotiate).and_then(|_| {
            wait_handshake(
                &handshake_rx,
                HandshakeReply::Negotiated,
                "negotiate_protocol",
            )
        }) {
            return fail_start(&child, &ended, message);
        }

        // 先等 `get_state` 成功再发 prompt：失败必须让 start 返回错误，而不是静默运行。
        let state_request = json!({ "id": "state", "type": "get_state" });
        if let Err(message) = write_frame(&stdin, &state_request)
            .and_then(|_| wait_handshake(&handshake_rx, HandshakeReply::State, "get_state"))
        {
            return fail_start(&child, &ended, message);
        }

        let model_request = spec.model.as_deref().and_then(set_model_frame);
        let prompt_request = json!({
            "id": "prompt",
            "type": "prompt",
            "message": spec.prompt,
        });
        let handshake = match &model_request {
            Some(frame) => write_frame(&stdin, frame),
            None => Ok(()),
        }
        .and_then(|_| write_frame(&stdin, &prompt_request));
        if let Err(message) = handshake {
            return fail_start(&child, &ended, message);
        }

        let entry = SessionEntry {
            stdin,
            child,
            sink,
            pending,
            ended,
        };
        self.sessions
            .lock()
            .map_err(|_| "omp 会话表锁被占用".to_string())?
            .insert(handle_id.clone(), entry);

        Ok(SessionHandle { id: handle_id })
    }

    fn respond(
        &self,
        handle: &SessionHandle,
        request_id: &str,
        decision: Decision,
    ) -> Result<(), String> {
        let (stdin, sink, pending, ended) = {
            let sessions = self
                .sessions
                .lock()
                .map_err(|_| "omp 会话表锁被占用".to_string())?;
            let entry = sessions
                .get(&handle.id)
                .ok_or_else(|| format!("未知的 omp 会话: {}", handle.id))?;
            (
                entry.stdin.clone(),
                entry.sink.clone(),
                entry.pending.clone(),
                entry.ended.clone(),
            )
        };

        let approval = pending
            .lock()
            .map_err(|_| "omp 审批表锁被占用".to_string())?
            .remove(request_id)
            .unwrap_or(PendingApproval {
                method: String::new(),
                options: Vec::new(),
            });
        let frame = approval_response_frame(request_id, &decision, &approval);
        write_frame(&stdin, &frame)?;
        emit_event(
            &sink,
            &ended,
            AgentEvent::Status {
                state: AgentState::Running,
            },
        );
        Ok(())
    }

    fn cancel(&self, handle: &SessionHandle) -> Result<(), String> {
        let (stdin, child, sink, ended) = {
            let sessions = self
                .sessions
                .lock()
                .map_err(|_| "omp 会话表锁被占用".to_string())?;
            let entry = sessions
                .get(&handle.id)
                .ok_or_else(|| format!("未知的 omp 会话: {}", handle.id))?;
            (
                entry.stdin.clone(),
                entry.child.clone(),
                entry.sink.clone(),
                entry.ended.clone(),
            )
        };

        // abort 是尽力而为：管道可能已经关闭，2 秒后仍然强制杀进程。
        let _ = write_frame(&stdin, &json!({ "type": "abort" }));
        emit_event(
            &sink,
            &ended,
            AgentEvent::Ended {
                status: RunStatus::Cancelled,
            },
        );

        let _ = thread::spawn(move || {
            thread::sleep(KILL_DELAY);
            let Ok(mut child) = child.lock() else {
                return;
            };
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
                let _ = child.wait();
            }
        });

        Ok(())
    }

    /// 关闭会话：先关闭 stdin，再杀掉并回收子进程；不发送任何 UI 事件。
    fn close(&self, handle: &SessionHandle) -> Result<(), String> {
        let entry = self
            .sessions
            .lock()
            .map_err(|_| "omp 会话表锁被占用".to_string())?
            .remove(&handle.id);
        if let Some(entry) = entry {
            // 先置终态，保证读取线程在 EOF 时不补发 Error/Ended。
            entry.ended.store(true, Ordering::SeqCst);
            drop(entry.stdin);
            kill_child(&entry.child);
        }
        if let Ok(mut ids) = self.session_ids.lock() {
            ids.remove(&handle.id);
        }
        Ok(())
    }

    /// 返回 `get_state` 协商得到的 omp `sessionId`。
    fn session_id(&self, handle: &SessionHandle) -> Option<String> {
        self.session_ids.lock().ok()?.get(&handle.id).cloned()
    }
}

/// 握手阶段等待的回复类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HandshakeReply {
    Negotiated,
    State,
}

/// 等待一条握手响应；超时或失败时返回错误，避免 `start` 无限等待。
fn wait_handshake(
    rx: &mpsc::Receiver<Result<HandshakeReply, String>>,
    expected: HandshakeReply,
    stage: &str,
) -> Result<(), String> {
    match rx.recv_timeout(HANDSHAKE_TIMEOUT) {
        Ok(Ok(reply)) if reply == expected => Ok(()),
        Ok(Ok(_)) => Err(format!("omp 在 {stage} 阶段返回了意外的响应")),
        Ok(Err(message)) => Err(message),
        Err(RecvTimeoutError::Timeout) => Err(format!(
            "omp 在 {} 秒内没有响应 {stage}",
            HANDSHAKE_TIMEOUT.as_secs()
        )),
        Err(RecvTimeoutError::Disconnected) => {
            Err(format!("omp RPC 读取线程在 {stage} 握手期间退出"))
        }
    }
}

/// 握手失败：标记终态、杀掉子进程，并把错误交还给 `start`。
fn fail_start(
    child: &Mutex<Child>,
    ended: &AtomicBool,
    message: String,
) -> Result<SessionHandle, String> {
    ended.store(true, Ordering::SeqCst);
    kill_child(child);
    Err(message)
}

/// 读取 omp 的 stdout：先做 ready 握手，之后逐帧映射成 [`AgentEvent`]。
#[allow(clippy::too_many_arguments)]
fn read_stdout(
    stdout: ChildStdout,
    handle_id: String,
    session_ids: Arc<Mutex<HashMap<String, String>>>,
    pending: Arc<Mutex<HashMap<String, PendingApproval>>>,
    ended: Arc<AtomicBool>,
    child: Arc<Mutex<Child>>,
    sink: EventSink,
    ready_tx: mpsc::Sender<Result<(), String>>,
    handshake_tx: mpsc::Sender<Result<HandshakeReply, String>>,
) {
    let mut lines = BufReader::new(stdout).lines();

    loop {
        match lines.next() {
            Some(Ok(line)) if line.trim().is_empty() => continue,
            Some(Ok(line)) => {
                let ready = serde_json::from_str::<Value>(&line)
                    .ok()
                    .and_then(|frame| {
                        frame
                            .get("type")
                            .and_then(Value::as_str)
                            .map(|kind| kind == "ready")
                    })
                    .unwrap_or(false);
                if ready {
                    let _ = ready_tx.send(Ok(()));
                    break;
                }
                let message = format!("omp RPC 首帧不是 ready: {}", truncate(&line, SUMMARY_LIMIT));
                emit_event(
                    &sink,
                    &ended,
                    AgentEvent::Error {
                        message: message.clone(),
                        retryable: false,
                    },
                );
                let _ = ready_tx.send(Err(message));
                return;
            }
            Some(Err(error)) => {
                let message = format!("读取 omp RPC 输出失败: {error}");
                if !ended.load(Ordering::SeqCst) {
                    emit_event(
                        &sink,
                        &ended,
                        AgentEvent::Error {
                            message: message.clone(),
                            retryable: false,
                        },
                    );
                }
                let _ = ready_tx.send(Err(message));
                return;
            }
            None => {
                let message = "omp 在发送 ready 之前就退出了".to_string();
                if !ended.load(Ordering::SeqCst) {
                    emit_event(
                        &sink,
                        &ended,
                        AgentEvent::Error {
                            message: message.clone(),
                            retryable: false,
                        },
                    );
                }
                let _ = ready_tx.send(Err(message));
                return;
            }
        }
    }

    let mut chunks = RpcChunkBuffer::default();
    let mut run_state = RunState::default();

    for line in lines {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(mut frame) = serde_json::from_str::<Value>(&line) else {
            // 非 JSON 行直接忽略，避免噪声日志淹没事件流。
            continue;
        };

        // 协议 v2 下超过 1 MiB 的帧会拆成多个 `rpc_chunk`，重组后再走同一映射路径。
        if frame.get("type").and_then(Value::as_str) == Some("rpc_chunk") {
            match chunks.accept(&frame) {
                Ok(Some(assembled)) => frame = assembled,
                Ok(None) => continue,
                Err(message) => {
                    emit_event(
                        &sink,
                        &ended,
                        AgentEvent::Error {
                            message,
                            retryable: false,
                        },
                    );
                    continue;
                }
            }
        } else if chunks.interrupted() {
            // 序列未完成时插入其他帧，视为交错并丢弃半成品。
            emit_event(
                &sink,
                &ended,
                AgentEvent::Error {
                    message: "rpc_chunk 序列被非 chunk 帧打断".to_string(),
                    retryable: false,
                },
            );
        }

        let frame_type = frame.get("type").and_then(Value::as_str).unwrap_or("");
        let mut handshake_reply: Option<Result<HandshakeReply, String>> = None;

        if frame_type == "response" {
            // get_state 的响应：记录 omp 的 sessionId。
            if let Some(session_id) = extract_session_id(&frame)
                && let Ok(mut ids) = session_ids.lock()
            {
                ids.insert(handle_id.clone(), session_id);
            }
            let command = response_command(&frame);
            if command.eq_ignore_ascii_case("negotiate_protocol")
                || command.eq_ignore_ascii_case("get_state")
            {
                handshake_reply = Some(match frame.get("success").and_then(Value::as_bool) {
                    Some(false) => Err(error_message(&frame)),
                    _ if command.eq_ignore_ascii_case("negotiate_protocol") => {
                        Ok(HandshakeReply::Negotiated)
                    }
                    _ => Ok(HandshakeReply::State),
                });
            }
        }

        if frame_type == "extension_ui_request"
            && is_dialog_method(lookup_str(&frame, &["method"]).unwrap_or(""))
        {
            let request_id = frame_id(&frame);
            if !request_id.is_empty() {
                let method = lookup_str(&frame, &["method"]).unwrap_or("").to_string();
                let approval = PendingApproval {
                    method,
                    options: approval_options(&frame),
                };
                if let Ok(mut pending) = pending.lock() {
                    pending.insert(request_id, approval);
                }
            }
        }

        for event in map_frame_with_state(&frame, &mut run_state) {
            emit_event(&sink, &ended, event);
        }
        if let Some(reply) = handshake_reply {
            // 先发 UI 事件再唤醒 start：失败响应不能被 start 的终态竞速吞掉。
            let _ = handshake_tx.send(reply);
        }
    }

    if chunks.active.is_some() {
        emit_event(
            &sink,
            &ended,
            AgentEvent::Error {
                message: "omp 在 rpc_chunk 序列完成前退出".to_string(),
                retryable: false,
            },
        );
    }

    // 若 start 仍在等待握手响应，EOF/读错误必须让它失败而不是等到超时。
    let _ = handshake_tx.send(Err("omp RPC 在握手完成前退出".to_string()));

    // 进程退出且没有正常结束 → 视为失败。
    if !ended.load(Ordering::SeqCst) {
        emit_event(
            &sink,
            &ended,
            AgentEvent::Error {
                message: "omp 进程已退出".to_string(),
                retryable: false,
            },
        );
        emit_event(
            &sink,
            &ended,
            AgentEvent::Ended {
                status: RunStatus::Failed,
            },
        );
    }
    if let Ok(mut ids) = session_ids.lock() {
        ids.remove(&handle_id);
    }
    reap_child(&child);
}

/// 把 `provider/modelId` 形式的模型标识编码成 `set_model` 帧。
///
/// 不含 `/`，或任一侧为空的标识返回 `None`，调用方静默跳过。
fn set_model_frame(model: &str) -> Option<Value> {
    let (provider, model_id) = model.split_once('/')?;
    let provider = provider.trim();
    let model_id = model_id.trim();
    if provider.is_empty() || model_id.is_empty() {
        return None;
    }
    Some(json!({
        "id": "model",
        "type": "set_model",
        "provider": provider,
        "modelId": model_id,
    }))
}

/// 运行终态推导所需的跨帧状态。
#[derive(Default)]
struct RunState {
    /// `prompt_result` 已给出结果但 `sessionSettled=false`，等待 `session_settled` 终结。
    pending: Option<RunStatus>,
}

/// 把 omp 的一帧映射为 UI 事件；纯函数，测试无需启动子进程。
///
/// 仅测试使用：生产路径直接调用 [`map_frame_with_state`] 以跨帧维护终态。
#[cfg(test)]
pub fn map_frame(frame: &Value) -> Vec<AgentEvent> {
    map_frame_with_state(frame, &mut RunState::default())
}

fn map_frame_with_state(frame: &Value, state: &mut RunState) -> Vec<AgentEvent> {
    let Some(frame_type) = frame.get("type").and_then(Value::as_str) else {
        return Vec::new();
    };

    match frame_type {
        "agent_start" => vec![AgentEvent::Status {
            state: AgentState::Running,
        }],
        "message_update" => map_message_update(frame),
        "tool_execution_start" => map_tool_start(frame),
        "tool_execution_end" => map_tool_end(frame),
        "extension_ui_request" => map_ui_request(frame),
        "agent_end" => map_agent_end(frame),
        "prompt_result" => map_prompt_result(frame, state),
        "session_settled" => map_session_settled(state),
        "response" => map_response(frame),
        "error" | "notice" => map_error(frame),
        _ => Vec::new(),
    }
}

/// 编码一条 `extension_ui_response` 帧。
///
/// 回复必须与请求方法匹配：
/// - 拒绝/取消 → `cancelled: true`，与请求方法无关；
/// - `confirm` → `confirmed` 布尔值，允许为 `true`，选择 `"true"` / `"yes"` /
///   `"allow"` 为 `true`，其他选择为 `false`；
/// - `select` / `input` / `editor` → `value`，允许时回填第一个选项，
///   无选项时回填 `"allow"`；
/// - `ask` 及其他方法不被支持，一律按取消处理。
pub fn approval_response_frame(
    request_id: &str,
    decision: &Decision,
    pending: &PendingApproval,
) -> Value {
    let mut frame = json!({
        "type": "extension_ui_response",
        "id": request_id,
    });
    let method = pending.method.as_str();
    match decision {
        Decision::Deny | Decision::Cancel => {
            frame["cancelled"] = Value::Bool(true);
        }
        Decision::Allow if method.eq_ignore_ascii_case("confirm") => {
            frame["confirmed"] = Value::Bool(true);
        }
        Decision::Select(value) if method.eq_ignore_ascii_case("confirm") => {
            frame["confirmed"] = Value::Bool(is_confirm_value(value));
        }
        Decision::Allow if is_answer_method(method) => {
            let value = pending
                .options
                .first()
                .cloned()
                .unwrap_or_else(|| "allow".to_string());
            frame["value"] = Value::String(value);
        }
        Decision::Select(value) if is_answer_method(method) => {
            frame["value"] = Value::String(value.clone());
        }
        _ => {
            frame["cancelled"] = Value::Bool(true);
        }
    }
    frame
}

/// `confirm` 请求的肯定答复文案；其余文本都编码为 `confirmed: false`。
fn is_confirm_value(value: &str) -> bool {
    value.eq_ignore_ascii_case("true")
        || value.eq_ignore_ascii_case("yes")
        || value.eq_ignore_ascii_case("allow")
}

/// 以 `value` 回填答复的请求方法（`confirm` 用 `confirmed`，`ask` 不支持）。
fn is_answer_method(method: &str) -> bool {
    ["select", "input", "editor"]
        .iter()
        .any(|answer| method.eq_ignore_ascii_case(answer))
}

fn map_message_update(frame: &Value) -> Vec<AgentEvent> {
    let payload = frame
        .get("assistantMessageEvent")
        .or_else(|| frame.pointer("/data/assistantMessageEvent"))
        .or_else(|| frame.pointer("/message/assistantMessageEvent"));
    let Some(payload) = payload else {
        return Vec::new();
    };
    if payload.get("type").and_then(Value::as_str) != Some("text_delta") {
        return Vec::new();
    }
    let text = payload
        .get("delta")
        .or_else(|| payload.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("");
    vec![AgentEvent::TextDelta {
        text: text.to_string(),
    }]
}

fn map_tool_start(frame: &Value) -> Vec<AgentEvent> {
    vec![AgentEvent::ToolStart {
        name: tool_name(frame),
        summary: tool_summary(frame),
    }]
}

fn map_tool_end(frame: &Value) -> Vec<AgentEvent> {
    let is_error = frame
        .get("isError")
        .and_then(Value::as_bool)
        .or_else(|| frame.get("is_error").and_then(Value::as_bool))
        .or_else(|| frame.pointer("/data/isError").and_then(Value::as_bool))
        .or_else(|| frame.get("error").map(|error| !error.is_null()))
        .unwrap_or(false);
    vec![AgentEvent::ToolEnd {
        name: tool_name(frame),
        ok: !is_error,
    }]
}

/// 只有这些 `extension_ui_request` 方法需要用户决策并回写 `extension_ui_response`。
///
/// `setWidget` / `setStatus` / `notify` / `setTitle` / `set_editor_text` /
/// `open_url` / `cancel` 等展示型方法是即发即忘的，不需要响应，因此不得当作审批。
fn is_dialog_method(method: &str) -> bool {
    ["select", "confirm", "input", "editor", "ask"]
        .iter()
        .any(|dialog| method.eq_ignore_ascii_case(dialog))
}

fn map_ui_request(frame: &Value) -> Vec<AgentEvent> {
    let method = lookup_str(frame, &["method"]).unwrap_or("");
    if !is_dialog_method(method) {
        return Vec::new();
    }
    let request_id = frame_id(frame);
    let prompt = lookup_str(frame, &["prompt", "message", "title", "text"]).unwrap_or("");
    let title = if prompt.is_empty() {
        method.to_string()
    } else {
        format!("{method}: {prompt}")
    };
    let risk = if method.eq_ignore_ascii_case("confirm") && mentions_danger(prompt) {
        RiskLevel::High
    } else {
        RiskLevel::Medium
    };
    vec![
        AgentEvent::ApprovalRequest {
            request_id,
            title,
            detail: truncate(&frame.to_string(), DETAIL_LIMIT),
            risk,
            options: approval_options(frame),
        },
        AgentEvent::Status {
            state: AgentState::WaitingApproval,
        },
    ]
}

/// 从帧里的状态/错误字段推导运行终态。
fn run_status(frame: &Value) -> RunStatus {
    let status = lookup_str(frame, &["status", "stopReason", "stop_reason"])
        .map(|value| value.to_ascii_lowercase());
    // `agent_end` 本身不带 status，但 `messages` 里最后一条 assistant 消息的
    // `stopReason` 与 OMP 判定 prompt_result 的方式一致。
    let message_status = frame
        .get("messages")
        .or_else(|| frame.pointer("/data/messages"))
        .and_then(Value::as_array)
        .and_then(|messages| {
            messages
                .iter()
                .rev()
                .find(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        })
        .and_then(|message| {
            message
                .get("stopReason")
                .or_else(|| message.get("stop_reason"))
                .and_then(Value::as_str)
        })
        .map(|value| value.to_ascii_lowercase());

    let aborted = matches!(
        status.as_deref(),
        Some("aborted" | "abort" | "cancelled" | "canceled")
    ) || matches!(
        message_status.as_deref(),
        Some("aborted" | "abort" | "cancelled" | "canceled")
    ) || lookup_bool(frame, "aborted")
        || lookup_bool(frame, "cancelled");
    let failed = matches!(status.as_deref(), Some("error" | "failed" | "failure"))
        || matches!(
            message_status.as_deref(),
            Some("error" | "failed" | "failure")
        )
        || frame
            .get("error")
            .map(|error| !error.is_null())
            .unwrap_or(false)
        || frame
            .pointer("/data/error")
            .map(|error| !error.is_null())
            .unwrap_or(false);

    if aborted {
        RunStatus::Cancelled
    } else if failed {
        RunStatus::Failed
    } else {
        RunStatus::Completed
    }
}

/// `agent_end` 只有在 `yielded != false` 且 `isTerminal != false` 时才算完成；
/// 否则交给后续的 `prompt_result` / `session_settled` 终结。
fn map_agent_end(frame: &Value) -> Vec<AgentEvent> {
    let yielded = lookup_opt_bool(frame, "yielded").unwrap_or(true);
    let is_terminal = lookup_opt_bool(frame, "isTerminal")
        .or_else(|| lookup_opt_bool(frame, "is_terminal"))
        .unwrap_or(true);
    // 兼容旧字段：`willContinue=true` 同样表示后面还有后续工作。
    let will_continue = lookup_opt_bool(frame, "willContinue").unwrap_or(false);

    let mut events = Vec::with_capacity(2);
    if let Some(usage) = usage_event(frame) {
        events.push(usage);
    }
    if yielded && is_terminal && !will_continue {
        events.push(AgentEvent::Ended {
            status: run_status(frame),
        });
    }
    events
}

/// `prompt_result` 给出本轮状态；`sessionSettled=false` 时先记下状态，
/// 等 `session_settled` 再发终态。
fn map_prompt_result(frame: &Value, state: &mut RunState) -> Vec<AgentEvent> {
    let status = run_status(frame);
    let settled = lookup_opt_bool(frame, "sessionSettled")
        .or_else(|| lookup_opt_bool(frame, "session_settled"))
        .unwrap_or(true);

    let mut events = Vec::with_capacity(2);
    if let Some(usage) = usage_event(frame) {
        events.push(usage);
    }
    if settled {
        state.pending = None;
        events.push(AgentEvent::Ended { status });
    } else {
        state.pending = Some(status);
    }
    events
}

/// `session_settled` 是会话彻底安静后的终止信号：优先使用挂起的 prompt 状态。
fn map_session_settled(state: &mut RunState) -> Vec<AgentEvent> {
    let status = state.pending.take().unwrap_or(RunStatus::Completed);
    vec![AgentEvent::Ended { status }]
}

/// 命令响应：`success:false` 必须报错；`prompt` 失败同时结束为 Failed。
fn map_response(frame: &Value) -> Vec<AgentEvent> {
    if frame.get("success").and_then(Value::as_bool) != Some(false) {
        return Vec::new();
    }
    let mut events = map_error(frame);
    if response_command(frame).eq_ignore_ascii_case("prompt") {
        events.push(AgentEvent::Ended {
            status: RunStatus::Failed,
        });
    }
    events
}

/// 命令名：服务端在 `response.command` 中回显；旧实现可能只有 `id`。
fn response_command(frame: &Value) -> &str {
    frame
        .get("command")
        .and_then(Value::as_str)
        .or_else(|| frame.get("id").and_then(Value::as_str))
        .unwrap_or("")
}

fn map_error(frame: &Value) -> Vec<AgentEvent> {
    let message = error_message(frame);
    let code = lookup_str(frame, &["code", "reason"]).unwrap_or("");
    let retryable = lookup_bool(frame, "retryable")
        || code.eq_ignore_ascii_case("rate_limit")
        || code.eq_ignore_ascii_case("overloaded");
    vec![AgentEvent::Error { message, retryable }]
}

/// 从错误帧/失败响应里提取可读消息。
fn error_message(frame: &Value) -> String {
    lookup_str(frame, &["message", "error", "detail", "text"])
        .map(str::to_string)
        .unwrap_or_else(|| truncate(&frame.to_string(), DETAIL_LIMIT))
}

fn usage_event(frame: &Value) -> Option<AgentEvent> {
    let data = frame.get("data");
    let usage = frame
        .get("usage")
        .filter(|value| value.is_object())
        .or_else(|| {
            data.and_then(|data| data.get("usage"))
                .filter(|value| value.is_object())
        });
    let direct_tokens = frame.get("tokens").and_then(Value::as_u64).or_else(|| {
        data.and_then(|data| data.get("tokens"))
            .and_then(Value::as_u64)
    });
    let direct_cost = frame
        .get("cost_usd")
        .and_then(Value::as_f64)
        .or_else(|| frame.get("totalCost").and_then(Value::as_f64))
        .or_else(|| {
            data.and_then(|data| data.get("cost_usd"))
                .and_then(Value::as_f64)
        })
        .or_else(|| {
            data.and_then(|data| data.get("totalCost"))
                .and_then(Value::as_f64)
        });

    if usage.is_none() && direct_tokens.is_none() && direct_cost.is_none() {
        return None;
    }

    Some(AgentEvent::Usage {
        tokens: usage.and_then(token_count).or(direct_tokens),
        cost_usd: usage.and_then(cost_value).or(direct_cost),
    })
}

fn token_count(usage: &Value) -> Option<u64> {
    for key in ["totalTokens", "total_tokens", "total"] {
        if let Some(total) = usage.get(key).and_then(Value::as_u64) {
            return Some(total);
        }
    }
    let input = usage
        .get("input")
        .and_then(Value::as_u64)
        .or_else(|| usage.get("inputTokens").and_then(Value::as_u64))
        .or_else(|| usage.get("input_tokens").and_then(Value::as_u64));
    let output = usage
        .get("output")
        .and_then(Value::as_u64)
        .or_else(|| usage.get("outputTokens").and_then(Value::as_u64))
        .or_else(|| usage.get("output_tokens").and_then(Value::as_u64));
    match (input, output) {
        (Some(input), Some(output)) => Some(input.saturating_add(output)),
        (input, output) => input.or(output),
    }
}

fn cost_value(usage: &Value) -> Option<f64> {
    usage
        .get("cost_usd")
        .and_then(Value::as_f64)
        .or_else(|| usage.get("cost").and_then(Value::as_f64))
        .or_else(|| usage.pointer("/cost/total").and_then(Value::as_f64))
}

fn approval_options(frame: &Value) -> Vec<String> {
    let method = lookup_str(frame, &["method"]).unwrap_or("");
    if !method.eq_ignore_ascii_case("select") {
        return vec!["allow".to_string(), "deny".to_string()];
    }
    let options = frame.get("options").and_then(Value::as_array).or_else(|| {
        frame
            .get("data")
            .and_then(|data| data.get("options"))
            .and_then(Value::as_array)
    });
    options
        .map(|items| items.iter().filter_map(option_label).collect())
        .unwrap_or_default()
}

fn option_label(item: &Value) -> Option<String> {
    match item {
        Value::String(text) => Some(text.clone()),
        Value::Object(map) => ["label", "value", "title"]
            .iter()
            .find_map(|key| map.get(*key).and_then(Value::as_str))
            .map(str::to_string),
        _ => None,
    }
}

fn tool_name(frame: &Value) -> String {
    lookup_str(frame, &["toolName", "tool_name", "name", "tool"])
        .map(str::to_string)
        .unwrap_or_else(|| "tool".to_string())
}

fn tool_summary(frame: &Value) -> String {
    if let Some(summary) = lookup_str(frame, &["summary", "title", "description"])
        && !summary.is_empty()
    {
        return truncate(summary, SUMMARY_LIMIT);
    }
    if let Some(args) = frame
        .get("args")
        .or_else(|| frame.get("arguments"))
        .or_else(|| frame.pointer("/data/args"))
    {
        return truncate(&args.to_string(), SUMMARY_LIMIT);
    }
    String::new()
}

fn mentions_danger(text: &str) -> bool {
    let lowered = text.to_lowercase();
    lowered.contains("delete") || lowered.contains("rm") || lowered.contains("secret")
}

fn lookup_str<'a>(frame: &'a Value, keys: &[&str]) -> Option<&'a str> {
    if let Some(found) = keys
        .iter()
        .find_map(|key| frame.get(*key).and_then(Value::as_str))
    {
        return Some(found);
    }
    let data = frame.get("data")?;
    keys.iter()
        .find_map(|key| data.get(*key).and_then(Value::as_str))
}

fn lookup_bool(frame: &Value, key: &str) -> bool {
    lookup_opt_bool(frame, key).unwrap_or(false)
}

fn lookup_opt_bool(frame: &Value, key: &str) -> Option<bool> {
    frame.get(key).and_then(Value::as_bool).or_else(|| {
        frame
            .get("data")
            .and_then(|data| data.get(key))
            .and_then(Value::as_bool)
    })
}

fn frame_id(frame: &Value) -> String {
    let id = frame
        .get("id")
        .or_else(|| frame.get("data").and_then(|data| data.get("id")));
    match id {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

fn extract_session_id(frame: &Value) -> Option<String> {
    frame
        .pointer("/data/sessionId")
        .or_else(|| frame.pointer("/data/session_id"))
        .or_else(|| frame.get("sessionId"))
        .or_else(|| frame.get("session_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// 重组协议 v2 的 `rpc_chunk` 序列；同一时间只允许一个未完成序列。
#[derive(Default)]
struct RpcChunkBuffer {
    active: Option<ChunkSequence>,
}

struct ChunkSequence {
    chunk_id: String,
    count: u64,
    byte_length: usize,
    next_index: u64,
    data: Vec<u8>,
}

impl RpcChunkBuffer {
    /// 喂入一帧：`Ok(None)` 表示序列未完成，`Ok(Some(frame))` 表示重组完成。
    ///
    /// 交错、乱序、重复或长度不符的序列一律拒绝，并清空缓冲以便后续序列恢复。
    fn accept(&mut self, frame: &Value) -> Result<Option<Value>, String> {
        let outcome = self.push(frame);
        if outcome.is_err() {
            self.active = None;
        }
        outcome
    }

    fn push(&mut self, frame: &Value) -> Result<Option<Value>, String> {
        let chunk_id = frame
            .get("chunkId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.chars().count() <= MAX_CHUNK_ID_CHARS)
            .ok_or_else(|| "rpc_chunk 的 chunkId 非法".to_string())?
            .to_string();
        let index = frame
            .get("index")
            .and_then(Value::as_u64)
            .ok_or_else(|| "rpc_chunk 的 index 非法".to_string())?;
        let count = frame
            .get("count")
            .and_then(Value::as_u64)
            .ok_or_else(|| "rpc_chunk 的 count 非法".to_string())?;
        let byte_length = frame
            .get("byteLength")
            .and_then(Value::as_u64)
            .ok_or_else(|| "rpc_chunk 的 byteLength 非法".to_string())?;
        let data = frame
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| "rpc_chunk 缺少 data".to_string())?;

        if !(2..=MAX_CHUNK_COUNT).contains(&count) || index >= count {
            return self.reject("rpc_chunk 的 count/index 非法");
        }
        if byte_length == 0 || byte_length > MAX_FRAME_BYTES as u64 {
            return self.reject("rpc_chunk 的 byteLength 越界");
        }
        let byte_length = byte_length as usize;
        let decoded = decode_base64(data)
            .map_err(|error| format!("rpc_chunk 的 data 不是规范 base64: {error}"))?;
        if decoded.is_empty() || decoded.len() > MAX_CHUNK_BYTES {
            return self.reject("rpc_chunk 的 data 长度非法");
        }

        if self.active.is_some() {
            let active = self.active.as_mut().expect("rpc_chunk 序列应处于活动状态");
            if active.chunk_id != chunk_id
                || active.count != count
                || active.byte_length != byte_length
            {
                return self.reject("rpc_chunk 序列交错或元数据不一致");
            }
            if active.next_index != index {
                return self.reject("rpc_chunk 序列乱序、重复或缺失");
            }
            if active.data.len().saturating_add(decoded.len()) > active.byte_length {
                return self.reject("rpc_chunk 序列长度超过 byteLength");
            }
            active.data.extend_from_slice(&decoded);
            active.next_index += 1;
            if active.next_index < active.count {
                return Ok(None);
            }
        } else {
            if index != 0 {
                return self.reject("rpc_chunk 序列必须从 index 0 开始");
            }
            if decoded.len() > byte_length {
                return self.reject("rpc_chunk 序列长度超过 byteLength");
            }
            // count >= 2，首片之后一定还未完成。
            let mut data = Vec::with_capacity(byte_length);
            data.extend_from_slice(&decoded);
            self.active = Some(ChunkSequence {
                chunk_id,
                count,
                byte_length,
                next_index: 1,
                data,
            });
            return Ok(None);
        }

        let sequence = self.active.take().expect("rpc_chunk 序列应处于活动状态");
        if sequence.data.len() != sequence.byte_length {
            return Err(format!(
                "rpc_chunk 重组长度 {} 与 byteLength {} 不符",
                sequence.data.len(),
                sequence.byte_length
            ));
        }
        let text = String::from_utf8(sequence.data)
            .map_err(|_| "rpc_chunk 重组结果不是合法 UTF-8".to_string())?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|error| format!("rpc_chunk 重组结果不是合法 JSON: {error}"))?;
        if !value.is_object() {
            return Err("rpc_chunk 重组结果不是 JSON 对象".to_string());
        }
        Ok(Some(value))
    }

    fn reject(&mut self, message: &str) -> Result<Option<Value>, String> {
        self.active = None;
        Err(message.to_string())
    }

    /// 序列被其他帧打断时清空半成品并返回是否有活动序列。
    fn interrupted(&mut self) -> bool {
        self.active.take().is_some()
    }
}

/// 严格的标准 base64 解码：只接受带规范填充、尾位为零的输入。
fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    let bytes = input.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err("长度不是 4 的倍数".to_string());
    }
    let quad_count = bytes.len() / 4;
    let mut output = Vec::with_capacity(quad_count * 3);
    for (quad_index, quad) in bytes.as_chunks::<4>().0.iter().enumerate() {
        let last_quad = quad_index + 1 == quad_count;
        let mut values = [0u8; 4];
        let mut padding = 0usize;
        for (position, &byte) in quad.iter().enumerate() {
            if byte == b'=' {
                if position < 2 {
                    return Err("填充符位置非法".to_string());
                }
                padding += 1;
                continue;
            }
            if padding > 0 {
                return Err("填充符之后仍有数据".to_string());
            }
            values[position] = base64_value(byte)?;
        }
        if padding > 0 && !last_quad {
            return Err("填充符只能出现在最后一个四元组".to_string());
        }
        match padding {
            0 => {
                output.push((values[0] << 2) | (values[1] >> 4));
                output.push((values[1] << 4) | (values[2] >> 2));
                output.push((values[2] << 6) | values[3]);
            }
            1 => {
                if values[2] & 0b11 != 0 {
                    return Err("非规范的尾位".to_string());
                }
                output.push((values[0] << 2) | (values[1] >> 4));
                output.push((values[1] << 4) | (values[2] >> 2));
            }
            2 => {
                if values[1] & 0b1111 != 0 {
                    return Err("非规范的尾位".to_string());
                }
                output.push((values[0] << 2) | (values[1] >> 4));
            }
            _ => unreachable!("一个四元组最多两个填充符"),
        }
    }
    Ok(output)
}

fn base64_value(byte: u8) -> Result<u8, String> {
    match byte {
        b'A'..=b'Z' => Ok(byte - b'A'),
        b'a'..=b'z' => Ok(byte - b'a' + 26),
        b'0'..=b'9' => Ok(byte - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err(format!("非法字符 {:?}", byte as char)),
    }
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(limit).collect();
    truncated.push('…');
    truncated
}

fn write_frame(stdin: &Mutex<ChildStdin>, frame: &Value) -> Result<(), String> {
    let mut guard = stdin.lock().map_err(|_| "omp stdin 锁被占用".to_string())?;
    writeln!(guard, "{frame}").map_err(|error| format!("写入 omp stdin 失败: {error}"))?;
    guard
        .flush()
        .map_err(|error| format!("刷新 omp stdin 失败: {error}"))
}

fn emit_event(sink: &EventSink, ended: &AtomicBool, event: AgentEvent) {
    let is_ended = matches!(&event, AgentEvent::Ended { .. });
    if is_ended {
        if ended.swap(true, Ordering::SeqCst) {
            return;
        }
    } else if ended.load(Ordering::SeqCst) {
        // 终态之后不再向 UI 泄漏残余事件。
        return;
    }
    let callback: &(dyn Fn(AgentEvent) + Send + Sync) = &**sink;
    callback(event);
}

fn kill_child(child: &Mutex<Child>) {
    // 锁中毒时也要尽量完成清理。
    let mut guard = child
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _ = guard.kill();
    let _ = guard.wait();
}

/// 进程自然退出后回收，避免留下僵尸进程；最多等待约 200ms。
fn reap_child(child: &Mutex<Child>) {
    for _ in 0..5 {
        let reaped = match child.lock() {
            Ok(mut child) => matches!(child.try_wait(), Ok(Some(_))),
            Err(_) => true,
        };
        if reaped {
            return;
        }
        thread::sleep(Duration::from_millis(40));
    }
}

/// 在独立线程中执行 `omp --version`，主线程最多等待 5 秒。
///
/// 仅测试使用：生产代码不调用 [`AgentAdapter::detect`]。
#[cfg(test)]
fn probe_version() -> Result<Option<String>, String> {
    let (tx, rx) = mpsc::channel::<std::io::Result<std::process::Output>>();
    let _ = thread::spawn(move || {
        let _ = tx.send(
            Command::new("omp")
                .arg("--version")
                .stdin(Stdio::null())
                .output(),
        );
    });

    let output = match rx.recv_timeout(VERSION_TIMEOUT) {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => return Err(format!("未找到 omp 可执行文件: {error}")),
        Err(_) => {
            return Err(format!(
                "omp --version 超时（{} 秒）",
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
            format!("omp --version 退出码 {:?}", output.status.code())
        } else {
            format!("omp --version 失败: {detail}")
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
    use serde_json::json;

    fn assert_ended(events: &[AgentEvent], expected: RunStatus) {
        match events.last() {
            Some(AgentEvent::Ended { status }) => assert_eq!(*status, expected),
            other => panic!("expected Ended, got {other:?}"),
        }
    }

    fn chunk_frame(id: &str, index: u64, count: u64, byte_length: usize, data: &str) -> Value {
        json!({
            "type": "rpc_chunk",
            "chunkId": id,
            "index": index,
            "count": count,
            "byteLength": byte_length,
            "data": data,
        })
    }

    /// 测试用标准 base64 编码（生产代码只实现严格解码）。
    fn encode_base64(input: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut output = String::new();
        for chunk in input.chunks(3) {
            let first = chunk[0] as u32;
            let second = chunk.get(1).copied().unwrap_or(0) as u32;
            let third = chunk.get(2).copied().unwrap_or(0) as u32;
            let bits = (first << 16) | (second << 8) | third;
            output.push(TABLE[((bits >> 18) & 63) as usize] as char);
            output.push(TABLE[((bits >> 12) & 63) as usize] as char);
            output.push(if chunk.len() > 1 {
                TABLE[((bits >> 6) & 63) as usize] as char
            } else {
                '='
            });
            output.push(if chunk.len() > 2 {
                TABLE[(bits & 63) as usize] as char
            } else {
                '='
            });
        }
        output
    }

    #[test]
    fn reassembles_two_chunk_frame() {
        let payload = json!({
            "type": "notice",
            "message": "重组完成",
            "details": "x".repeat(40),
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let split = bytes.len() / 2;
        let (first, second) = bytes.split_at(split);
        let mut buffer = RpcChunkBuffer::default();

        let pending = buffer
            .accept(&chunk_frame(
                "rpc-1",
                0,
                2,
                bytes.len(),
                &encode_base64(first),
            ))
            .expect("first chunk");
        assert!(pending.is_none(), "first chunk must not complete the frame");

        let assembled = buffer
            .accept(&chunk_frame(
                "rpc-1",
                1,
                2,
                bytes.len(),
                &encode_base64(second),
            ))
            .expect("second chunk")
            .expect("assembled frame");
        assert_eq!(assembled, payload);
    }

    #[test]
    fn rejects_out_of_order_chunk() {
        let mut buffer = RpcChunkBuffer::default();
        let error = buffer
            .accept(&chunk_frame("rpc-2", 1, 2, 6, &encode_base64(b"second")))
            .unwrap_err();
        assert!(error.contains("index 0"), "unexpected error: {error}");
        assert!(buffer.active.is_none(), "rejection must reset the buffer");
    }

    #[test]
    fn rejects_interleaved_chunk_sequence() {
        let mut buffer = RpcChunkBuffer::default();
        buffer
            .accept(&chunk_frame("rpc-a", 0, 2, 4, &encode_base64(b"aa")))
            .expect("first chunk");
        let error = buffer
            .accept(&chunk_frame("rpc-b", 0, 2, 4, &encode_base64(b"bb")))
            .unwrap_err();
        assert!(error.contains("交错"), "unexpected error: {error}");
        assert!(buffer.active.is_none(), "rejection must reset the buffer");
    }

    #[test]
    fn rejects_mismatched_chunk_length() {
        let mut buffer = RpcChunkBuffer::default();
        buffer
            .accept(&chunk_frame("rpc-c", 0, 2, 6, &encode_base64(b"abc")))
            .expect("first chunk");
        let error = buffer
            .accept(&chunk_frame("rpc-c", 1, 2, 6, &encode_base64(b"de")))
            .unwrap_err();
        assert!(error.contains("byteLength"), "unexpected error: {error}");
    }

    #[test]
    fn interrupted_sequence_is_dropped() {
        let mut buffer = RpcChunkBuffer::default();
        buffer
            .accept(&chunk_frame("rpc-d", 0, 2, 6, &encode_base64(b"abc")))
            .expect("first chunk");
        assert!(buffer.interrupted(), "an incomplete sequence is active");
        assert!(buffer.active.is_none());
    }

    #[test]
    fn decodes_strict_base64() {
        let cases: [(&str, &[u8]); 6] = [
            ("", b""),
            ("aGVsbG8=", b"hello"),
            ("aGVsbG8h", b"hello!"),
            ("YQ==", b"a"),
            ("YWI=", b"ab"),
            ("/w==", b"\xff"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                decode_base64(input).unwrap().as_slice(),
                expected,
                "{input}"
            );
        }
    }

    #[test]
    fn rejects_invalid_base64() {
        for input in [
            "aGVsbG8",
            "aGVsbG8==",
            "ab=c",
            "====",
            "A===",
            "AA=A",
            "AA==AA==",
            "YR==",
            "ABC=",
            "A?==",
            "AAA",
            "-_-_",
            "A A=",
        ] {
            assert!(decode_base64(input).is_err(), "{input} should be rejected");
        }
    }

    #[test]
    fn prompt_response_failure_emits_error_and_failed_end() {
        let events = map_frame(&json!({
            "type": "response",
            "id": "prompt-1",
            "command": "prompt",
            "success": false,
            "error": "model unavailable"
        }));
        match &events[0] {
            AgentEvent::Error { message, retryable } => {
                assert_eq!(message, "model unavailable");
                assert!(!retryable);
            }
            other => panic!("expected Error, got {other:?}"),
        }
        assert_ended(&events, RunStatus::Failed);
    }

    #[test]
    fn handshake_response_failure_emits_error_without_ending() {
        let events = map_frame(&json!({
            "type": "response",
            "id": "proto",
            "command": "negotiate_protocol",
            "success": false,
            "error": "Unsupported RPC protocol version: 2"
        }));
        assert!(matches!(events.as_slice(), [AgentEvent::Error { .. }]));
    }

    #[test]
    fn successful_response_emits_nothing() {
        assert!(
            map_frame(&json!({
                "type": "response",
                "id": "prompt",
                "command": "prompt",
                "success": true
            }))
            .is_empty()
        );
    }

    #[test]
    fn non_terminal_agent_end_waits_for_settlement() {
        for frame in [
            json!({ "type": "agent_end", "yielded": false }),
            json!({ "type": "agent_end", "isTerminal": false }),
            json!({ "type": "agent_end", "yielded": true, "isTerminal": false }),
            json!({ "type": "agent_end", "yielded": false, "isTerminal": true }),
            json!({ "type": "agent_end", "willContinue": true }),
            json!({
                "type": "agent_end",
                "isTerminal": false,
                "messages": [{ "role": "assistant", "stopReason": "error" }]
            }),
        ] {
            let events = map_frame(&frame);
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, AgentEvent::Ended { .. })),
                "{frame} should not end the run: {events:?}"
            );
        }
    }

    #[test]
    fn session_settled_uses_pending_prompt_status() {
        let mut state = RunState::default();
        let events = map_frame_with_state(
            &json!({
                "type": "prompt_result",
                "status": "aborted",
                "sessionSettled": false
            }),
            &mut state,
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentEvent::Ended { .. })),
            "unsettled prompt_result must not end the run: {events:?}"
        );

        let events = map_frame_with_state(&json!({ "type": "session_settled" }), &mut state);
        assert_ended(&events, RunStatus::Cancelled);
    }

    #[test]
    fn session_settled_ends_completed_without_pending_prompt_result() {
        let events = map_frame(&json!({ "type": "session_settled" }));
        assert_ended(&events, RunStatus::Completed);
    }

    #[test]
    fn settled_prompt_error_maps_to_failed() {
        let events = map_frame(&json!({
            "type": "prompt_result",
            "status": "error",
            "sessionSettled": true,
            "error": { "message": "boom" }
        }));
        assert_ended(&events, RunStatus::Failed);
    }

    #[test]
    fn terminal_agent_end_uses_last_assistant_stop_reason() {
        let events = map_frame(&json!({
            "type": "agent_end",
            "isTerminal": true,
            "yielded": true,
            "messages": [
                { "role": "assistant", "stopReason": "error", "errorMessage": "boom" }
            ]
        }));
        assert_ended(&events, RunStatus::Failed);

        let events = map_frame(&json!({
            "type": "agent_end",
            "isTerminal": true,
            "yielded": true,
            "messages": [
                { "role": "assistant", "stopReason": "aborted" }
            ]
        }));
        assert_ended(&events, RunStatus::Cancelled);
    }

    #[test]
    fn emit_event_dedupes_post_terminal_events() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let sink: EventSink = Arc::new(move |event| {
            captured.lock().unwrap().push(event);
        });
        let ended = AtomicBool::new(false);

        emit_event(
            &sink,
            &ended,
            AgentEvent::Ended {
                status: RunStatus::Completed,
            },
        );
        emit_event(
            &sink,
            &ended,
            AgentEvent::TextDelta {
                text: "late".to_string(),
            },
        );
        emit_event(
            &sink,
            &ended,
            AgentEvent::Ended {
                status: RunStatus::Failed,
            },
        );

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1, "only the first Ended should be emitted");
        assert_ended(&events, RunStatus::Completed);
    }

    #[cfg(unix)]
    #[test]
    fn close_removes_session_and_reaps_child() {
        let mut child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn cat");
        let stdin = child.stdin.take().expect("cat stdin");
        let child = Arc::new(Mutex::new(child));
        let child_probe = child.clone();
        let ended = Arc::new(AtomicBool::new(false));
        let sink: EventSink = Arc::new(|_| {});
        let adapter = OmpAdapter::new();
        let handle = SessionHandle {
            id: "omp-close".to_string(),
        };
        adapter.sessions.lock().unwrap().insert(
            handle.id.clone(),
            SessionEntry {
                stdin: Arc::new(Mutex::new(stdin)),
                child,
                sink,
                pending: Arc::new(Mutex::new(HashMap::new())),
                ended: ended.clone(),
            },
        );
        adapter
            .session_ids
            .lock()
            .unwrap()
            .insert(handle.id.clone(), "session-1".to_string());

        AgentAdapter::close(&adapter, &handle).unwrap();

        assert!(!adapter.sessions.lock().unwrap().contains_key(&handle.id));
        assert!(
            adapter
                .session_ids
                .lock()
                .unwrap()
                .get(&handle.id)
                .is_none(),
            "session id should be removed"
        );
        assert!(ended.load(Ordering::SeqCst));
        let mut guard = child_probe.lock().unwrap();
        assert!(
            matches!(guard.try_wait(), Ok(Some(_))),
            "child should be killed and reaped"
        );
    }

    #[test]
    fn session_id_returns_negotiated_id() {
        let adapter = OmpAdapter::new();
        let handle = SessionHandle {
            id: "omp-session".to_string(),
        };
        adapter
            .session_ids
            .lock()
            .unwrap()
            .insert(handle.id.clone(), "session-42".to_string());
        assert_eq!(
            AgentAdapter::session_id(&adapter, &handle).as_deref(),
            Some("session-42")
        );

        adapter.session_ids.lock().unwrap().remove(&handle.id);
        assert_eq!(AgentAdapter::session_id(&adapter, &handle), None);
    }

    #[test]
    fn maps_text_delta() {
        let events = map_frame(&json!({
            "type": "message_update",
            "assistantMessageEvent": { "type": "text_delta", "delta": "你好" }
        }));
        assert_eq!(events.len(), 1);
        match &events[0] {
            AgentEvent::TextDelta { text } => assert_eq!(text, "你好"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn ignores_non_text_message_updates() {
        let events = map_frame(&json!({
            "type": "message_update",
            "assistantMessageEvent": { "type": "thinking_delta", "delta": "..." }
        }));
        assert!(events.is_empty());
    }

    #[test]
    fn maps_agent_end_completed() {
        let events = map_frame(&json!({
            "type": "agent_end",
            "data": { "status": "completed" }
        }));
        assert_ended(&events, RunStatus::Completed);
    }

    #[test]
    fn maps_prompt_result_aborted_to_cancelled() {
        let events = map_frame(&json!({
            "type": "prompt_result",
            "data": { "status": "aborted" }
        }));
        assert_eq!(events.len(), 1);
        assert_ended(&events, RunStatus::Cancelled);
    }

    #[test]
    fn maps_agent_end_error_to_failed() {
        let events = map_frame(&json!({
            "type": "agent_end",
            "data": { "status": "error", "error": "boom" }
        }));
        assert_ended(&events, RunStatus::Failed);
    }

    #[test]
    fn maps_usage_before_ended() {
        let events = map_frame(&json!({
            "type": "agent_end",
            "status": "completed",
            "usage": { "input": 10, "output": 5, "cost": { "total": 0.25 } }
        }));
        let usage = events.iter().find_map(|event| match event {
            AgentEvent::Usage { tokens, cost_usd } => Some((*tokens, *cost_usd)),
            _ => None,
        });
        assert_eq!(usage, Some((Some(15), Some(0.25))));
        assert_ended(&events, RunStatus::Completed);
    }

    #[test]
    fn maps_confirm_extension_request_with_high_risk() {
        let events = map_frame(&json!({
            "type": "extension_ui_request",
            "id": "req-7",
            "method": "confirm",
            "prompt": "Delete src/main.rs?"
        }));
        let approval = events.iter().find_map(|event| match event {
            AgentEvent::ApprovalRequest {
                request_id, risk, ..
            } => Some((request_id.as_str(), *risk)),
            _ => None,
        });
        assert_eq!(approval, Some(("req-7", RiskLevel::High)));
        match events.last() {
            Some(AgentEvent::Status { state }) => {
                assert_eq!(*state, AgentState::WaitingApproval);
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[test]
    fn confirm_without_danger_uses_medium_and_allow_deny() {
        let events = map_frame(&json!({
            "type": "extension_ui_request",
            "id": "req-8",
            "method": "confirm",
            "prompt": "Run the test suite?"
        }));
        match &events[0] {
            AgentEvent::ApprovalRequest { risk, options, .. } => {
                assert_eq!(*risk, RiskLevel::Medium);
                assert_eq!(options, &vec!["allow".to_string(), "deny".to_string()]);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn select_approval_uses_offered_options() {
        let events = map_frame(&json!({
            "type": "extension_ui_request",
            "id": "req-9",
            "method": "select",
            "prompt": "Which environment?",
            "options": ["staging", "production"]
        }));
        match &events[0] {
            AgentEvent::ApprovalRequest { options, .. } => {
                assert_eq!(
                    options,
                    &vec!["staging".to_string(), "production".to_string()]
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn ignores_presentation_ui_requests() {
        for method in [
            "setWidget",
            "setStatus",
            "notify",
            "setTitle",
            "set_editor_text",
            "open_url",
            "cancel",
        ] {
            let events = map_frame(&json!({
                "type": "extension_ui_request",
                "id": "req-view",
                "method": method,
                "text": "working on it"
            }));
            assert!(
                events.is_empty(),
                "{method} should not emit events: {events:?}"
            );
        }
    }

    #[test]
    fn maps_only_dialog_methods_to_approval() {
        let widget = map_frame(&json!({
            "type": "extension_ui_request",
            "id": "req-widget",
            "method": "setWidget",
            "text": "3 tests running"
        }));
        assert!(widget.is_empty());

        let select = map_frame(&json!({
            "type": "extension_ui_request",
            "id": "req-select",
            "method": "select",
            "prompt": "Which environment?",
            "options": ["staging", "production"]
        }));
        let approvals = select
            .iter()
            .filter(|event| matches!(event, AgentEvent::ApprovalRequest { .. }))
            .count();
        assert_eq!(approvals, 1, "select should open exactly one approval");
        assert_eq!(select.len(), 2);

        let confirm = map_frame(&json!({
            "type": "extension_ui_request",
            "id": "req-delete",
            "method": "confirm",
            "prompt": "Delete src/main.rs?"
        }));
        match confirm.first() {
            Some(AgentEvent::ApprovalRequest { risk, .. }) => {
                assert_eq!(*risk, RiskLevel::High);
            }
            other => panic!("expected ApprovalRequest, got {other:?}"),
        }
    }

    #[test]
    fn maps_tool_lifecycle() {
        let start = map_frame(&json!({
            "type": "tool_execution_start",
            "toolName": "bash",
            "args": { "command": "ls -la" }
        }));
        assert_eq!(start.len(), 1);
        match &start[0] {
            AgentEvent::ToolStart { name, summary } => {
                assert_eq!(name, "bash");
                assert!(summary.contains("ls -la"));
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let end = map_frame(&json!({
            "type": "tool_execution_end",
            "toolName": "bash",
            "isError": true
        }));
        match &end[0] {
            AgentEvent::ToolEnd { name, ok } => {
                assert_eq!(name, "bash");
                assert!(!ok);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn maps_error_and_notice() {
        for frame_type in ["error", "notice"] {
            let events = map_frame(&json!({
                "type": frame_type,
                "message": "rate limited",
                "retryable": true
            }));
            match &events[0] {
                AgentEvent::Error { message, retryable } => {
                    assert_eq!(message, "rate limited");
                    assert!(retryable);
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
    }

    #[test]
    fn handshake_and_unknown_frames_are_ignored() {
        assert!(map_frame(&json!({ "type": "ready" })).is_empty());
        assert!(
            map_frame(&json!({
                "type": "response",
                "id": "state",
                "data": { "sessionId": "session-1" }
            }))
            .is_empty()
        );
        assert!(map_frame(&json!({ "type": "definitely_not_a_frame" })).is_empty());
        assert!(map_frame(&json!({ "no_type": true })).is_empty());
    }

    #[test]
    fn extracts_session_id_from_state_response() {
        let frame = json!({
            "type": "response",
            "id": "state",
            "data": { "sessionId": "session-1" }
        });
        assert_eq!(extract_session_id(&frame).as_deref(), Some("session-1"));
    }

    fn approval(method: &str, options: &[&str]) -> PendingApproval {
        PendingApproval {
            method: method.to_string(),
            options: options.iter().map(|option| option.to_string()).collect(),
        }
    }

    #[test]
    fn encodes_confirm_allow_as_confirmed() {
        let frame = approval_response_frame(
            "req-1",
            &Decision::Allow,
            &approval("confirm", &["allow", "deny"]),
        );
        assert_eq!(
            frame,
            json!({
                "type": "extension_ui_response",
                "id": "req-1",
                "confirmed": true
            })
        );
        assert!(frame.get("value").is_none());
    }

    #[test]
    fn encodes_confirm_deny_as_cancelled() {
        for decision in [Decision::Deny, Decision::Cancel] {
            let frame = approval_response_frame(
                "req-2",
                &decision,
                &approval("confirm", &["allow", "deny"]),
            );
            assert_eq!(
                frame,
                json!({
                    "type": "extension_ui_response",
                    "id": "req-2",
                    "cancelled": true
                })
            );
        }
    }

    #[test]
    fn encodes_confirm_select_as_boolean() {
        for (value, expected) in [
            ("true", true),
            ("yes", true),
            ("allow", true),
            ("deny", false),
        ] {
            let frame = approval_response_frame(
                "req-3",
                &Decision::Select(value.to_string()),
                &approval("confirm", &["allow", "deny"]),
            );
            assert_eq!(frame["confirmed"], Value::Bool(expected), "{value}");
        }
    }

    #[test]
    fn encodes_select_allow_with_first_option() {
        let frame = approval_response_frame(
            "req-4",
            &Decision::Allow,
            &approval("select", &["staging", "production"]),
        );
        assert_eq!(
            frame,
            json!({
                "type": "extension_ui_response",
                "id": "req-4",
                "value": "staging"
            })
        );
    }

    #[test]
    fn encodes_select_explicit_value() {
        let frame = approval_response_frame(
            "req-5",
            &Decision::Select("production".to_string()),
            &approval("select", &["staging", "production"]),
        );
        assert_eq!(
            frame,
            json!({
                "type": "extension_ui_response",
                "id": "req-5",
                "value": "production"
            })
        );
    }

    #[test]
    fn encodes_input_allow_without_options_as_allow() {
        let frame = approval_response_frame("req-6", &Decision::Allow, &approval("input", &[]));
        assert_eq!(
            frame,
            json!({
                "type": "extension_ui_response",
                "id": "req-6",
                "value": "allow"
            })
        );
    }

    #[test]
    fn treats_unsupported_methods_as_cancelled() {
        for method in ["ask", "unknown", ""] {
            let frame = approval_response_frame("req-7", &Decision::Allow, &approval(method, &[]));
            assert_eq!(
                frame,
                json!({
                    "type": "extension_ui_response",
                    "id": "req-7",
                    "cancelled": true
                }),
                "method {method:?} should be cancelled"
            );
        }
    }

    #[test]
    fn manifest_declares_omp_capabilities() {
        let manifest = OmpAdapter::new().manifest();
        assert_eq!(manifest.id, "omp");
        assert!(manifest.capabilities.streaming);
        assert!(manifest.capabilities.approvals);
        assert!(manifest.capabilities.cancel);
        assert!(manifest.capabilities.usage);
        assert!(!manifest.capabilities.cost_cap);
    }
}
