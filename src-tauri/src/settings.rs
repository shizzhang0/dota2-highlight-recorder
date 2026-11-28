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
    /// 录完再拼一个集锦。比赛页底部"拼成集锦"勾没勾（2026-11-16 从设置页挪过去），记住上次的；新装默认勾上
    pub compile: bool,
    /// 界面语言："auto" 跟随系统，或 "zh-CN" / "en"
    pub language: String,
    /// ffmpeg.exe 或所在目录；空 = 自动找（程序目录、环境变量 D2HR_FFMPEG、PATH）
    pub ffmpeg: String,
    /// 片段规则
    pub rules: Rules,
    /// 旧版存的：上次实测的录制速度（每秒墙钟录多少秒画面）。`measured_speeds` 还空着时用它
    pub measured_speed: Option<f32>,
    /// 最近几次实测的录制速度，旧的在前，最多 `KEEP_SPEEDS` 个。估录制耗时用（`speed`）
    pub measured_speeds: Vec<f32>,
}

/// 记最近几次的录制速度
const KEEP_SPEEDS: usize = 5;

impl Settings {
    /// 片段规则参数，录制速度用这台机器实测的
    pub fn params(&self) -> clips::Params {
        let mut p = self.rules.params();
        if let Some(speed) = self.speed() {
            p.speed = speed;
        }
        p
    }

    /// 估耗时用的录制速度：最近几次实测的中位数（偶数个取偏快的那个）。
    /// 原来只用上一次的，一次反常的慢录制就把下一次的预计带偏：2026-11-18 有一次 Steam 刚打开、
    /// 测出 0.17（平时 0.8 左右），下一次 4 分钟的录制预计成了 11 分多钟
    fn speed(&self) -> Option<f32> {
        if self.measured_speeds.is_empty() {
            return self.measured_speed;
        }
        let mut s = self.measured_speeds.clone();
        s.sort_by(f32::total_cmp);
        Some(s[s.len() / 2])
    }

    /// 记下这次实测的录制速度，只留最近 `KEEP_SPEEDS` 次
    pub fn add_speed(&mut self, speed: f32) {
        if self.measured_speeds.is_empty()
            && let Some(old) = self.measured_speed
        {
            self.measured_speeds.push(old);
        }
        self.measured_speeds.push(speed);
        let extra = self.measured_speeds.len().saturating_sub(KEEP_SPEEDS);
        self.measured_speeds.drain(..extra);
        self.measured_speed = Some(speed);
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
            compile: true,
            language: "auto".into(),
            ffmpeg: String::new(),
            rules: Rules::default(),
            measured_speed: None,
            measured_speeds: Vec::new(),
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
        .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
        .map(Settings::migrated)
        .unwrap_or_default()
}

impl Settings {
    /// 旧版本存下的默认值换成新的：设置里总是存着整套片段规则，只改 `Params::default` 不会生效。
    /// 起点上限 2026-11-23 从 19 秒改成 8 秒，存着 19 的当成旧默认值
    fn migrated(mut self) -> Settings {
        if self.rules.max_lead == 19.0 {
            self.rules.max_lead = clips::Params::default().max_lead;
        }
        self
    }
}

#[tauri::command]
pub fn save_settings(settings: Settings) -> Result<(), String> {
    let p = path().ok_or_else(|| crate::uierr::key("err.noAppData"))?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&p, text).map_err(|e| crate::uierr::detail("err.saveSettings", format!("{}: {e}", p.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_slow_run_does_not_skew_the_speed() {
        // 旧设置只有上一次的速度：接着用，记新的时先把它放进列表
        let mut s = Settings { measured_speed: Some(0.8), ..Settings::default() };
        assert_eq!(s.speed(), Some(0.8));
        s.add_speed(0.17);
        assert_eq!(s.measured_speeds, vec![0.8, 0.17]);
        assert_eq!(s.speed(), Some(0.8));
        s.add_speed(0.78);
        assert_eq!(s.speed(), Some(0.78));
        for v in [0.82, 0.79, 0.81] {
            s.add_speed(v);
        }
        // 只留最近 5 次：最早的 0.8 被挤掉
        assert_eq!(s.measured_speeds, vec![0.17, 0.78, 0.82, 0.79, 0.81]);
        assert_eq!(s.speed(), Some(0.79));
    }

    #[test]
    fn no_speed_yet() {
        assert_eq!(Settings::default().speed(), None);
    }
}
