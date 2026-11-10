//! 一键更新，做法同 dota2-game-helper2。
//!
//! 前端（`ui/js/update.js`）查 GitHub 的最新 Release，负责**发现**新版本（从前端发请求：WebView2 走系统代理）；
//! 这里负责**装**：Tauri 的 updater 插件读 Release 里的 `latest.json`，下载安装包、验签、静默安装，装完程序自己重启。
//!
//! **只对安装版生效**：插件装的是 NSIS 安装包。看 exe 旁边有没有 NSIS 写的 `uninstall.exe`，没有就当成
//! 直接编译出来跑的，前端只给「打开下载页」。**录制中不装**：装新版会结束程序，游戏设置来不及恢复。

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

/// 是不是 NSIS 装出来的：安装目录里有 `uninstall.exe`
#[tauri::command]
pub fn app_installed() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("uninstall.exe").is_file()))
        .unwrap_or(false)
}

/// 程序版本号（tauri.conf.json 的 version）
#[tauri::command]
pub fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

/// 用默认浏览器打开 Releases 页。地址写死在这里，前端传不进任意网址
#[tauri::command]
pub fn open_release_page() {
    let _ = std::process::Command::new("explorer")
        .arg("https://github.com/shizzhang0/dota2-highlight-recorder/releases/latest")
        .spawn();
}

#[derive(Clone, Serialize)]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
}

/// 下载并安装。成功的话安装器一启动程序就被结束，这个函数不会正常返回；返回了就是失败或没有可装的更新，
/// 错误给前端显示："portable" = 不是安装版，"recording" = 正在录制，"none" = latest.json 里没有更新的版本
#[tauri::command]
pub async fn update_install(app: AppHandle) -> Result<(), String> {
    if !app_installed() {
        return Err("portable".into());
    }
    if crate::record::is_recording() {
        return Err("recording".into());
    }
    let update = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?;
    let Some(update) = update else {
        return Err("none".into());
    };
    let mut downloaded = 0u64;
    let app2 = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                let _ = app2.emit("update-progress", Progress { downloaded, total });
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;
    // Windows 上安装器启动时程序已经被结束；走到这里说明没装上，重启一下
    app.restart();
}
