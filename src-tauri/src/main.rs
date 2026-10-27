//! 桌面应用：选录像、选玩家、预览并勾选片段、录制、管理输出的视频。
//! 录制流程在 `pipeline`，和命令行共用；这里只是把它接到界面上。计划见 docs/phase2-plan.md。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod history;
mod matches;
mod record;
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
            matches::download_replay,
            matches::delete_replays,
            matches::open_match,
            matches::plan,
            matches::default_rules,
            matches::hero_names,
            matches::reveal,
            record::start_recording,
            record::stop_recording,
            record::resume_recording,
            record::answer_confirm,
            record::is_recording,
            record::quit_game,
            record::game_open,
            history::recorded_matches,
            history::delete_match_record,
            history::open_file,
            settings::get_settings,
            settings::save_settings,
            settings::default_settings,
        ])
        // 关主窗口就退出：隐藏的进度窗也算窗口，不然进程会留在后台。
        // 录制中先不关，当作点了停止：等流程停下、恢复完游戏设置，用户再关
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "main"
            {
                if record::is_recording() {
                    api.prevent_close();
                    record::stop_recording(window.app_handle().clone());
                } else {
                    window.app_handle().exit(0);
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("启动失败");
}
