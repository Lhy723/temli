//! 轻量持久日志：写入 `<data_dir>/logs/temli.log`。
//!
//! 单个日志文件达到 1 MiB 后整体轮换为 `temli.log.1`（只保留一份备份）。
//! 所有写入失败都被静默忽略：日志是旁路能力，绝不能影响主流程。

use std::any::Any;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once, OnceLock, TryLockError};
use std::time::{SystemTime, UNIX_EPOCH};

/// 单个日志文件的字节上限；下一次写入会超出时先轮换。
const MAX_BYTES: u64 = 1024 * 1024;

/// 日志文件名。
const FILE_NAME: &str = "temli.log";

struct Sink {
    path: PathBuf,
    file: File,
}

static SINK: OnceLock<Mutex<Option<Sink>>> = OnceLock::new();
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
static PANIC_HOOK: Once = Once::new();

/// 初始化日志文件并安装 panic hook；重复调用只有第一次生效。
///
/// 目录创建或文件打开失败时静默降级（日志停用），但仍安装 panic hook，
/// 保证默认的 stderr 行为不受影响。
pub fn init(data_dir: &Path) {
    let path = data_dir.join("logs").join(FILE_NAME);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = LOG_PATH.set(path.clone());
    if let Ok(file) = open_log(&path) {
        let _ = SINK.set(Mutex::new(Some(Sink { path, file })));
    }
    PANIC_HOOK.call_once(install_panic_hook);
}

/// 日志文件的绝对路径；[`init`] 之前返回 `None`。
pub fn path() -> Option<&'static Path> {
    LOG_PATH.get().map(PathBuf::as_path)
}

/// 追加一行日志；任何 I/O 失败都不会向上传播。
pub fn log(msg: impl AsRef<str>) {
    let line = format!("[{}] {}\n", timestamp(), msg.as_ref());
    write_line(&line);
}

fn write_line(line: &str) {
    let Some(lock) = SINK.get() else {
        // 没有可用的写入句柄（初始化失败）时仍尝试直接追加一次。
        if let Some(path) = LOG_PATH.get() {
            append_direct(path, line);
        }
        return;
    };
    match lock.try_lock() {
        Ok(mut guard) => write_sink(&mut guard, line),
        Err(TryLockError::Poisoned(poisoned)) => {
            write_sink(&mut poisoned.into_inner(), line);
        }
        Err(TryLockError::WouldBlock) => {
            // 另一个线程正在写；panic hook 可能就处在这条路径上，
            // 因此不等待锁，直接用独立的追加句柄写入。
            if let Some(path) = LOG_PATH.get() {
                append_direct(path, line);
            }
        }
    }
}

fn write_sink(sink: &mut Option<Sink>, line: &str) {
    let Some(sink) = sink.as_mut() else {
        return;
    };
    let current = sink.file.metadata().map(|meta| meta.len()).unwrap_or(0);
    if current > 0 && current + line.len() as u64 > MAX_BYTES {
        rotate(sink);
    }
    let _ = sink.file.write_all(line.as_bytes());
    let _ = sink.file.flush();
}

/// 把 `temli.log` 重命名为 `temli.log.1`（覆盖旧备份）并重新打开新文件。
fn rotate(sink: &mut Sink) {
    let backup = rotated_path(&sink.path);
    let _ = fs::remove_file(&backup);
    let _ = fs::rename(&sink.path, &backup);
    match open_log(&sink.path) {
        Ok(file) => sink.file = file,
        // 新文件打不开时退回备份文件，日志不中断。
        Err(_) => {
            if let Ok(file) = OpenOptions::new().create(true).append(true).open(&backup) {
                sink.file = file;
            }
        }
    }
}

fn open_log(path: &Path) -> std::io::Result<File> {
    if fs::metadata(path).map(|meta| meta.len()).unwrap_or(0) > MAX_BYTES {
        let backup = rotated_path(path);
        let _ = fs::remove_file(&backup);
        let _ = fs::rename(path, &backup);
    }
    OpenOptions::new().create(true).append(true).open(path)
}

fn append_direct(path: &Path, line: &str) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

fn rotated_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".1");
    PathBuf::from(name)
}

/// 安装 panic hook：先把 panic 信息（含位置）写入日志，再执行原有 hook，
/// 保留默认的 stderr 输出。
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
            .unwrap_or_else(|| "未知位置".to_string());
        log(format!(
            "panic at {location}: {}",
            payload_message(info.payload())
        ));
        previous(info);
    }));
}

fn payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "<非字符串 panic payload>".to_string()
    }
}

/// UTC 时间戳，形如 `2026-11-09T06:12:33Z`。
fn timestamp() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    )
}

/// 把 1970-01-01 起的天数转换为公历年月日（Howard Hinnant 的 civil_from_days）。
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
        assert_eq!(civil_from_days(20_833), (2027, 1, 15));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn rotated_path_appends_single_backup_suffix() {
        assert_eq!(
            rotated_path(Path::new("/tmp/temli.log")),
            PathBuf::from("/tmp/temli.log.1")
        );
    }
}
