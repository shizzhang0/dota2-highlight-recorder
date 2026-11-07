//! 设置：存在 `%APPDATA%\dota2-highlight-recorder\settings.json`。读不到或缺项就用默认值。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::matches::Rules;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    /// 输出目录
    pub out_dir: String,
    /// 录制画面大小："1080p"（以 1920×1080 启动游戏，比 2560×1600 录得快约一倍）或 "native"（跟游戏窗口一致）。
    /// 原先叫 resolution、可填任意大小、默认跟游戏窗口一致；2026-11 改成这两个选项、默认 1080p，旧值不再沿用
    pub capture: String,
    /// 镜头："hero_chase"（英雄一直在画面中间）或 "player_view"（还原玩家当时自己的镜头）。比赛页上选，记住上次的
    pub camera: String,
    /// 录完再拼一个合集
    pub compile: bool,
    /// 全部录完后让游戏退出（中途停下或出错时保留，方便接着录）。只对 "native" 有用：1080p 录完一定退出（要改回显示设置）。
    /// 原先叫 quit_after、默认关，改名后旧值不再沿用
    pub quit_when_done: bool,
    /// 界面语言："auto" 跟随系统，或 "zh-CN" / "en"
    pub language: String,
    /// ffmpeg.exe 或所在目录；空 = 自动找（程序目录、环境变量 D2HR_FFMPEG、PATH）
    pub ffmpeg: String,
    /// 片段规则
    pub rules: Rules,
    /// 这台机器上次实测的录制速度（每秒墙钟录多少秒画面），估录制耗时用；没录过为 null
    pub measured_speed: Option<f32>,
}

impl Settings {
    /// 片段规则参数，录制速度用这台机器实测的
    pub fn params(&self) -> clips::Params {
        let mut p = self.rules.params();
        if let Some(speed) = self.measured_speed {
            p.speed = speed;
        }
        p
    }

    /// 录制时用的游戏窗口大小；None = 跟游戏窗口一致
    pub fn capture_size(&self) -> Option<(u32, u32)> {
        (self.capture != "native").then_some((1920, 1080))
    }
}

impl Default for Settings {
    fn default() -> Self {
        let videos = std::env::var("USERPROFILE").map(|h| PathBuf::from(h).join(r"Videos\d2hr")).unwrap_or_default();
        Settings {
            out_dir: videos.to_string_lossy().into_owned(),
            capture: "1080p".into(),
            camera: "hero_chase".into(),
            compile: false,
            quit_when_done: true,
            language: "auto".into(),
            ffmpeg: String::new(),
            rules: Rules::default(),
            measured_speed: None,
        }
    }
}

/// 运行日志目录：`%APPDATA%\dota2-highlight-recorder\logs`
pub fn logs_dir() -> Option<PathBuf> {
    std::env::var("APPDATA").ok().map(|d| PathBuf::from(d).join("dota2-highlight-recorder").join("logs"))
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
