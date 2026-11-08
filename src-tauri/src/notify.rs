//! 系统通知：任务进入终态或需要人工决策时提醒用户。
//!
//! 通知是尽力而为的：`init` 之前静默跳过，权限被拒绝或平台不支持时只记录
//! 错误，绝不影响调度与运行状态机。

use std::sync::OnceLock;

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

static APP: OnceLock<AppHandle> = OnceLock::new();

/// 通知正文里任务标题最多保留的字符数（含省略号）。
const MAX_TITLE_CHARS: usize = 80;
/// 通知正文里决策原因最多保留的字符数（含省略号）。
const MAX_REASON_CHARS: usize = 160;

/// 保存主窗口的 `AppHandle`，之后后台线程用它发送系统通知。
///
/// 重复调用（例如窗口重建）沿用首个句柄即可。
pub fn init(app: AppHandle) {
    let _ = APP.set(app);
}

/// 任务进入终态（合并 / 需要决策 / 失败 / 中止）后通知用户。
pub fn task_finished(title: &str, status: &str) {
    send(status_title(status), &task_body(title, status));
}

/// 打开人工决策（Agent 审批等）后提醒用户处理。
pub fn decision(reason: &str) {
    send("需要决策", &decision_body(reason));
}

fn send(title: &str, body: &str) {
    let Some(app) = APP.get() else {
        return;
    };
    if let Err(err) = app.notification().builder().title(title).body(body).show() {
        // 用户拒绝通知权限、平台不支持等情况都不应影响任务流程。
        eprintln!("发送系统通知失败：{err}");
    }
}

/// 终态 → 通知标题；未知状态按失败处理。
fn status_title(status: &str) -> &'static str {
    match status {
        "merged" => "任务完成",
        "needs_human" => "需要决策",
        "interrupted" => "任务已中止",
        _ => "任务失败",
    }
}

/// 终态 → 正文里的中文状态说明；未知状态原样展示。
fn status_label(status: &str) -> &str {
    match status {
        "merged" => "已合并",
        "needs_human" => "等待人工决策",
        "failed" => "失败",
        "interrupted" => "已中止",
        other => other,
    }
}

fn task_body(title: &str, status: &str) -> String {
    let title = single_line(title);
    let title = if title.is_empty() {
        "未命名任务".to_string()
    } else {
        truncate_chars(&title, MAX_TITLE_CHARS)
    };
    format!("{title}\n状态：{}", status_label(status))
}

fn decision_body(reason: &str) -> String {
    let reason = single_line(reason);
    if reason.is_empty() {
        "请到决策收件箱处理。".to_string()
    } else {
        format!(
            "{}\n请到决策收件箱处理。",
            truncate_chars(&reason, MAX_REASON_CHARS)
        )
    }
}

/// 把多行文本压成单行：通知正文按行折行，多余空白只会让内容更难读。
fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 按字符（而不是字节）截断，超长时用省略号收尾。
fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_statuses_map_to_titles() {
        assert_eq!(status_title("merged"), "任务完成");
        assert_eq!(status_title("needs_human"), "需要决策");
        assert_eq!(status_title("failed"), "任务失败");
        assert_eq!(status_title("interrupted"), "任务已中止");
        assert_eq!(status_title("unexpected"), "任务失败");
    }

    #[test]
    fn task_body_includes_title_and_status() {
        let body = task_body("修复登录崩溃", "merged");
        assert!(body.contains("修复登录崩溃"), "body: {body}");
        assert!(body.contains("已合并"), "body: {body}");

        let body = task_body("修复登录崩溃", "needs_human");
        assert!(body.contains("等待人工决策"), "body: {body}");
    }

    #[test]
    fn empty_title_falls_back_to_placeholder() {
        let body = task_body("   ", "failed");
        assert!(body.starts_with("未命名任务"), "body: {body}");
    }

    #[test]
    fn short_text_is_kept_whole() {
        assert_eq!(truncate_chars("短标题", 10), "短标题");
        let exact = "标".repeat(10);
        assert_eq!(truncate_chars(&exact, 10), exact);
    }

    #[test]
    fn long_titles_are_truncated_by_chars() {
        let long = "长".repeat(MAX_TITLE_CHARS * 2);
        let body = task_body(&long, "merged");
        let first_line = body.lines().next().expect("title line");
        assert_eq!(first_line.chars().count(), MAX_TITLE_CHARS);
        assert!(first_line.ends_with('…'), "title: {first_line}");
        assert!(first_line.chars().all(|c| c == '长' || c == '…'));
    }

    #[test]
    fn long_reasons_collapse_to_one_line_and_truncate() {
        let reason = format!("{}\n\n{}", "执行命令".repeat(40), "需要确认".repeat(40));
        let body = decision_body(&reason);
        let first_line = body.lines().next().expect("reason line");
        assert_eq!(first_line.chars().count(), MAX_REASON_CHARS);
        assert!(first_line.ends_with('…'), "reason: {first_line}");
        assert!(!first_line.contains('\n'));
    }

    #[test]
    fn empty_reason_still_has_action_hint() {
        assert_eq!(decision_body(" \n "), "请到决策收件箱处理。");
    }

    #[test]
    fn sending_before_init_is_a_no_op() {
        // 测试进程没有 Tauri 运行时：未初始化时必须静默返回而不是 panic。
        task_finished("修复登录崩溃", "merged");
        task_finished("修复登录崩溃", "failed");
        decision("执行命令");
    }
}
