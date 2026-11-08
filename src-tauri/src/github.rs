use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteIssue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,
    pub assignee: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct IssuePage {
    pub not_modified: bool,
    pub etag: Option<String>,
    pub issues: Vec<RemoteIssue>,
}

#[derive(Debug, Clone, Default)]
pub struct IssuePatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<String>,
    pub assignee: Option<Option<String>>,
    /// 本地持有的 `updated_at`；用于乐观并发校验。
    pub expected_updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckRun {
    pub name: String,
    pub state: String,
    pub bucket: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
    pub state: String,
}

/// GitHub 操作面；测试用假实现替代真实 gh/HTTP。
pub trait GitHubOps: Send + Sync {
    /// 条件请求：`etag` 命中时返回 `not_modified=true`。
    fn list_issues(&self, repo: &str, etag: Option<&str>) -> Result<IssuePage, String>;
    fn get_issue(&self, repo: &str, number: u64) -> Result<RemoteIssue, String>;
    fn update_issue(
        &self,
        repo: &str,
        number: u64,
        patch: &IssuePatch,
    ) -> Result<RemoteIssue, String>;
    fn create_issue(&self, repo: &str, title: &str, body: &str) -> Result<RemoteIssue, String>;
    fn create_pull_request(
        &self,
        repo: &str,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<PullRequest, String>;
    fn pull_request_checks(&self, repo: &str, number: u64) -> Result<Vec<CheckRun>, String>;
    /// 合并 PR，并钉住被审查的 head `sha`；分支在等待检查期间被追加提交时 GitHub 会拒绝。
    fn merge_pull_request(&self, repo: &str, number: u64, sha: &str) -> Result<(), String>;
}

const API_BASE: &str = "https://api.github.com";

static CURL_SEQ: AtomicU64 = AtomicU64::new(0);

/// 基于 `gh auth token` + `curl` 的实现；不经过 shell，令牌经 stdin 传给 curl。
pub struct GhOps {
    /// 只缓存成功读到的令牌；失败不写入，下一次调用会重新读取。
    token: Mutex<Option<String>>,
}

fn token_cache_lock(cache: &Mutex<Option<String>>) -> std::sync::MutexGuard<'_, Option<String>> {
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 只缓存成功读到的令牌；`load` 失败时不写入缓存，下一次调用仍会重试。
fn get_or_load_token<F>(cache: &Mutex<Option<String>>, load: F) -> Result<String, String>
where
    F: FnOnce() -> Result<String, String>,
{
    if let Some(token) = token_cache_lock(cache).as_ref() {
        return Ok(token.clone());
    }
    let token = load()?;
    *token_cache_lock(cache) = Some(token.clone());
    Ok(token)
}

fn read_gh_token() -> Result<String, String> {
    let output = Command::new("gh")
        .arg("auth")
        .arg("token")
        .output()
        .map_err(|e| format!("无法执行 gh auth token: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh auth token 失败: {}", stderr.trim()));
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if token.is_empty() {
        return Err("gh auth token 未返回令牌".to_string());
    }
    Ok(token)
}

fn escape_curl_config(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

impl GhOps {
    pub fn new() -> Self {
        Self {
            token: Mutex::new(None),
        }
    }

    fn token(&self) -> Result<String, String> {
        get_or_load_token(&self.token, read_gh_token)
    }

    fn clear_token(&self) {
        *token_cache_lock(&self.token) = None;
    }

    fn call(
        &self,
        method: &str,
        url: &str,
        body: Option<&str>,
        etag: Option<&str>,
    ) -> Result<HttpOutput, String> {
        let token = self.token()?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tag = format!(
            "{}-{}-{}",
            std::process::id(),
            nonce,
            CURL_SEQ.fetch_add(1, Ordering::Relaxed)
        );
        let headers_path = std::env::temp_dir().join(format!("temli-gh-{tag}-headers.txt"));
        let body_path = std::env::temp_dir().join(format!("temli-gh-{tag}-body.txt"));

        let mut cmd = Command::new("curl");
        cmd.arg("-sS")
            .arg("--connect-timeout")
            .arg("10")
            .arg("--max-time")
            .arg("30")
            .arg("-D")
            .arg(&headers_path)
            .arg("-o")
            .arg(&body_path)
            .arg("-w")
            .arg("%{http_code}")
            .arg("-X")
            .arg(method)
            .arg("--config")
            .arg("-")
            .arg("-H")
            .arg("Accept: application/vnd.github+json")
            .arg("-H")
            .arg("X-GitHub-Api-Version: 2022-11-28");
        if let Some(etag) = etag {
            cmd.arg("-H").arg(format!("If-None-Match: {etag}"));
        }
        if let Some(body) = body {
            cmd.arg("-H")
                .arg("Content-Type: application/json")
                .arg("--data-binary")
                .arg(body);
        }
        cmd.arg(url);

        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("无法执行 curl: {e}"))?;
        let write_result = match child.stdin.as_mut() {
            Some(stdin) => writeln!(
                stdin,
                "header = \"Authorization: Bearer {}\"",
                escape_curl_config(&token)
            )
            .map_err(|e| format!("写入 curl 配置失败: {e}")),
            None => Err("curl 标准输入不可用".to_string()),
        };
        drop(child.stdin.take());
        if let Err(err) = write_result {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&headers_path);
            let _ = std::fs::remove_file(&body_path);
            return Err(err);
        }
        let output = child
            .wait_with_output()
            .map_err(|e| format!("等待 curl 失败: {e}"))?;
        let headers_bytes = std::fs::read(&headers_path).unwrap_or_default();
        let body_bytes = std::fs::read(&body_path).unwrap_or_default();
        let _ = std::fs::remove_file(&headers_path);
        let _ = std::fs::remove_file(&body_path);

        if !output.status.success() {
            return Err(format!(
                "curl 失败: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let parsed = parse_http_output(
            &String::from_utf8_lossy(&output.stdout),
            &String::from_utf8_lossy(&headers_bytes),
            &String::from_utf8_lossy(&body_bytes),
        )?;
        // 令牌可能已被吊销：丢掉缓存，下一次调用重新执行 `gh auth token`。
        if parsed.status == 401 {
            self.clear_token();
        }
        Ok(parsed)
    }
}

impl GitHubOps for GhOps {
    fn list_issues(&self, repo: &str, etag: Option<&str>) -> Result<IssuePage, String> {
        collect_issue_pages(etag, |page, page_etag| {
            let url = format!("{API_BASE}/repos/{repo}/issues?state=all&per_page=100&page={page}");
            self.call("GET", &url, None, page_etag)
        })
    }

    fn get_issue(&self, repo: &str, number: u64) -> Result<RemoteIssue, String> {
        let url = format!("{API_BASE}/repos/{repo}/issues/{number}");
        let resp = self.call("GET", &url, None, None)?;
        ensure_ok(&resp, "获取 issue")?;
        parse_issue(&resp.body)
    }

    fn update_issue(
        &self,
        repo: &str,
        number: u64,
        patch: &IssuePatch,
    ) -> Result<RemoteIssue, String> {
        let current = self.get_issue(repo, number)?;
        if current.updated_at != patch.expected_updated_at {
            return Err("远端已变更".to_string());
        }

        let mut payload = Map::new();
        if let Some(title) = &patch.title {
            payload.insert("title".to_string(), Value::String(title.to_string()));
        }
        if let Some(body) = &patch.body {
            payload.insert("body".to_string(), Value::String(body.to_string()));
        }
        if let Some(state) = &patch.state {
            payload.insert("state".to_string(), Value::String(state.to_string()));
        }
        if let Some(assignee) = &patch.assignee {
            payload.insert(
                "assignee".to_string(),
                match assignee {
                    Some(login) => Value::String(login.to_string()),
                    None => Value::Null,
                },
            );
        }
        let body = Value::Object(payload).to_string();

        let url = format!("{API_BASE}/repos/{repo}/issues/{number}");
        let resp = self.call("PATCH", &url, Some(&body), None)?;
        ensure_ok(&resp, "更新 issue")?;
        parse_issue(&resp.body)
    }

    fn create_issue(&self, repo: &str, title: &str, body: &str) -> Result<RemoteIssue, String> {
        let payload = serde_json::json!({ "title": title, "body": body }).to_string();
        let url = format!("{API_BASE}/repos/{repo}/issues");
        let resp = self.call("POST", &url, Some(&payload), None)?;
        ensure_ok(&resp, "创建 issue")?;
        parse_issue(&resp.body)
    }

    fn create_pull_request(
        &self,
        repo: &str,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<PullRequest, String> {
        let payload = serde_json::json!({
            "title": title,
            "head": head,
            "base": base,
            "body": body,
        })
        .to_string();
        let url = format!("{API_BASE}/repos/{repo}/pulls");
        let resp = self.call("POST", &url, Some(&payload), None)?;
        ensure_ok(&resp, "创建 PR")?;
        parse_pull_request(&resp.body)
    }

    fn pull_request_checks(&self, repo: &str, number: u64) -> Result<Vec<CheckRun>, String> {
        let pr_url = format!("{API_BASE}/repos/{repo}/pulls/{number}");
        let pr_resp = self.call("GET", &pr_url, None, None)?;
        ensure_ok(&pr_resp, "获取 PR")?;
        let head: PullHeadPayload =
            serde_json::from_str(&pr_resp.body).map_err(|e| format!("解析 PR head 失败: {e}"))?;

        let checks_url = format!(
            "{API_BASE}/repos/{repo}/commits/{}/check-runs?per_page=100",
            head.head.sha
        );
        let resp = self.call("GET", &checks_url, None, None)?;
        ensure_ok(&resp, "获取 PR 检查")?;
        parse_check_runs(&resp.body)
    }

    fn merge_pull_request(&self, repo: &str, number: u64, sha: &str) -> Result<(), String> {
        let payload = merge_pull_request_payload(sha);
        let url = format!("{API_BASE}/repos/{repo}/pulls/{number}/merge");
        let resp = self.call("PUT", &url, Some(&payload), None)?;
        ensure_ok(&resp, "合并 PR")?;
        Ok(())
    }
}

fn merge_pull_request_payload(sha: &str) -> String {
    serde_json::json!({ "merge_method": "squash", "sha": sha }).to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HttpOutput {
    status: u16,
    /// 小写头名；多次出现时取最后一次。
    headers: Vec<(String, String)>,
    body: String,
}

impl HttpOutput {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .rev()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// 解析 `curl -w '%{http_code}'` 的 stdout、`-D` 头文件与 `-o` body 文件。
/// 重定向残留的多个响应块只保留最后一个。
fn parse_http_output(status: &str, headers: &str, body: &str) -> Result<HttpOutput, String> {
    let status = status
        .trim()
        .parse::<u16>()
        .map_err(|_| format!("curl 返回无效状态码: {:?}", status.trim()))?;

    let mut parsed: Vec<(String, String)> = Vec::new();
    for line in headers.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        if line.starts_with("HTTP/") {
            parsed.clear();
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            parsed.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }

    Ok(HttpOutput {
        status,
        headers: parsed,
        body: body.to_string(),
    })
}

fn ensure_ok(resp: &HttpOutput, action: &str) -> Result<(), String> {
    if (200..300).contains(&resp.status) {
        Ok(())
    } else {
        Err(format!(
            "{action}失败: HTTP {} {}",
            resp.status,
            resp.body.trim()
        ))
    }
}

/// 逐页抓取 issue：第 1 页带 `If-None-Match`，命中 304 时直接返回；
/// 之后无条件翻页，直到某页原始条目少于 100。按编号去重并丢弃 PR 条目，
/// ETag 始终取第 1 页的响应头。
fn collect_issue_pages<F>(etag: Option<&str>, mut fetch: F) -> Result<IssuePage, String>
where
    F: FnMut(u64, Option<&str>) -> Result<HttpOutput, String>,
{
    let mut seen = HashSet::new();
    let mut issues = Vec::new();
    let mut first_etag = None;
    let mut page = 1u64;
    loop {
        let resp = fetch(page, if page == 1 { etag } else { None })?;
        if page == 1 {
            first_etag = resp.header("etag").map(str::to_string);
            if resp.status == 304 {
                return Ok(IssuePage {
                    not_modified: true,
                    etag: first_etag,
                    issues: Vec::new(),
                });
            }
        }
        ensure_ok(&resp, "获取 issue 列表")?;
        let (raw_count, page_issues) = parse_issue_page(&resp.body)?;
        for issue in page_issues {
            if seen.insert(issue.number) {
                issues.push(issue);
            }
        }
        if raw_count < 100 {
            return Ok(IssuePage {
                not_modified: false,
                etag: first_etag,
                issues,
            });
        }
        page += 1;
    }
}

#[derive(Debug, Deserialize)]
struct IssuePayload {
    number: u64,
    title: String,
    body: Option<String>,
    state: String,
    assignee: Option<AssigneePayload>,
    updated_at: String,
    pull_request: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct AssigneePayload {
    login: String,
}

impl From<IssuePayload> for RemoteIssue {
    fn from(value: IssuePayload) -> Self {
        RemoteIssue {
            number: value.number,
            title: value.title,
            body: value.body.unwrap_or_default(),
            state: value.state,
            assignee: value.assignee.map(|a| a.login),
            updated_at: value.updated_at,
        }
    }
}

fn parse_issue_page(json: &str) -> Result<(usize, Vec<RemoteIssue>), String> {
    let payloads: Vec<IssuePayload> =
        serde_json::from_str(json).map_err(|e| format!("解析 issue 列表失败: {e}"))?;
    let raw_count = payloads.len();
    let issues = payloads
        .into_iter()
        .filter(|issue| issue.pull_request.is_none())
        .map(RemoteIssue::from)
        .collect();
    Ok((raw_count, issues))
}

fn parse_issue(json: &str) -> Result<RemoteIssue, String> {
    let payload: IssuePayload =
        serde_json::from_str(json).map_err(|e| format!("解析 issue 失败: {e}"))?;
    Ok(payload.into())
}

#[derive(Debug, Deserialize)]
struct CheckRunsPayload {
    check_runs: Vec<CheckRunPayload>,
}

#[derive(Debug, Deserialize)]
struct CheckRunPayload {
    name: String,
    status: String,
    conclusion: Option<String>,
}

fn parse_check_runs(json: &str) -> Result<Vec<CheckRun>, String> {
    let payload: CheckRunsPayload =
        serde_json::from_str(json).map_err(|e| format!("解析 check-runs 失败: {e}"))?;
    Ok(payload
        .check_runs
        .into_iter()
        .map(|check| {
            let bucket = check
                .conclusion
                .filter(|conclusion| !conclusion.is_empty())
                .unwrap_or_else(|| "pending".to_string());
            CheckRun {
                name: check.name,
                state: check.status,
                bucket,
            }
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct PullHeadPayload {
    head: PullHead,
}

#[derive(Debug, Deserialize)]
struct PullHead {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct PullRequestPayload {
    number: u64,
    html_url: String,
    state: String,
}

fn parse_pull_request(json: &str) -> Result<PullRequest, String> {
    let payload: PullRequestPayload =
        serde_json::from_str(json).map_err(|e| format!("解析 PR 失败: {e}"))?;
    Ok(PullRequest {
        number: payload.number,
        url: payload.html_url,
        state: payload.state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_output_and_headers() {
        let headers = "HTTP/2 200\r\netag: \"abc\"\r\nContent-Type: application/json\r\n\r\n";
        let resp = parse_http_output("200\n", headers, "{\"ok\":true}").unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.header("ETag"), Some("\"abc\""));
        assert_eq!(resp.header("content-type"), Some("application/json"));
        assert_eq!(resp.body, "{\"ok\":true}");
    }

    #[test]
    fn keeps_last_header_block() {
        let headers = "HTTP/1.1 100 Continue\r\netag: old\r\n\r\nHTTP/2 200\r\netag: new\r\n\r\n";
        let resp = parse_http_output("200", headers, "x").unwrap();
        assert_eq!(resp.header("etag"), Some("new"));
    }

    #[test]
    fn rejects_invalid_status() {
        let err = parse_http_output("oops", "", "").unwrap_err();
        assert!(err.contains("状态码"));
    }

    #[test]
    fn parses_issue_list_skipping_pull_requests() {
        let json = r#"[
            {
                "number": 7,
                "title": "修复登录",
                "body": null,
                "state": "open",
                "assignee": null,
                "updated_at": "2026-01-02T03:04:05Z"
            },
            {
                "number": 8,
                "title": "a pull request",
                "body": "pr body",
                "state": "open",
                "assignee": {"login": "octocat"},
                "updated_at": "2026-01-03T00:00:00Z",
                "pull_request": {"url": "https://api.github.com/pulls/8"}
            }
        ]"#;
        let (raw_count, issues) = parse_issue_page(json).unwrap();
        assert_eq!(raw_count, 2);
        assert_eq!(
            issues,
            vec![RemoteIssue {
                number: 7,
                title: "修复登录".to_string(),
                body: String::new(),
                state: "open".to_string(),
                assignee: None,
                updated_at: "2026-01-02T03:04:05Z".to_string(),
            }]
        );
    }

    #[test]
    fn parses_issue_with_assignee() {
        let json = r#"{
            "number": 12,
            "title": "t",
            "body": "内容",
            "state": "closed",
            "assignee": {"login": "octocat"},
            "updated_at": "2026-02-02T00:00:00Z"
        }"#;
        let issue = parse_issue(json).unwrap();
        assert_eq!(issue.number, 12);
        assert_eq!(issue.body, "内容");
        assert_eq!(issue.assignee.as_deref(), Some("octocat"));
        assert_eq!(issue.state, "closed");
    }

    #[test]
    fn parses_check_runs_into_buckets() {
        let json = r#"{
            "total_count": 3,
            "check_runs": [
                {"name": "build", "status": "completed", "conclusion": "success"},
                {"name": "lint", "status": "in_progress", "conclusion": null},
                {"name": "docs", "status": "completed", "conclusion": "skipped"}
            ]
        }"#;
        let checks = parse_check_runs(json).unwrap();
        assert_eq!(
            checks,
            vec![
                CheckRun {
                    name: "build".to_string(),
                    state: "completed".to_string(),
                    bucket: "success".to_string(),
                },
                CheckRun {
                    name: "lint".to_string(),
                    state: "in_progress".to_string(),
                    bucket: "pending".to_string(),
                },
                CheckRun {
                    name: "docs".to_string(),
                    state: "completed".to_string(),
                    bucket: "skipped".to_string(),
                },
            ]
        );
    }

    #[test]
    fn parses_pull_request() {
        let json = r#"{
            "number": 3,
            "html_url": "https://github.com/o/r/pull/3",
            "state": "open"
        }"#;
        let pr = parse_pull_request(json).unwrap();
        assert_eq!(
            pr,
            PullRequest {
                number: 3,
                url: "https://github.com/o/r/pull/3".to_string(),
                state: "open".to_string(),
            }
        );
    }

    #[test]
    fn conditional_304_returns_not_modified_page() {
        let page = collect_issue_pages(
            Some("\"abc\""),
            |number, etag| -> Result<HttpOutput, String> {
                assert_eq!(number, 1);
                assert_eq!(etag, Some("\"abc\""));
                Ok(HttpOutput {
                    status: 304,
                    headers: vec![("etag".to_string(), "W/\"abc\"".to_string())],
                    body: String::new(),
                })
            },
        )
        .unwrap();
        assert!(page.not_modified);
        assert_eq!(page.etag.as_deref(), Some("W/\"abc\""));
        assert!(page.issues.is_empty());
    }

    #[test]
    fn http_error_includes_status_and_trimmed_body() {
        let err = collect_issue_pages(None, |_, _| -> Result<HttpOutput, String> {
            Ok(HttpOutput {
                status: 422,
                headers: Vec::new(),
                body: "  {\"message\":\"Validation Failed\"}\n".to_string(),
            })
        })
        .unwrap_err();
        assert!(err.contains("422"));
        assert!(err.contains("{\"message\":\"Validation Failed\"}"));
        assert!(!err.ends_with('\n'));
    }

    fn issue_entry(number: u64, pull_request: bool) -> String {
        let mut value = serde_json::json!({
            "number": number,
            "title": format!("issue {number}"),
            "body": Value::Null,
            "state": "open",
            "assignee": Value::Null,
            "updated_at": "2026-01-01T00:00:00Z",
        });
        if pull_request {
            value["pull_request"] = serde_json::json!({
                "url": format!("https://api.github.com/pulls/{number}")
            });
        }
        value.to_string()
    }

    fn issues_json(entries: impl IntoIterator<Item = (u64, bool)>) -> String {
        let entries: Vec<String> = entries
            .into_iter()
            .map(|(number, pull_request)| issue_entry(number, pull_request))
            .collect();
        format!("[{}]", entries.join(","))
    }

    #[test]
    fn aggregates_all_issue_pages_and_dedupes() {
        let mut requests: Vec<(u64, Option<String>)> = Vec::new();
        let page = collect_issue_pages(
            Some("\"page-1\""),
            |number, etag| -> Result<HttpOutput, String> {
                requests.push((number, etag.map(str::to_string)));
                match number {
                    1 => Ok(HttpOutput {
                        status: 200,
                        headers: vec![("etag".to_string(), "\"page-1\"".to_string())],
                        body: issues_json((1..=100).map(|n| (n, false))),
                    }),
                    2 => Ok(HttpOutput {
                        status: 200,
                        headers: vec![("etag".to_string(), "\"page-2\"".to_string())],
                        body: issues_json([(101, false), (50, false), (102, true)]),
                    }),
                    other => Err(format!("unexpected page {other}")),
                }
            },
        )
        .unwrap();

        assert_eq!(
            requests,
            vec![(1, Some("\"page-1\"".to_string())), (2, None)]
        );
        assert!(!page.not_modified);
        // ETag 只取第 1 页。
        assert_eq!(page.etag.as_deref(), Some("\"page-1\""));
        // 100 + 新增 1 条；重复的 50 与 PR 102 被丢弃。
        assert_eq!(page.issues.len(), 101);
        assert_eq!(page.issues.first().map(|issue| issue.number), Some(1));
        assert_eq!(page.issues.last().map(|issue| issue.number), Some(101));
    }

    #[test]
    fn merge_payload_pins_checked_sha() {
        let payload = merge_pull_request_payload("abc123");
        let value: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["merge_method"], "squash");
        assert_eq!(value["sha"], "abc123");
    }

    #[test]
    fn token_failure_is_not_cached() {
        let cache = Mutex::new(None);
        let err = get_or_load_token(&cache, || Err("gh 未登录".to_string())).unwrap_err();
        assert!(err.contains("gh 未登录"));
        // 失败没有写入缓存：下一次调用会重新读取并成功。
        let token = get_or_load_token(&cache, || Ok("tok".to_string())).unwrap();
        assert_eq!(token, "tok");
        // 成功令牌命中缓存，不再调用 loader。
        let cached = get_or_load_token(&cache, || panic!("缓存命中时不应重新读取令牌")).unwrap();
        assert_eq!(cached, "tok");
    }
}
