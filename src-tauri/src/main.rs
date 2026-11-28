//! 桌面应用：选录像、选玩家、预览并勾选片段、录制、管理输出的视频。
//! 录制流程在 `pipeline`，和命令行共用；这里只是把它接到界面上。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod account;
mod history;
mod log;
mod matches;
mod record;
mod settings;
mod uierr;
mod updater;
mod video;

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
        // 录完或出错时弹系统通知
        .plugin(tauri_plugin_notification::init())
        // 一键更新（只对安装版生效，见 updater.rs）
        .plugin(tauri_plugin_updater::Builder::new().build())
        // 应用日志（log.rs）：插件都起来后才记，第二次打开的那个进程在单实例插件里就退出了，不会多记一次启动
        .setup(|app| {
            log::init();
            log::watch_main_thread(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            matches::list_replays,
            matches::download_replay,
            matches::cancel_download,
            matches::delete_replays,
            matches::open_match,
            matches::has_replay,
            matches::plan,
            matches::default_rules,
            matches::hero_names,
            matches::reveal,
            record::start_recording,
            record::stop_recording,
            record::answer_confirm,
            record::is_recording,
            record::record_result,
            record::quit_game,
            record::game_open,
            account::accounts,
            account::set_account,
            history::recorded_matches,
            history::delete_video,
            history::open_file,
            video::video_url,
            history::video_thumb,
            settings::get_settings,
            settings::save_settings,
            updater::app_installed,
            updater::app_version,
            updater::open_release_page,
            updater::update_install,
            log::write_log,
            log::open_logs,
        ])
        // 关主窗口就退出：隐藏的置顶对话框也算窗口，不然进程会一直留着。
        // 录制中先不关，当作点了停止：等流程停下、恢复完游戏设置，用户再关
        .on_window_event(|window, event| {
            // 显示设置变了（Dota 退出时把屏幕分辨率换回来之类）也记一笔
            if let tauri::WindowEvent::ScaleFactorChanged { scale_factor, .. } = event
                && window.label() == "main"
            {
                log::app(&format!("主窗口缩放变成 {scale_factor}"));
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "main"
            {
                if record::is_recording() {
                    api.prevent_close();
                    record::stop_recording(window.app_handle().clone());
                } else {
                    log::app("退出");
                    window.app_handle().exit(0);
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("启动失败");
}
