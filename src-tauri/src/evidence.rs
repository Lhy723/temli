//! 证据执行、验收发现、diff 收集与风险分类。
//!
//! 本模块只做与仓库无关的纯逻辑 + 进程/文件操作：
//! - [`discover_commands`]：从 `package.json`、`Cargo.toml`、`Makefile` 发现可执行命令；
//! - [`default_acceptance`]：把发现到的命令转成默认验收标准；
//! - [`run_command`]：带超时执行命令并捕获（有上限的）输出；
//! - [`collect_diff`]：收集 `base_sha..HEAD` 以及未提交改动的文件清单与补丁；
//! - [`classify_risk`]、[`test_ci_files`]：为审查报告提供风险标记与测试/CI 文件清单。

use crate::store::Criterion;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 单个输出流（stdout/stderr）最多保留的字节数。
const OUTPUT_CAP: usize = 64 * 1024;
/// diff 补丁最多保留的字节数。
const PATCH_CAP: usize = 256 * 1024;
/// `git diff --name-status` 输出的捕获上限，正常仓库远小于此值。
const NAME_STATUS_CAP: usize = 1024 * 1024;
/// 等待子进程退出时的轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(25);
/// 超时杀死进程组后，等待输出读取线程收尾的最长时间；超过则放弃等待。
const READER_JOIN_TIMEOUT: Duration = Duration::from_millis(500);

/// 命令类别，序列化为 snake_case（`test`/`build`/`lint`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CmdKind {
    Test,
    Build,
    Lint,
}

impl CmdKind {
    /// 输出排序权重：test → build → lint。
    fn rank(self) -> u8 {
        match self {
            CmdKind::Test => 0,
            CmdKind::Build => 1,
            CmdKind::Lint => 2,
        }
    }
}

/// 从仓库声明文件中发现到的一条候选命令。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredCommand {
    /// 可直接交给 `sh -c` 执行的完整命令。
    pub cmd: String,
    pub kind: CmdKind,
    /// 来源文件，例如 `package.json`、`Cargo.toml`、`Makefile`。
    pub source: String,
}

/// 带超时执行命令后的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandOutcome {
    /// 退出码；被信号杀死（含超时）或启动失败时为 `None`。
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    /// 最多保留 64 KiB，超长时按 UTF-8 有损截断。
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// 一个改动文件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    /// `git diff --name-status` 的状态首字母：A/M/D/R/C/T/U 等。
    pub status: char,
}

/// base 到 HEAD 的 diff 产物。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffArtifact {
    pub base_sha: String,
    pub files: Vec<FileChange>,
    /// `base_sha..HEAD` 的补丁文本，超过 256 KiB 时被截断。
    pub patch: String,
    pub truncated: bool,
}

/// 风险类别，序列化为 snake_case。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskKind {
    Deleted,
    Dependency,
    Migration,
    Ci,
    Secret,
}

/// 单个文件上的风险标记；一个文件可能命中多个类别（如被删除的依赖文件）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskFlag {
    pub path: String,
    pub kind: RiskKind,
}

/// 发现仓库内可执行的测试/构建/检查命令。
///
/// 支持来源：`package.json` 的 `scripts`（`test`、`test:unit`、`build`、`lint`）、
/// `Cargo.toml`（`cargo test`、`cargo build`，仅当存在 `.clippy.toml`/`clippy.toml`
/// 或 CI 配置提到 clippy 时追加 `cargo clippy`）、`Makefile` 的 `test`/`build`/`lint` 目标。
/// 结果按 `cmd` 去重，并按 test → build → lint 稳定排序（同类内部保持发现顺序）。
pub fn discover_commands(repo: &Path) -> Vec<DiscoveredCommand> {
    let mut commands: Vec<DiscoveredCommand> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    if let Some(scripts) = package_scripts(&repo.join("package.json")) {
        const NPM_SCRIPTS: [(&str, CmdKind); 4] = [
            ("test", CmdKind::Test),
            ("test:unit", CmdKind::Test),
            ("build", CmdKind::Build),
            ("lint", CmdKind::Lint),
        ];
        for (script, kind) in NPM_SCRIPTS {
            if scripts.contains(script) {
                push_command(
                    &mut commands,
                    &mut seen,
                    format!("npm run {script}"),
                    kind,
                    "package.json",
                );
            }
        }
    }

    if repo.join("Cargo.toml").is_file() {
        push_command(
            &mut commands,
            &mut seen,
            "cargo test".to_string(),
            CmdKind::Test,
            "Cargo.toml",
        );
        push_command(
            &mut commands,
            &mut seen,
            "cargo build".to_string(),
            CmdKind::Build,
            "Cargo.toml",
        );
        if clippy_configured(repo) {
            push_command(
                &mut commands,
                &mut seen,
                "cargo clippy".to_string(),
                CmdKind::Lint,
                "Cargo.toml",
            );
        }
    }

    if let Ok(makefile) = fs::read_to_string(repo.join("Makefile")) {
        for target in parse_make_targets(&makefile) {
            let kind = match target.as_str() {
                "test" => CmdKind::Test,
                "build" => CmdKind::Build,
                "lint" => CmdKind::Lint,
                _ => continue,
            };
            push_command(
                &mut commands,
                &mut seen,
                format!("make {target}"),
                kind,
                "Makefile",
            );
        }
    }

    // sort_by_key 是稳定排序：同类命令保持原有发现顺序。
    commands.sort_by_key(|command| command.kind.rank());
    commands
}

/// 把发现到的命令转成默认验收标准：id 为 `c-1`、`c-2`……，要求退出码 0，不做输出匹配。
pub fn default_acceptance(commands: &[DiscoveredCommand]) -> Vec<Criterion> {
    commands
        .iter()
        .enumerate()
        .map(|(index, command)| Criterion {
            id: format!("c-{}", index + 1),
            cmd: command.cmd.clone(),
            expect_exit: 0,
            expect_match: None,
        })
        .collect()
}

/// 在 `cwd` 下用 `/bin/sh -c` 执行命令，超过 `timeout_secs` 秒则杀死整个进程组。
///
/// 子进程通过 `process_group(0)` 成为独立进程组的组长，超时后用 `kill -KILL`
/// 作用于该组，确保 `sh` 派生的后代进程一并终止。`timeout_secs == 0` 表示不设超时。
/// stdout/stderr 各保留前 64 KiB 字节，但会持续读空管道，避免子进程因管道写满而阻塞。
pub fn run_command(cwd: &Path, cmd: &str, timeout_secs: u64) -> CommandOutcome {
    let started = Instant::now();
    let spawned = Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn();

    let mut child = match spawned {
        Ok(child) => child,
        Err(err) => {
            return CommandOutcome {
                exit_code: None,
                duration_ms: elapsed_ms(started),
                stdout: String::new(),
                stderr: format!("无法启动命令: {err}"),
                timed_out: false,
            };
        }
    };

    let stdout_reader = child
        .stdout
        .take()
        .map(|pipe| spawn_reader(pipe, OUTPUT_CAP));
    let stderr_reader = child
        .stderr
        .take()
        .map(|pipe| spawn_reader(pipe, OUTPUT_CAP));
    let deadline = if timeout_secs > 0 {
        Some(Instant::now() + Duration::from_secs(timeout_secs))
    } else {
        None
    };

    let mut timed_out = false;
    let mut killed_group = false;
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    timed_out = true;
                    killed_group = true;
                    kill_process_group(child.id());
                    let _ = child.kill();
                    break child.wait().ok().and_then(|status| status.code());
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(_) => {
                killed_group = true;
                kill_process_group(child.id());
                let _ = child.kill();
                break child.wait().ok().and_then(|status| status.code());
            }
        }
    };

    // 正常退出时读取线程会因管道 EOF 自行结束；只有强制杀死进程组后才使用
    // 有界等待，避免个别后代仍持有管道时卡住调用方。
    let join_timeout = killed_group.then_some(READER_JOIN_TIMEOUT);
    let (stdout, _) = join_reader(stdout_reader, join_timeout);
    let (stderr, _) = join_reader(stderr_reader, join_timeout);

    CommandOutcome {
        exit_code,
        duration_ms: elapsed_ms(started),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        timed_out,
    }
}

/// 收集 base 到 HEAD 的文件清单，以及未提交（工作区/暂存区）的改动。
///
/// 文件清单来自 `git diff --name-status <base>..HEAD` 与 `git diff --name-status <base>`，
/// 按路径去重（保留先出现者）。补丁只取 `git diff <base>..HEAD`，最多 [`PATCH_CAP`] 字节。
pub fn collect_diff(repo: &Path, base_sha: &str) -> Result<DiffArtifact, String> {
    let base_sha = base_sha.trim();
    if base_sha.is_empty() {
        return Err("base_sha 为空".to_string());
    }

    let committed_range = format!("{base_sha}..HEAD");
    let (committed, _) = run_git(
        repo,
        &["diff", "--name-status", committed_range.as_str()],
        NAME_STATUS_CAP,
    )?;
    let (uncommitted, _) = run_git(repo, &["diff", "--name-status", base_sha], NAME_STATUS_CAP)?;
    let (patch_bytes, truncated) = run_git(repo, &["diff", committed_range.as_str()], PATCH_CAP)?;

    let mut files = parse_name_status(&String::from_utf8_lossy(&committed));
    let mut seen: HashSet<String> = files.iter().map(|file| file.path.clone()).collect();
    for change in parse_name_status(&String::from_utf8_lossy(&uncommitted)) {
        if seen.insert(change.path.clone()) {
            files.push(change);
        }
    }

    Ok(DiffArtifact {
        base_sha: base_sha.to_string(),
        files,
        patch: String::from_utf8_lossy(&patch_bytes).into_owned(),
        truncated,
    })
}

/// 对改动文件做风险分类：删除、依赖声明、数据库迁移、CI 配置、敏感信息。
///
/// 同一文件可产生多个标记，顺序固定为 deleted → dependency → migration → ci → secret。
pub fn classify_risk(files: &[FileChange]) -> Vec<RiskFlag> {
    let mut flags = Vec::new();
    for file in files {
        let normalized = file.path.replace('\\', "/");
        let lower = normalized.to_ascii_lowercase();
        let name = lower.rsplit('/').next().unwrap_or("");

        if file.status == 'D' {
            flags.push(RiskFlag {
                path: normalized.clone(),
                kind: RiskKind::Deleted,
            });
        }
        if is_dependency_file(name) {
            flags.push(RiskFlag {
                path: normalized.clone(),
                kind: RiskKind::Dependency,
            });
        }
        if lower.split('/').any(|segment| segment == "migrations") {
            flags.push(RiskFlag {
                path: normalized.clone(),
                kind: RiskKind::Migration,
            });
        }
        if is_ci_path(&lower) {
            flags.push(RiskFlag {
                path: normalized.clone(),
                kind: RiskKind::Ci,
            });
        }
        if is_secret_path(&lower) {
            flags.push(RiskFlag {
                path: normalized.clone(),
                kind: RiskKind::Secret,
            });
        }
    }
    flags
}

/// 筛出测试相关文件（`tests/`、`test/`、`__tests__/`、`*_test.*`、`*.test.*`、`*_spec.*`）
/// 与 CI 配置文件路径，保持输入顺序并按路径去重。
pub fn test_ci_files(files: &[FileChange]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    for file in files {
        let normalized = file.path.replace('\\', "/");
        if !is_test_path(&normalized) && !is_ci_path(&normalized.to_ascii_lowercase()) {
            continue;
        }
        if seen.insert(normalized.clone()) {
            paths.push(normalized);
        }
    }
    paths
}

fn push_command(
    commands: &mut Vec<DiscoveredCommand>,
    seen: &mut HashSet<String>,
    cmd: String,
    kind: CmdKind,
    source: &str,
) {
    if seen.insert(cmd.clone()) {
        commands.push(DiscoveredCommand {
            cmd,
            kind,
            source: source.to_string(),
        });
    }
}

/// 读取 `package.json` 中 `scripts` 的键集合；文件缺失或格式非法时返回 `None`。
fn package_scripts(path: &Path) -> Option<HashSet<String>> {
    let text = fs::read_to_string(path).ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&text).ok()?;
    let scripts = manifest.get("scripts")?.as_object()?;
    Some(scripts.keys().cloned().collect())
}

/// 解析 Makefile 中的目标名：只认行首（无缩进）、`name:` 形式且不是变量赋值。
fn parse_make_targets(content: &str) -> Vec<String> {
    let mut targets: Vec<String> = Vec::new();
    for line in content.lines() {
        if line.starts_with('#') || line.starts_with('\t') || line.starts_with(' ') {
            continue;
        }
        let Some(colon) = line.find(':') else {
            continue;
        };
        let name = line[..colon].trim();
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            continue;
        }
        let rest = &line[colon + 1..];
        // `name := value` / `name::= value` 是变量赋值，不是目标。
        if rest.starts_with('=') || rest.starts_with(":=") {
            continue;
        }
        if !targets.iter().any(|existing| existing == name) {
            targets.push(name.to_string());
        }
    }
    targets
}

/// clippy 是否被项目显式启用：存在 `.clippy.toml`/`clippy.toml`，或 CI 配置提到 clippy。
fn clippy_configured(repo: &Path) -> bool {
    if repo.join(".clippy.toml").is_file() || repo.join("clippy.toml").is_file() {
        return true;
    }
    ci_files(repo).iter().any(|path| {
        fs::read_to_string(path)
            .map(|text| text.to_ascii_lowercase().contains("clippy"))
            .unwrap_or(false)
    })
}

fn ci_files(repo: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(repo.join(".github").join("workflows")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("yml" | "yaml")
            ) {
                files.push(path);
            }
        }
    }
    for name in [".gitlab-ci.yml", ".gitlab-ci.yaml", "Jenkinsfile"] {
        let path = repo.join(name);
        if path.is_file() {
            files.push(path);
        }
    }
    files
}

/// 启动一个读取线程，读空管道并只保留前 `cap` 字节。
fn spawn_reader<R>(mut reader: R, cap: usize) -> std::thread::JoinHandle<(Vec<u8>, bool)>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || read_capped(&mut reader, cap))
}

fn read_capped<R: Read>(reader: &mut R, cap: usize) -> (Vec<u8>, bool) {
    let mut captured = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut truncated = false;
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                let room = cap.saturating_sub(captured.len());
                let take = room.min(read);
                captured.extend_from_slice(&buffer[..take]);
                if take < read {
                    truncated = true;
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            // 管道被关闭（如超时杀死子进程）时保留已读到的内容。
            Err(_) => break,
        }
    }
    (captured, truncated)
}

fn join_reader(
    handle: Option<std::thread::JoinHandle<(Vec<u8>, bool)>>,
    timeout: Option<Duration>,
) -> (Vec<u8>, bool) {
    let Some(handle) = handle else {
        return (Vec::new(), false);
    };

    if let Some(timeout) = timeout {
        let deadline = Instant::now() + timeout;
        while !handle.is_finished() {
            if Instant::now() >= deadline {
                // 放弃等待并 detach：读取线程会在管道最终关闭后自行结束，
                // 但超时路径不再被它阻塞。
                return (Vec::new(), false);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    handle.join().unwrap_or_else(|_| (Vec::new(), false))
}

/// 向进程组发送 SIGKILL。子进程通过 `process_group(0)` 成为新进程组组长，
/// 其 pid 即 pgid；`kill` 的负 pid 参数表示对整个进程组发信号。
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

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

fn run_git(repo: &Path, args: &[&str], cap: usize) -> Result<(Vec<u8>, bool), String> {
    let mut child = Command::new("git")
        .arg("-c")
        .arg("core.quotepath=false")
        .args(args)
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("无法启动 git: {err}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "git 标准输出不可用".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "git 标准错误不可用".to_string())?;
    let stdout_reader = spawn_reader(stdout, cap);
    let stderr_reader = spawn_reader(stderr, OUTPUT_CAP);

    let status = child
        .wait()
        .map_err(|err| format!("等待 git 进程失败: {err}"))?;
    let (stdout, truncated) = join_reader(Some(stdout_reader), None);
    let (stderr, _) = join_reader(Some(stderr_reader), None);

    if !status.success() {
        let message = String::from_utf8_lossy(&stderr).trim().to_string();
        return Err(if message.is_empty() {
            format!("git {} 失败（退出码 {:?}）", args.join(" "), status.code())
        } else {
            format!("git {} 失败: {message}", args.join(" "))
        });
    }

    Ok((stdout, truncated))
}

/// 解析 `git diff --name-status` 输出；重命名/复制取新路径，状态取首字母。
fn parse_name_status(output: &str) -> Vec<FileChange> {
    let mut files = Vec::new();
    for line in output.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 2 {
            continue;
        }
        let Some(status) = fields[0].chars().next() else {
            continue;
        };
        let path = if matches!(status, 'R' | 'C') && fields.len() >= 3 {
            fields[2..].join("\t")
        } else {
            fields[1..].join("\t")
        };
        let path = path.trim();
        if path.is_empty() || path == "/dev/null" {
            continue;
        }
        files.push(FileChange {
            path: path.to_string(),
            status,
        });
    }
    files
}

fn is_dependency_file(name: &str) -> bool {
    matches!(
        name,
        "cargo.toml"
            | "cargo.lock"
            | "package.json"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "bun.lockb"
            | "bun.lock"
            | "pyproject.toml"
            | "poetry.lock"
            | "pipfile"
            | "pipfile.lock"
            | "go.mod"
            | "go.sum"
            | "gemfile"
            | "gemfile.lock"
            | "composer.json"
            | "composer.lock"
            | "requirements.txt"
    ) || name.ends_with(".lock")
}

fn is_ci_path(lower: &str) -> bool {
    lower == ".github/workflows"
        || lower.starts_with(".github/workflows/")
        || lower.starts_with(".gitlab-ci")
        || lower == "jenkinsfile"
        || lower.ends_with("/jenkinsfile")
}

fn is_secret_path(lower: &str) -> bool {
    lower.contains("secret")
        || lower.contains(".env")
        || lower.contains("auth")
        || lower.contains("credential")
}

fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower
        .split('/')
        .any(|segment| matches!(segment, "tests" | "test" | "__tests__"))
    {
        return true;
    }
    let name = lower.rsplit('/').next().unwrap_or("");
    name.contains("_test.") || name.contains(".test.") || name.contains("_spec.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "temli-evidence-{}-{unique}-{label}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("创建临时目录");
            Self { path }
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.path.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("创建临时子目录");
            }
            fs::write(&path, contents).expect("写入临时文件");
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn command_list(commands: &[DiscoveredCommand]) -> Vec<&str> {
        commands
            .iter()
            .map(|command| command.cmd.as_str())
            .collect()
    }

    fn change(path: &str, status: char) -> FileChange {
        FileChange {
            path: path.to_string(),
            status,
        }
    }

    #[test]
    fn discovers_package_json_scripts() {
        let dir = TempDir::new("package-scripts");
        dir.write(
            "package.json",
            r#"{
                "name": "demo",
                "scripts": {
                    "build": "tsc -p .",
                    "lint": "eslint .",
                    "test": "jest",
                    "test:unit": "jest unit",
                    "dev": "vite"
                }
            }"#,
        );

        let commands = discover_commands(&dir.path);
        assert_eq!(
            command_list(&commands),
            vec![
                "npm run test",
                "npm run test:unit",
                "npm run build",
                "npm run lint"
            ]
        );
        assert_eq!(
            commands
                .iter()
                .map(|command| command.kind)
                .collect::<Vec<_>>(),
            vec![CmdKind::Test, CmdKind::Test, CmdKind::Build, CmdKind::Lint]
        );
        assert!(
            commands
                .iter()
                .all(|command| command.source == "package.json")
        );
    }

    #[test]
    fn discovers_makefile_targets_without_duplicates() {
        let dir = TempDir::new("makefile-targets");
        dir.write(
            "Makefile",
            concat!(
                "# 注释行不算目标\n",
                "build: deps\n",
                "\tcargo build\n",
                "\n",
                "test: build\n",
                "\tcargo test\n",
                "\n",
                "test := ignored-variable\n",
                "\n",
                "lint:\n",
                "\tsh scripts/lint.sh\n",
                "\n",
                "test:\n",
                "\techo duplicate-target\n",
                "\n",
                "other:\n",
                "\techo not-a-candidate\n"
            ),
        );

        let commands = discover_commands(&dir.path);
        assert_eq!(
            command_list(&commands),
            vec!["make test", "make build", "make lint"]
        );
        assert_eq!(commands[0].source, "Makefile");
        assert_eq!(commands[0].kind, CmdKind::Test);
    }

    #[test]
    fn cargo_commands_include_clippy_only_when_configured() {
        let plain = TempDir::new("cargo-plain");
        plain.write("Cargo.toml", "[package]\nname = \"demo\"\n");
        assert_eq!(
            command_list(&discover_commands(&plain.path)),
            vec!["cargo test", "cargo build"]
        );

        let configured = TempDir::new("cargo-clippy-toml");
        configured.write("Cargo.toml", "[package]\nname = \"demo\"\n");
        configured.write(".clippy.toml", "avoid-breaking-exported-api = false\n");
        let commands = discover_commands(&configured.path);
        assert_eq!(
            command_list(&commands),
            vec!["cargo test", "cargo build", "cargo clippy"]
        );
        assert_eq!(commands[2].kind, CmdKind::Lint);

        let ci = TempDir::new("cargo-clippy-ci");
        ci.write("Cargo.toml", "[package]\nname = \"demo\"\n");
        ci.write(
            ".github/workflows/ci.yml",
            "jobs:\n  lint:\n    steps:\n      - run: cargo clippy --all-targets\n",
        );
        assert!(
            command_list(&discover_commands(&ci.path)).contains(&"cargo clippy"),
            "CI 提到 clippy 时应追加 cargo clippy"
        );
    }

    #[test]
    fn default_acceptance_ids_are_unique_and_stable() {
        let commands = vec![
            DiscoveredCommand {
                cmd: "npm run test".to_string(),
                kind: CmdKind::Test,
                source: "package.json".to_string(),
            },
            DiscoveredCommand {
                cmd: "cargo build".to_string(),
                kind: CmdKind::Build,
                source: "Cargo.toml".to_string(),
            },
            DiscoveredCommand {
                cmd: "make lint".to_string(),
                kind: CmdKind::Lint,
                source: "Makefile".to_string(),
            },
        ];

        let criteria = default_acceptance(&commands);
        assert_eq!(
            criteria
                .iter()
                .map(|criterion| criterion.id.as_str())
                .collect::<Vec<_>>(),
            vec!["c-1", "c-2", "c-3"]
        );
        assert_eq!(
            criteria
                .iter()
                .map(|criterion| criterion.cmd.as_str())
                .collect::<Vec<_>>(),
            vec!["npm run test", "cargo build", "make lint"]
        );
        assert!(
            criteria
                .iter()
                .all(|criterion| criterion.expect_exit == 0 && criterion.expect_match.is_none())
        );
        let ids: HashSet<&str> = criteria
            .iter()
            .map(|criterion| criterion.id.as_str())
            .collect();
        assert_eq!(ids.len(), criteria.len());

        // 同一输入重复生成结果一致。
        assert_eq!(default_acceptance(&commands), criteria);
    }

    #[test]
    fn run_command_reports_success() {
        let dir = TempDir::new("run-success");
        let outcome = run_command(&dir.path, "echo hello", 10);

        assert_eq!(outcome.exit_code, Some(0));
        assert!(!outcome.timed_out);
        assert_eq!(outcome.stdout.trim(), "hello");
        assert!(outcome.stderr.is_empty());
    }

    #[test]
    fn run_command_reports_failure_exit_code() {
        let dir = TempDir::new("run-failure");
        let outcome = run_command(&dir.path, "echo oops 1>&2; exit 3", 10);

        assert_eq!(outcome.exit_code, Some(3));
        assert!(!outcome.timed_out);
        assert!(outcome.stderr.contains("oops"));
    }

    #[test]
    fn run_command_times_out_and_kills_child() {
        let dir = TempDir::new("run-timeout");
        let started = Instant::now();
        let outcome = run_command(&dir.path, "sleep 5", 1);
        let elapsed = started.elapsed();

        assert!(outcome.timed_out, "应标记超时");
        assert!(
            outcome.duration_ms >= 800,
            "应至少等待接近 1 秒，实际 {}ms",
            outcome.duration_ms
        );
        assert!(
            elapsed < Duration::from_secs(4),
            "超时后应尽快杀死子进程，实际耗时 {elapsed:?}"
        );
    }

    #[test]
    fn run_command_timeout_kills_descendants_in_process_group() {
        let dir = TempDir::new("run-timeout-group");
        let started = Instant::now();
        let outcome = run_command(&dir.path, "sleep 30 & echo $!; wait", 1);
        let elapsed = started.elapsed();

        assert!(outcome.timed_out, "应标记超时");
        assert!(
            elapsed < Duration::from_secs(3),
            "超时后应在约 2 秒内返回，实际耗时 {elapsed:?}"
        );

        let pid = outcome
            .stdout
            .lines()
            .next()
            .map(str::trim)
            .filter(|pid| !pid.is_empty())
            .expect("应输出后台进程 pid");

        std::thread::sleep(Duration::from_millis(250));
        let stat = Command::new("ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .expect("运行 ps");
        let state = String::from_utf8_lossy(&stat.stdout).trim().to_string();
        assert!(
            state.is_empty() || state.starts_with('Z'),
            "后代进程 {pid} 应已被杀死，当前状态：{state:?}"
        );
    }

    #[test]
    fn run_command_caps_captured_output() {
        let dir = TempDir::new("run-output-cap");
        let outcome = run_command(
            &dir.path,
            r#"awk 'BEGIN { for (i = 0; i < 8000; i++) print "0123456789" }'"#,
            10,
        );

        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(outcome.stdout.len(), OUTPUT_CAP);
    }

    #[test]
    fn classifies_risk_flags() {
        let files = vec![
            change("src/old.rs", 'D'),
            change("yarn.lock", 'D'),
            change("package.json", 'M'),
            change("Cargo.lock", 'M'),
            change("pyproject.toml", 'M'),
            change("db/migrations/0001_init.sql", 'A'),
            change(".github/workflows/ci.yml", 'M'),
            change("Jenkinsfile", 'M'),
            change(".gitlab-ci.yml", 'M'),
            change("deploy/.env.production", 'A'),
            change("src/auth/session.rs", 'M'),
            change("docs/credentials.md", 'M'),
            change("src/main.rs", 'M'),
        ];

        let flags = classify_risk(&files);
        let kinds_for = |path: &str| -> Vec<RiskKind> {
            flags
                .iter()
                .filter(|flag| flag.path == path)
                .map(|flag| flag.kind)
                .collect()
        };

        assert_eq!(kinds_for("src/old.rs"), vec![RiskKind::Deleted]);
        assert_eq!(
            kinds_for("yarn.lock"),
            vec![RiskKind::Deleted, RiskKind::Dependency]
        );
        assert_eq!(kinds_for("package.json"), vec![RiskKind::Dependency]);
        assert_eq!(kinds_for("Cargo.lock"), vec![RiskKind::Dependency]);
        assert_eq!(kinds_for("pyproject.toml"), vec![RiskKind::Dependency]);
        assert_eq!(
            kinds_for("db/migrations/0001_init.sql"),
            vec![RiskKind::Migration]
        );
        assert_eq!(kinds_for(".github/workflows/ci.yml"), vec![RiskKind::Ci]);
        assert_eq!(kinds_for("Jenkinsfile"), vec![RiskKind::Ci]);
        assert_eq!(kinds_for(".gitlab-ci.yml"), vec![RiskKind::Ci]);
        assert_eq!(kinds_for("deploy/.env.production"), vec![RiskKind::Secret]);
        assert_eq!(kinds_for("src/auth/session.rs"), vec![RiskKind::Secret]);
        assert_eq!(kinds_for("docs/credentials.md"), vec![RiskKind::Secret]);
        assert!(kinds_for("src/main.rs").is_empty());
    }

    #[test]
    fn test_ci_files_matches_test_and_ci_paths_only() {
        let files = vec![
            change("tests/integration.rs", 'M'),
            change("src/test/helper.rs", 'M'),
            change("src/__tests__/app.test.tsx", 'A'),
            change("src/app.test.tsx", 'M'),
            change("src/app_test.rs", 'M'),
            change("spec/thing_spec.rb", 'M'),
            change("contest/main.rs", 'M'),
            change("src/main.rs", 'M'),
            change(".github/workflows/ci.yml", 'M'),
        ];

        assert_eq!(
            test_ci_files(&files),
            vec![
                "tests/integration.rs",
                "src/test/helper.rs",
                "src/__tests__/app.test.tsx",
                "src/app.test.tsx",
                "src/app_test.rs",
                "spec/thing_spec.rb",
                ".github/workflows/ci.yml",
            ]
        );
    }

    #[test]
    fn parses_git_name_status_lines() {
        let parsed = parse_name_status(
            "M\tsrc/lib.rs\nA\tREADME.md\nR100\told/name.txt\tnew/name.txt\nD\tgone.txt\n",
        );

        assert_eq!(
            parsed,
            vec![
                FileChange {
                    path: "src/lib.rs".to_string(),
                    status: 'M',
                },
                FileChange {
                    path: "README.md".to_string(),
                    status: 'A',
                },
                FileChange {
                    path: "new/name.txt".to_string(),
                    status: 'R',
                },
                FileChange {
                    path: "gone.txt".to_string(),
                    status: 'D',
                },
            ]
        );
    }

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn git_ok(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("运行 git");
        assert!(status.success(), "git {args:?} 失败");
    }

    fn git_stdout(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("运行 git");
        assert!(output.status.success(), "git {args:?} 失败");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn collects_committed_and_uncommitted_changes() {
        if !git_available() {
            return;
        }
        let dir = TempDir::new("diff");
        git_ok(&dir.path, &["init", "-q"]);
        dir.write("src/main.rs", "fn main() {}\n");
        dir.write("old.txt", "old\n");
        git_ok(&dir.path, &["add", "-A"]);
        git_ok(
            &dir.path,
            &[
                "-c",
                "user.name=temli",
                "-c",
                "user.email=temli@example.com",
                "commit",
                "-q",
                "-m",
                "init",
            ],
        );
        let base = git_stdout(&dir.path, &["rev-parse", "HEAD"]);

        dir.write("committed.txt", "committed\n");
        git_ok(&dir.path, &["add", "committed.txt"]);
        git_ok(
            &dir.path,
            &[
                "-c",
                "user.name=temli",
                "-c",
                "user.email=temli@example.com",
                "commit",
                "-q",
                "-m",
                "second",
            ],
        );

        dir.write("src/main.rs", "fn main() { println!(\"hi\"); }\n");
        dir.write("new.txt", "new\n");
        git_ok(&dir.path, &["add", "new.txt"]);

        let artifact = collect_diff(&dir.path, &base).expect("收集 diff");
        let status_of = |path: &str| {
            artifact
                .files
                .iter()
                .find(|file| file.path == path)
                .map(|file| file.status)
        };

        assert_eq!(artifact.base_sha, base);
        assert_eq!(status_of("committed.txt"), Some('A'));
        assert_eq!(status_of("src/main.rs"), Some('M'));
        assert_eq!(status_of("new.txt"), Some('A'));
        assert_eq!(
            artifact
                .files
                .iter()
                .filter(|file| file.path == "committed.txt")
                .count(),
            1,
            "已提交与未提交清单应去重"
        );
        assert!(
            matches!(collect_diff(&dir.path, "  "), Err(message) if message == "base_sha 为空"),
            "空 base_sha 应返回错误"
        );
    }
}
