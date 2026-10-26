//! 设置：存在 `%APPDATA%\dota2-highlight-recorder\settings.json`。读不到或缺项就用默认值。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::matches::Rules;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    /// 输出目录
    pub out_dir: String,
    /// 录制分辨率，例如 "1920x1080"；空 = 跟游戏窗口一致
    pub resolution: String,
    /// 录完再拼一个合集
    pub compile: bool,
    /// 全部录完后让游戏退出（中途停下或出错时保留，方便接着录）。原先叫 quit_after、默认关，改名后旧值不再沿用
    pub quit_when_done: bool,
    /// 界面语言："auto" 跟随系统，或 "zh-CN" / "en"
    pub language: String,
    /// ffmpeg.exe 或所在目录；空 = 自动找（程序目录、环境变量 D2HR_FFMPEG、PATH）
    pub ffmpeg: String,
    /// 片段规则
    pub rules: Rules,
}

impl Default for Settings {
    fn default() -> Self {
        let videos = std::env::var("USERPROFILE").map(|h| PathBuf::from(h).join(r"Videos\d2hr")).unwrap_or_default();
        Settings {
            out_dir: videos.to_string_lossy().into_owned(),
            resolution: String::new(),
            compile: false,
            quit_when_done: true,
            language: "auto".into(),
            ffmpeg: String::new(),
            rules: Rules::default(),
        }
    }
}

fn path() -> Option<PathBuf> {
    std::env::var("APPDATA").ok().map(|d| PathBuf::from(d).join(r"dota2-highlight-recorder\settings.json"))
}

#[tauri::command]
pub fn get_settings() -> Settings {
    path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[tauri::command]
pub fn save_settings(settings: Settings) -> Result<(), String> {
    let p = path().ok_or("找不到 APPDATA 目录")?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&p, text).map_err(|e| format!("写不了 {}：{e}", p.display()))
}

#[tauri::command]
pub fn default_settings() -> Settings {
    Settings::default()
}
