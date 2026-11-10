//! 命令层类型化错误。
//!
//! 内部 store / orchestrator 仍然返回 `Result<T, String>`；把错误提升到 Tauri
//! IPC 边界时统一包成 [`AppError`]，前端即可按 `code` 分支、展示 `message`。
//! 为兼容旧测试与 `err.contains(..)` 一类字符串断言，`AppError` 同时实现了
//! `Deref<Target = str>` 与和字符串的比较。

use serde::Serialize;
use std::fmt;
use std::ops::Deref;

/// 命令返回给前端的错误；序列化为 `{ "code": ..., "message": ... }`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppError {
    /// 稳定的错误分类，供前端分支；不要放动态内容。
    pub code: String,
    /// 面向用户的错误提示。
    pub message: String,
}

impl AppError {
    /// 用显式分类与提示构造错误。
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    /// 参数或输入不合法。
    pub fn validation(message: impl Into<String>) -> Self {
        Self::new("validation", message)
    }

    /// 当前状态与操作冲突（重复创建、状态不允许等）。
    #[allow(dead_code)] // 分类已按约定保留，命令层暂无可稳定识别的冲突点。
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new("conflict", message)
    }

    /// 目标实体不存在。
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("not_found", message)
    }

    /// 未分类的内部错误。
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("internal", message)
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}

impl From<String> for AppError {
    fn from(message: String) -> Self {
        Self::internal(message)
    }
}

impl From<&str> for AppError {
    fn from(message: &str) -> Self {
        Self::internal(message)
    }
}

impl Deref for AppError {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.message
    }
}

impl PartialEq<&str> for AppError {
    fn eq(&self, other: &&str) -> bool {
        self.message == *other
    }
}

impl PartialEq<String> for AppError {
    fn eq(&self, other: &String) -> bool {
        self.message == *other
    }
}

/// 命令层统一结果类型。
pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_set_stable_codes() {
        assert_eq!(AppError::validation("x").code, "validation");
        assert_eq!(AppError::conflict("x").code, "conflict");
        assert_eq!(AppError::not_found("x").code, "not_found");
        assert_eq!(AppError::internal("x").code, "internal");
        assert_eq!(AppError::new("custom", "x").code, "custom");
    }

    #[test]
    fn string_errors_become_internal_errors() {
        let from_owned = AppError::from("磁盘写入失败".to_string());
        assert_eq!(from_owned.code, "internal");
        assert_eq!(from_owned.message, "磁盘写入失败");

        let from_borrowed = AppError::from("磁盘写入失败");
        assert_eq!(from_borrowed, from_owned);
    }

    #[test]
    fn displays_and_derefs_to_message() {
        let error = AppError::validation("标题不能为空");
        assert_eq!(error.to_string(), "标题不能为空");
        assert!(error.contains("不能为空"));
        assert!(error.starts_with("标题"));

        let as_str: &str = &error;
        assert_eq!(as_str, "标题不能为空");
    }

    #[test]
    fn compares_with_string_slices() {
        let error = AppError::validation("条目不存在：t-1");
        assert_eq!(error, "条目不存在：t-1");
        assert_eq!(error, "条目不存在：t-1".to_string());
        assert_ne!(error, "其它错误");
    }

    #[test]
    fn serializes_code_and_message() {
        let error = AppError::not_found("任务不存在：t-1");
        let value = serde_json::to_value(&error).expect("序列化 AppError");
        assert_eq!(
            value,
            serde_json::json!({ "code": "not_found", "message": "任务不存在：t-1" })
        );
    }
}
