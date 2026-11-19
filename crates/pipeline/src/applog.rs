//! 应用日志（2026-11-19 起）：录制、播放、应用本身都写进同一个文件 `d2hr.log`，一行一件事，按时间排。
//!
//! 原来每次录制单独一个 `record-*.log`，播放什么都不记，卡顿当场没抓到就查不了。用户定：全记在一个文件里，
//! 查问题时按时间就能对上（比如卡顿那一刻是不是正在生成缩略图）。
//! - 一行：`2026-11-19 10:21:15.3  录制  内容`；内容里的换行换成 ` | `，保证一件事一行；
//! - 超过 `MAX_BYTES` 就改名成 `d2hr.old.log`（覆盖上一份）再开新的，最多占两份的大小；
//! - 没调过 `init`（命令行）就什么都不写。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// 一份日志的上限，超过就轮换
const MAX_BYTES: u64 = 5 * 1024 * 1024;
const NAME: &str = "d2hr.log";
const OLD_NAME: &str = "d2hr.old.log";

struct Log {
    dir: PathBuf,
    file: Option<File>,
}

static LOG: Mutex<Option<Log>> = Mutex::new(None);

/// 日志写到这个目录（应用启动时调一次）
pub fn init(dir: PathBuf) {
    *LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(Log { dir, file: None });
}

/// 记一行。`kind` 是类别（录制 / 播放 / 应用 / 界面）。写不进去就算了，不影响正事
pub fn write(kind: &str, text: &str) {
    let mut guard = LOG.lock().unwrap_or_else(|e| e.into_inner());
    let Some(log) = guard.as_mut() else { return };
    let line = format!("{}  {kind}  {}\n", now(), text.replace("\r\n", " | ").replace(['\n', '\r'], " | "));
    if log.file.as_ref().and_then(|f| f.metadata().ok()).is_some_and(|m| m.len() >= MAX_BYTES) {
        log.file = None;
        let _ = std::fs::rename(log.dir.join(NAME), log.dir.join(OLD_NAME));
    }
    if log.file.is_none() {
        let _ = std::fs::create_dir_all(&log.dir);
        log.file = OpenOptions::new().create(true).append(true).open(log.dir.join(NAME)).ok();
    }
    if let Some(f) = log.file.as_mut()
        && f.write_all(line.as_bytes()).is_err()
    {
        // 文件被删了或者盘满了：下次重新打开
        log.file = None;
    }
}

/// 本地时间，精确到十分之一秒
fn now() -> String {
    // SAFETY: GetLocalTime 只往返回值里写当前时间，没有前置条件
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{}",
        t.wYear,
        t.wMonth,
        t.wDay,
        t.wHour,
        t.wMinute,
        t.wSecond,
        t.wMilliseconds / 100
    )
}
