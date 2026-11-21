//! 给界面看的错误（2026-11-21 起）：命令出错时不直接返回中文句子，而是返回语言文件里的键名和参数，
//! 界面（ui/js/api.js 的 `invoke`）按当前语言翻译。原来报错写死在这里，英文界面下也是中文，措辞也偏口语。
//!
//! 格式：`@键名` 或 `@键名 {JSON 参数}`，例如 `@err.fileMissing {"path":"C:\\…"}`。
//! 只写进日志、不给用户看的文字不用这个。

use serde_json::{Value, json};

/// 没有参数的错误
pub fn key(key: &str) -> String {
    format!("@{key}")
}

/// 带参数的错误（参数名和语言文件里的 `{占位符}` 对应）
pub fn with(key: &str, params: Value) -> String {
    format!("@{key} {params}")
}

/// 带一段技术细节（文件路径、系统报错原文）的错误，细节放在 `{detail}`
pub fn detail(key: &str, detail: impl std::fmt::Display) -> String {
    with(key, json!({ "detail": detail.to_string() }))
}
