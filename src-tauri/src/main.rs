//! 桌面应用：选录像、选玩家、预览并勾选片段、录制、管理输出的视频。
//! 录制流程在 `pipeline`，和命令行共用；这里只是把它接到界面上。计划见 docs/phase2-plan.md。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod matches;
mod settings;

use tauri::Manager;

fn main() {
    tauri::Builder::default()
        // 第二次打开时，把已有的窗口提到前面，不再开第二个
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .invoke_handler(tauri::generate_handler![
            matches::list_replays,
            matches::open_match,
            matches::plan,
            matches::default_rules,
            matches::hero_names,
            matches::now,
            matches::reveal,
            settings::get_settings,
            settings::save_settings,
            settings::default_settings,
        ])
        .run(tauri::generate_context!())
        .expect("启动失败");
}
