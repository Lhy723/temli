//! 开发者模式：仅监听 `127.0.0.1` 的本地调试 HTTP API（默认关闭）。
//!
//! 服务由设置里的 `dev_mode` / `dev_port` / `dev_api_token` 控制；所有接口
//! （除 `/health`）要求 `Authorization: Bearer <token>`。每个请求都包在
//! `catch_unwind` 里，处理线程 panic 不会带崩应用。

use crate::{AppError, AppResult, AppState, logging, store};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use tauri::{AppHandle, Manager, State};
use tiny_http::{Header, Response, Server};

/// 日志接口单次最多返回的行数。
const MAX_LOG_LINES: usize = 2000;

/// `/logs` 未指定 `lines` 时的默认行数。
const DEFAULT_LOG_LINES: u32 = 200;

type HttpResponse = Response<Cursor<Vec<u8>>>;

/// 设置页展示的开发者模式状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DevStatus {
    /// 服务当前是否真的在监听（不是只看持久化开关）。
    pub enabled: bool,
    pub port: u16,
    pub token: String,
    pub base_url: String,
}

impl DevStatus {
    fn current(stored_port: u16, token: String) -> Self {
        let running = running_port();
        let port = running.unwrap_or(stored_port);
        Self {
            enabled: running.is_some(),
            port,
            token,
            base_url: base_url(port),
        }
    }
}

fn base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

struct RunningServer {
    server: Arc<Server>,
    thread: JoinHandle<()>,
    port: u16,
}

impl RunningServer {
    /// 让 `recv()` 返回错误并等待服务线程退出。
    fn stop(self) {
        self.server.unblock();
        let _ = self.thread.join();
    }
}

static RUNNING: Mutex<Option<RunningServer>> = Mutex::new(None);

fn running_lock() -> MutexGuard<'static, Option<RunningServer>> {
    RUNNING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 当前实际监听端口；服务线程已退出时视为未运行。
fn running_port() -> Option<u16> {
    running_lock()
        .as_ref()
        .filter(|running| !running.thread.is_finished())
        .map(|running| running.port)
}

/// 校验端口在 `1024..=65535` 内。
pub fn validate_port(port: u32) -> Result<u16, String> {
    if (u32::from(store::MIN_DEV_PORT)..=65535).contains(&port) {
        Ok(port as u16)
    } else {
        Err(format!(
            "端口需在 {} 到 65535 之间：{port}",
            store::MIN_DEV_PORT
        ))
    }
}

/// 启动（或重启）本地 API 服务；已在同端口运行时为空操作。
///
/// 返回 `true` 表示本次新建了服务线程，`false` 表示沿用已有服务。
fn start_server(app: &AppHandle, port: u16, token: String) -> Result<bool, String> {
    let mut guard = running_lock();
    if let Some(running) = guard.as_ref()
        && running.port == port
        && !running.thread.is_finished()
    {
        return Ok(false);
    }

    // 同端口时旧监听必须先释放，否则重新 bind 会失败；换端口时先绑定新端口，
    // 绑定失败则保留旧服务继续运行。
    if guard.as_ref().is_some_and(|running| running.port == port)
        && let Some(old) = guard.take()
    {
        old.stop();
    }

    let server = Arc::new(
        Server::http(("127.0.0.1", port))
            .map_err(|err| format!("无法监听 127.0.0.1:{port}：{err}"))?,
    );
    let thread_server = Arc::clone(&server);
    let thread_app = app.clone();
    let thread = std::thread::Builder::new()
        .name("temli-dev-api".to_string())
        .spawn(move || serve(thread_server, thread_app, token))
        .map_err(|err| format!("无法启动开发者 API 线程：{err}"))?;
    let old = guard.take();
    *guard = Some(RunningServer {
        server,
        thread,
        port,
    });
    drop(guard);
    if let Some(old) = old {
        old.stop();
    }
    Ok(true)
}

/// 停止本地 API 服务；未运行时为空操作。
pub fn stop_server() {
    let old = running_lock().take();
    if let Some(running) = old {
        running.stop();
    }
}

/// 启动时按设置自动开启；失败只写日志，不阻断应用启动。
pub fn autostart(app: &AppHandle) {
    let (port, token) = {
        let state = app.state::<AppState>();
        let db = match state.db.lock() {
            Ok(db) => db,
            Err(_) => {
                logging::log("开发者模式自动启动失败：数据库锁被占用");
                return;
            }
        };
        if !store::dev_mode_enabled(db.conn()) {
            return;
        }
        let port = store::dev_port(db.conn());
        match store::ensure_dev_api_token(db.conn()) {
            Ok(token) => (port, token),
            Err(err) => {
                logging::log(format!("开发者模式自动启动失败：{err}"));
                return;
            }
        }
    };

    match start_server(app, port, token) {
        Ok(_) => logging::log(format!("开发者模式已开启：{}/", base_url(port))),
        Err(err) => logging::log(format!("开发者模式自动启动失败：{err}")),
    }
}

// ---------------------------------------------------------------------------
// Tauri 命令
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_dev_status(state: State<AppState>) -> AppResult<DevStatus> {
    let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
    let stored_port = store::dev_port(db.conn());
    let token = store::get_setting(db.conn(), store::SETTING_DEV_API_TOKEN)?.unwrap_or_default();
    drop(db);
    Ok(DevStatus::current(stored_port, token))
}

#[tauri::command]
pub fn set_dev_mode(
    app: AppHandle,
    state: State<AppState>,
    enabled: bool,
    port: u32,
) -> AppResult<DevStatus> {
    let port = validate_port(port).map_err(AppError::validation)?;
    let token = {
        let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
        store::ensure_dev_api_token(db.conn())?
    };

    if enabled {
        start_server(&app, port, token)?;
    }

    let persisted = (|| -> Result<(), String> {
        let db = state.db.lock().map_err(|_| "数据库锁被占用".to_string())?;
        store::set_setting(db.conn(), store::SETTING_DEV_PORT, &port.to_string())?;
        store::set_setting(
            db.conn(),
            store::SETTING_DEV_MODE,
            if enabled { "1" } else { "0" },
        )
    })();

    if !enabled {
        stop_server();
    }
    persisted?;

    logging::log(format!(
        "开发者模式{}：端口 {port}",
        if enabled { "已开启" } else { "已关闭" }
    ));
    get_dev_status(state)
}

/// 读取日志文件末尾 `lines` 行；上限 2000，文件不存在返回空。
#[tauri::command]
pub fn read_app_log(lines: u32) -> Vec<String> {
    let limit = (lines as usize).min(MAX_LOG_LINES);
    if limit == 0 {
        return Vec::new();
    }
    logging::path()
        .map(|path| tail_lines(path, limit))
        .unwrap_or_default()
}

/// 读取文件末尾 `limit` 行（按 UTF-8 有损解码）。
fn tail_lines(path: &Path, limit: usize) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if lines.len() > limit {
        lines.drain(..lines.len() - limit);
    }
    lines
}

// ---------------------------------------------------------------------------
// HTTP 服务
// ---------------------------------------------------------------------------

/// 请求里路由需要的全部信息；提前转成拥有所有权的值，便于在 panic 兜底后
/// 用原始 `Request` 回写响应。
struct RequestInfo {
    method: String,
    url: String,
    authorization: Option<String>,
}

fn serve(server: Arc<Server>, app: AppHandle, token: String) {
    loop {
        let request = match server.recv() {
            Ok(request) => request,
            // `unblock()` 或监听出错时退出；这是正常的停止路径。
            Err(_) => break,
        };
        let info = RequestInfo {
            method: request.method().as_str().to_string(),
            url: request.url().to_string(),
            authorization: request
                .headers()
                .iter()
                .find(|header| header.field.equiv("Authorization"))
                .map(|header| header.value.as_str().to_string()),
        };
        let response = catch_unwind(AssertUnwindSafe(|| route(&app, &token, &info)))
            .unwrap_or_else(|_| error_response(500, "开发者 API 内部错误"));
        let _ = request.respond(response);
    }
}

fn route(app: &AppHandle, token: &str, request: &RequestInfo) -> HttpResponse {
    let (path, query) = match request.url.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (request.url.as_str(), None),
    };

    // /health 无需认证，方便脚本探测服务是否在监听。
    if request.method == "GET" && path == "/health" {
        return json_response(
            200,
            &json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") }),
        );
    }

    if !authorized(request.authorization.as_deref(), token) {
        return error_response(401, "未授权：需要 Authorization: Bearer <token>");
    }

    if request.method == "POST" {
        if let Some(task_id) = action_id(path, "/tasks/", "/start") {
            return run_task(app, task_id, TaskAction::Start);
        }
        if let Some(task_id) = action_id(path, "/tasks/", "/cancel") {
            return run_task(app, task_id, TaskAction::Cancel);
        }
        if let Some(plan_id) = action_id(path, "/plans/", "/start") {
            return start_plan(app, plan_id);
        }
        if let Some(stage_id) = action_id(path, "/stages/", "/accept") {
            return accept_stage(app, stage_id);
        }
    }

    match (request.method.as_str(), path) {
        ("GET", "/summary") => summary(app),
        ("GET", "/tasks") => tasks(app, query),
        ("GET", "/plans") => plans(app, query),
        ("GET", "/logs") => logs(query),
        _ => error_response(404, "未知接口"),
    }
}

/// 从 `/tasks/<id>/start` 这类路径中取出中间 id。
fn action_id<'a>(path: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    path.strip_prefix(prefix)?
        .strip_suffix(suffix)
        .filter(|id| !id.is_empty() && !id.contains('/'))
}

fn query_param<'a>(query: Option<&'a str>, key: &str) -> Option<&'a str> {
    query?.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        (name == key).then_some(value)
    })
}

fn summary(app: &AppHandle) -> HttpResponse {
    let state = app.state::<AppState>();
    let (project_count, counts) = {
        let db = match state.db.lock() {
            Ok(db) => db,
            Err(_) => return error_response(500, "数据库锁被占用"),
        };
        let project_count = match db.list_projects() {
            Ok(projects) => projects.len(),
            Err(err) => return operation_error(err),
        };
        match store::task_status_counts(db.conn()) {
            Ok(counts) => (project_count, counts),
            Err(err) => return operation_error(err),
        }
    };
    let mut status_counts = serde_json::Map::new();
    // 固定输出全部状态（含 0），让外部脚本拿到稳定的键集合。
    for status in [
        "draft",
        "ready",
        "queued",
        "running",
        "verifying",
        "merged",
        "failed",
        "needs_human",
        "interrupted",
    ] {
        status_counts.insert(status.to_string(), Value::from(0));
    }
    for (status, count) in counts {
        status_counts.insert(status, Value::from(count));
    }
    json_response(
        200,
        &json!({
            "project_count": project_count,
            "task_status_counts": Value::Object(status_counts),
            "default_agent": current_default_agent(&state),
        }),
    )
}

fn current_default_agent(state: &AppState) -> String {
    match state.default_agent.read() {
        Ok(agent) => agent.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

fn tasks(app: &AppHandle, query: Option<&str>) -> HttpResponse {
    let Some(project_id) = query_param(query, "project_id").filter(|id| !id.is_empty()) else {
        return error_response(400, "缺少 project_id 参数");
    };
    let state = app.state::<AppState>();
    let db = match state.db.lock() {
        Ok(db) => db,
        Err(_) => return error_response(500, "数据库锁被占用"),
    };
    match store::list_tasks_for_project(db.conn(), project_id) {
        Ok(tasks) => json_response(200, &json!(tasks)),
        Err(err) => operation_error(err),
    }
}

fn plans(app: &AppHandle, query: Option<&str>) -> HttpResponse {
    let Some(project_id) = query_param(query, "project_id").filter(|id| !id.is_empty()) else {
        return error_response(400, "缺少 project_id 参数");
    };
    let state = app.state::<AppState>();
    let db = match state.db.lock() {
        Ok(db) => db,
        Err(_) => return error_response(500, "数据库锁被占用"),
    };
    match store::list_plans(db.conn(), project_id) {
        Ok(plans) => json_response(200, &json!(plans)),
        Err(err) => operation_error(err),
    }
}

fn logs(query: Option<&str>) -> HttpResponse {
    let lines = query_param(query, "lines")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(DEFAULT_LOG_LINES);
    json_response(200, &json!(read_app_log(lines)))
}

enum TaskAction {
    Start,
    Cancel,
}

fn run_task(app: &AppHandle, task_id: &str, action: TaskAction) -> HttpResponse {
    let state = app.state::<AppState>();
    let result = match action {
        TaskAction::Start => state.scheduler.enqueue(task_id),
        TaskAction::Cancel => state.scheduler.cancel(task_id),
    };
    match result {
        Ok(()) => json_response(200, &json!({ "ok": true })),
        Err(err) => operation_error(err),
    }
}

fn start_plan(app: &AppHandle, plan_id: &str) -> HttpResponse {
    let state = app.state::<AppState>();
    match state.scheduler.start_plan(plan_id) {
        Ok(plan) => json_response(200, &json!(plan)),
        Err(err) => operation_error(err),
    }
}

fn accept_stage(app: &AppHandle, stage_id: &str) -> HttpResponse {
    let state = app.state::<AppState>();
    match state.scheduler.accept_stage(stage_id) {
        Ok(plan) => json_response(200, &json!(plan)),
        Err(err) => operation_error(err),
    }
}

/// 解析 `Authorization` 头里的 Bearer token。
fn parse_bearer(header: Option<&str>) -> Option<&str> {
    let value = header?.trim();
    let (scheme, credentials) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = credentials.trim();
    (!token.is_empty()).then_some(token)
}

/// 校验请求头携带的 token 是否与设置一致（常量时间比较）。
fn authorized(header: Option<&str>, expected: &str) -> bool {
    if expected.is_empty() {
        return false;
    }
    match parse_bearer(header) {
        Some(token) => constant_time_eq(token.as_bytes(), expected.as_bytes()),
        None => false,
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right) {
        diff |= a ^ b;
    }
    diff == 0
}

fn json_response(status: u16, value: &Value) -> HttpResponse {
    let body = serde_json::to_string(value).unwrap_or_else(|err| {
        serde_json::to_string(&json!({ "error": format!("响应序列化失败：{err}") }))
            .unwrap_or_default()
    });
    let mut response = Response::from_string(body).with_status_code(status);
    if let Ok(header) = Header::from_bytes(
        &b"Content-Type"[..],
        &b"application/json; charset=utf-8"[..],
    ) {
        response = response.with_header(header);
    }
    response
}

fn error_response(status: u16, message: &str) -> HttpResponse {
    json_response(status, &json!({ "error": message }))
}

/// 调度/存储错误映射：实体不存在用 404，其它非法操作/参数用 400。
fn operation_error(err: String) -> HttpResponse {
    let status = if err.contains("不存在") { 404 } else { 400 };
    error_response(status, &err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use std::path::PathBuf;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("temli-dev-api-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    #[test]
    fn port_validation_accepts_only_unprivileged_range() {
        assert_eq!(validate_port(1024), Ok(1024));
        assert_eq!(validate_port(3789), Ok(3789));
        assert_eq!(validate_port(65535), Ok(65535));
        assert!(validate_port(0).is_err());
        assert!(validate_port(1023).is_err());
        assert!(validate_port(65536).is_err());
    }

    #[test]
    fn bearer_header_parsing_is_case_insensitive_and_strict() {
        assert_eq!(parse_bearer(Some("Bearer abc-123")), Some("abc-123"));
        assert_eq!(parse_bearer(Some("bearer abc-123")), Some("abc-123"));
        assert_eq!(parse_bearer(Some("  BEARER   abc-123  ")), Some("abc-123"));
        assert_eq!(parse_bearer(Some("Token abc-123")), None);
        assert_eq!(parse_bearer(Some("Bearer ")), None);
        assert_eq!(parse_bearer(Some("Bearer")), None);
        assert_eq!(parse_bearer(None), None);

        assert!(authorized(Some("Bearer s3cret"), "s3cret"));
        assert!(!authorized(Some("Bearer s3cre"), "s3cret"));
        assert!(!authorized(Some("Basic s3cret"), "s3cret"));
        assert!(!authorized(None, "s3cret"));
        assert!(!authorized(Some("Bearer "), "s3cret"));
        // 空 token 永远拒绝，避免未初始化设置时放行。
        assert!(!authorized(Some("Bearer "), ""));
    }

    #[test]
    fn dev_token_is_generated_once_and_round_trips() {
        let dir = temp_dir();
        let db = Db::open(&dir.join("temli.db")).expect("打开临时数据库");
        assert!(!store::dev_mode_enabled(db.conn()));
        assert_eq!(store::dev_port(db.conn()), store::DEFAULT_DEV_PORT);

        let token = store::ensure_dev_api_token(db.conn()).expect("生成 token");
        assert!(!token.is_empty());
        assert!(
            uuid::Uuid::parse_str(&token).is_ok(),
            "token 应为 UUID：{token}"
        );
        assert_eq!(
            store::ensure_dev_api_token(db.conn()).expect("复用 token"),
            token
        );

        store::set_setting(db.conn(), store::SETTING_DEV_MODE, "1").unwrap();
        assert!(store::dev_mode_enabled(db.conn()));
        store::set_setting(db.conn(), store::SETTING_DEV_MODE, "0").unwrap();
        assert!(!store::dev_mode_enabled(db.conn()));

        // 非法端口设置回退默认端口。
        store::set_setting(db.conn(), store::SETTING_DEV_PORT, "80").unwrap();
        assert_eq!(store::dev_port(db.conn()), store::DEFAULT_DEV_PORT);
        store::set_setting(db.conn(), store::SETTING_DEV_PORT, "65535").unwrap();
        assert_eq!(store::dev_port(db.conn()), 65535);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_lines_returns_file_tail_and_handles_missing_file() {
        let dir = temp_dir();
        let path = dir.join("temli.log");
        std::fs::write(&path, "one\ntwo\nthree\nfour\n").expect("写入日志");

        assert_eq!(tail_lines(&path, 2), vec!["three", "four"]);
        assert_eq!(tail_lines(&path, 10), vec!["one", "two", "three", "four"]);
        assert!(tail_lines(&path, 0).is_empty());
        assert!(tail_lines(&dir.join("missing.log"), 5).is_empty());

        std::fs::write(&path, "no-trailing-newline").expect("写入日志");
        assert_eq!(tail_lines(&path, 5), vec!["no-trailing-newline"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn action_id_extracts_entity_ids_only_for_exact_shapes() {
        assert_eq!(
            action_id("/tasks/t-1/start", "/tasks/", "/start"),
            Some("t-1")
        );
        assert_eq!(
            action_id("/stages/s-1/accept", "/stages/", "/accept"),
            Some("s-1")
        );
        assert_eq!(action_id("/tasks//start", "/tasks/", "/start"), None);
        assert_eq!(action_id("/tasks/t-1/stop", "/tasks/", "/start"), None);
        assert_eq!(action_id("/tasks/a/b/start", "/tasks/", "/start"), None);
    }

    #[test]
    fn query_param_matches_exact_key() {
        assert_eq!(
            query_param(Some("project_id=abc&lines=10"), "project_id"),
            Some("abc")
        );
        assert_eq!(
            query_param(Some("project_id=abc&lines=10"), "lines"),
            Some("10")
        );
        assert_eq!(query_param(Some("xproject_id=abc"), "project_id"), None);
        assert_eq!(query_param(None, "project_id"), None);
    }
}
