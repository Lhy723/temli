//! Codex 适配器：以非交互模式驱动 `codex exec`。
//!
//! Codex 不提供结构化 RPC：stdout 按行透传为文本增量，stderr 只在进程
//! 非零退出时作为错误详情上报。命令行构造与退出状态映射都是纯函数，
//! 测试不会启动真实 CLI。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::{
    AgentAdapter, AgentEvent, AgentManifest, AgentRole, AgentState, Capabilities, Decision,
    EventSink, RunSpec, RunStatus, SessionHandle,
};
#[cfg(test)]
use super::{AuthState, DetectResult};

/// stderr 保留的最大字节数，超出后只保留尾部。
const STDERR_LIMIT: usize = 4 * 1024;
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Default)]
pub struct CodexAdapter {
    sessions: Mutex<HashMap<String, SessionEntry>>,
}

struct SessionEntry {
    child: Arc<Mutex<Child>>,
    sink: EventSink,
    ended: Arc<AtomicBool>,
}

impl CodexAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    fn sessions(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, SessionEntry>>, String> {
        self.sessions
            .lock()
            .map_err(|_| "codex 会话表锁被占用".to_string())
    }
}

impl AgentAdapter for CodexAdapter {
    fn manifest(&self) -> AgentManifest {
        AgentManifest {
            id: "codex".to_string(),
            name: "Codex".to_string(),
            version: VERSION.to_string(),
            capabilities: Capabilities {
                streaming: true,
                approvals: false,
                cancel: true,
                usage: false,
                cost_cap: false,
                resume: false,
            },
            roles: AgentRole::all(),
        }
    }

    /// 探测 `codex` 是否可用；仅测试使用，生产代码不调用。
    #[cfg(test)]
    fn detect(&self) -> DetectResult {
        match super::probe_version("codex") {
            Ok(version) => {
                let detail = if version.is_none() {
                    Some("codex --version 未输出版本号".to_string())
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

    /// 启动一次 `codex exec`：prompt 通过 stdin 写入后立即关闭，
    /// stdout 逐行转发为文本增量，进程退出后按退出码收敛终态。
    fn start(&self, spec: RunSpec, sink: EventSink) -> Result<SessionHandle, String> {
        let handle_id = format!("codex-{}", uuid::Uuid::new_v4());
        let mut command = Command::new("codex");
        command
            .args(build_args(&spec))
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        apply_env(&mut command, &spec.env_allowlist);
        let mut child = command
            .spawn()
            .map_err(|error| format!("无法启动 codex: {error}"))?;

        let (stdin, stdout, stderr) =
            match (child.stdin.take(), child.stdout.take(), child.stderr.take()) {
                (Some(stdin), Some(stdout), Some(stderr)) => (stdin, stdout, stderr),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("codex 管道不可用".to_string());
                }
            };

        // 写入 prompt 后 stdin 随 helper 返回被丢弃；
        // `codex exec -` 读到 EOF 后才开始执行。
        if let Err(error) = write_prompt(stdin, &spec.prompt) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("写入 codex prompt 失败: {error}"));
        }

        sink(AgentEvent::Status {
            state: AgentState::Running,
        });

        let child = Arc::new(Mutex::new(child));
        let ended = Arc::new(AtomicBool::new(false));
        let stderr_buffer = Arc::new(Mutex::new(String::new()));

        let stderr_handle = {
            let buffer = stderr_buffer.clone();
            thread::spawn(move || read_stderr(stderr, buffer))
        };

        {
            let reader_child = child.clone();
            let reader_ended = ended.clone();
            let reader_sink = sink.clone();
            let _ = thread::spawn(move || {
                read_stdout(
                    stdout,
                    reader_child,
                    stderr_buffer,
                    stderr_handle,
                    reader_ended,
                    reader_sink,
                )
            });
        }

        self.sessions()?
            .insert(handle_id.clone(), SessionEntry { child, sink, ended });

        Ok(SessionHandle { id: handle_id })
    }

    fn respond(
        &self,
        _handle: &SessionHandle,
        request_id: &str,
        _decision: Decision,
    ) -> Result<(), String> {
        // 该后端不声明审批能力，正常情况下不会收到审批请求。
        Err(format!("codex 适配器不支持审批回复: {request_id}"))
    }

    /// 杀掉子进程并结束为 Cancelled；重复调用是幂等的。
    fn cancel(&self, handle: &SessionHandle) -> Result<(), String> {
        let session = {
            let sessions = self.sessions()?;
            sessions
                .get(&handle.id)
                .map(|entry| (entry.child.clone(), entry.sink.clone(), entry.ended.clone()))
        };
        let Some((child, sink, ended)) = session else {
            return Ok(());
        };

        // 先发布终态再杀进程：读者线程即便因进程退出而返回，
        // 其 Failed 终态也会被终态去重丢弃。
        emit_event(
            &sink,
            &ended,
            AgentEvent::Ended {
                status: RunStatus::Cancelled,
            },
        );
        kill_child(&child);
        Ok(())
    }

    /// 关闭会话：标记终态后杀掉并回收子进程，不发送任何 UI 事件。
    /// 重复调用（或关闭已结束的会话）是幂等的。
    fn close(&self, handle: &SessionHandle) -> Result<(), String> {
        let entry = self.sessions()?.remove(&handle.id);
        if let Some(entry) = entry {
            entry.ended.store(true, Ordering::SeqCst);
            kill_child(&entry.child);
        }
        Ok(())
    }
}

/// 构造 `codex exec` 参数；末尾的 `-` 表示 prompt 从 stdin 读取。
fn build_args(spec: &RunSpec) -> Vec<String> {
    let mut args = vec![
        "exec".to_string(),
        "--sandbox".to_string(),
        "workspace-write".to_string(),
        "--skip-git-repo-check".to_string(),
        "-C".to_string(),
        spec.cwd.to_string_lossy().into_owned(),
    ];
    if let Some(model) = spec
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        args.push("-m".to_string());
        args.push(model.to_string());
    }
    args.push("-".to_string());
    args
}

/// 把 prompt 写入 stdin；调用方随后丢弃 stdin 以发送 EOF。
fn write_prompt<W: Write>(mut stdin: W, prompt: &str) -> std::io::Result<()> {
    stdin.write_all(prompt.as_bytes())
}

/// 非空白名单时清空继承环境，只注入白名单中实际存在的变量。
fn apply_env(command: &mut Command, allowlist: &[String]) {
    if allowlist.is_empty() {
        return;
    }
    command.env_clear();
    for name in allowlist {
        if let Ok(value) = std::env::var(name) {
            command.env(name, value);
        }
    }
}

/// stdout 的每一行转换成文本增量；保留换行以维持原始格式。
fn line_event(line: &str) -> AgentEvent {
    AgentEvent::TextDelta {
        text: format!("{line}\n"),
    }
}

/// 退出码到终态的映射：只有 0 视为完成，信号终止也按失败处理。
fn exit_run_status(code: Option<i32>) -> RunStatus {
    if code == Some(0) {
        RunStatus::Completed
    } else {
        RunStatus::Failed
    }
}

/// 构造非零退出时的错误消息。
fn failure_message(code: Option<i32>, detail: &str) -> String {
    let base = match code {
        Some(code) => format!("codex 退出码 {code}"),
        None => "codex 被信号终止".to_string(),
    };
    let detail = detail.trim();
    if detail.is_empty() {
        base
    } else {
        format!("{base}: {detail}")
    }
}

fn stderr_detail(stderr: &Mutex<String>) -> String {
    stderr
        .lock()
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

/// 持续排空 stderr 并保留尾部内容，供失败时上报。
fn read_stderr(stderr: ChildStderr, buffer: Arc<Mutex<String>>) {
    let mut text = String::new();
    for line in BufReader::new(stderr).lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&line);
        if text.len() > STDERR_LIMIT {
            let mut start = text.len() - STDERR_LIMIT;
            while !text.is_char_boundary(start) {
                start -= 1;
            }
            text.drain(..start);
        }
    }
    if let Ok(mut buffer) = buffer.lock() {
        *buffer = text;
    }
}

/// 读取 stdout、等待进程退出，并在回收 stderr 后收敛终态。
fn read_stdout(
    stdout: ChildStdout,
    child: Arc<Mutex<Child>>,
    stderr: Arc<Mutex<String>>,
    stderr_handle: thread::JoinHandle<()>,
    ended: Arc<AtomicBool>,
    sink: EventSink,
) {
    let mut read_error = None;
    for line in BufReader::new(stdout).lines() {
        match line {
            Ok(line) => emit_event(&sink, &ended, line_event(&line)),
            Err(error) => {
                read_error = Some(format!("读取 codex 输出失败: {error}"));
                break;
            }
        }
    }

    let exit = wait_child(&child);
    // 等 stderr 排空后再读取失败详情。
    let _ = stderr_handle.join();

    let code = match exit {
        Ok(status) => status.code(),
        Err(message) => {
            fail(&sink, &ended, message);
            return;
        }
    };

    let status = if read_error.is_some() {
        RunStatus::Failed
    } else {
        exit_run_status(code)
    };
    if status == RunStatus::Failed {
        let message = match read_error {
            Some(message) => message,
            None => failure_message(code, &stderr_detail(&stderr)),
        };
        emit_event(
            &sink,
            &ended,
            AgentEvent::Error {
                message,
                retryable: false,
            },
        );
    }
    emit_event(&sink, &ended, AgentEvent::Ended { status });
}

fn fail(sink: &EventSink, ended: &AtomicBool, message: String) {
    emit_event(
        sink,
        ended,
        AgentEvent::Error {
            message,
            retryable: false,
        },
    );
    emit_event(
        sink,
        ended,
        AgentEvent::Ended {
            status: RunStatus::Failed,
        },
    );
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

/// 轮询子进程退出状态；两次尝试之间释放进程锁，
/// 保证 `cancel` / `close` 仍能杀掉迟迟不退出的子进程。
fn wait_child(child: &Mutex<Child>) -> Result<std::process::ExitStatus, String> {
    loop {
        let status = {
            let mut guard = child.lock().map_err(|_| "codex 进程锁被占用".to_string())?;
            guard
                .try_wait()
                .map_err(|error| format!("等待 codex 退出失败: {error}"))?
        };
        if let Some(status) = status {
            return Ok(status);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn kill_child(child: &Mutex<Child>) {
    // 锁中毒时也要尽量完成清理。
    let mut guard = child
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    kill_process_group(guard.id());
    let _ = guard.kill();
    let _ = guard.wait();
}

/// 向进程组发送 SIGKILL。子进程通过 `process_group(0)` 成为新进程组组长，
/// 其 pid 即 pgid；`kill` 的负 pid 参数表示对整个进程组发信号，
/// 从而一并终止 CLI 派生的孙进程。
fn kill_process_group(pgid: u32) {
    const SIGKILL: i32 = 9;
    // SAFETY: `kill` 是 libc 的系统调用封装；pgid 来自刚启动的子进程 pid，
    // 取负值表示进程组，信号为固定的 SIGKILL。失败（如进程组已不存在）时
    // 忽略返回值，调用方随后仍会杀死并回收子进程本身。
    unsafe {
        kill(-(pgid as i32), SIGKILL);
    }
}

unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn run_spec() -> RunSpec {
        RunSpec {
            cwd: PathBuf::from("/tmp/work tree"),
            prompt: "修复失败测试".into(),
            model: None,
            max_minutes: 5,
            env_allowlist: Vec::new(),
        }
    }

    #[test]
    fn build_args_requests_workspace_sandbox_and_stdin_prompt() {
        let args = build_args(&run_spec());
        assert_eq!(
            args,
            vec![
                "exec",
                "--sandbox",
                "workspace-write",
                "--skip-git-repo-check",
                "-C",
                "/tmp/work tree",
                "-",
            ]
        );
    }

    #[test]
    fn build_args_appends_model_before_stdin_marker() {
        let mut spec = run_spec();
        spec.model = Some("gpt-5-codex".into());
        let args = build_args(&spec);
        assert_eq!(
            args[args.len() - 3..].to_vec(),
            vec!["-m", "gpt-5-codex", "-"],
        );
    }

    #[test]
    fn build_args_ignores_blank_model() {
        let mut spec = run_spec();
        spec.model = Some("  ".into());
        let args = build_args(&spec);
        assert_eq!(args.last().map(String::as_str), Some("-"));
        assert!(!args.iter().any(|arg| arg == "-m"));
    }

    #[test]
    fn write_prompt_sends_prompt_bytes_only() {
        let mut stdin = Vec::new();
        write_prompt(&mut stdin, "修复失败测试").unwrap();
        assert_eq!(stdin, "修复失败测试".as_bytes());
    }

    #[test]
    fn exit_code_maps_to_run_status() {
        assert_eq!(exit_run_status(Some(0)), RunStatus::Completed);
        assert_eq!(exit_run_status(Some(1)), RunStatus::Failed);
        assert_eq!(exit_run_status(Some(127)), RunStatus::Failed);
        assert_eq!(exit_run_status(None), RunStatus::Failed);
    }

    #[test]
    fn stdout_lines_become_text_deltas() {
        match line_event("hello") {
            AgentEvent::TextDelta { text } => assert_eq!(text, "hello\n"),
            other => panic!("意外事件: {other:?}"),
        }
    }

    #[test]
    fn failure_message_includes_status_and_stderr() {
        assert_eq!(failure_message(Some(2), "boom\n"), "codex 退出码 2: boom");
        assert_eq!(failure_message(Some(2), "  "), "codex 退出码 2");
        assert_eq!(failure_message(None, ""), "codex 被信号终止");
    }

    #[test]
    fn cancel_unknown_handle_is_noop() {
        let adapter = CodexAdapter::new();
        let handle = SessionHandle {
            id: "missing".into(),
        };
        assert!(adapter.cancel(&handle).is_ok());
        assert!(adapter.close(&handle).is_ok());
    }

    #[test]
    fn manifest_exposes_both_roles_and_streaming_cancel() {
        let manifest = CodexAdapter::new().manifest();
        assert_eq!(manifest.id, "codex");
        assert!(manifest.capabilities.streaming);
        assert!(manifest.capabilities.cancel);
        assert!(!manifest.capabilities.approvals);
        assert!(!manifest.capabilities.usage);
        assert_eq!(manifest.roles, AgentRole::all());
    }
}
