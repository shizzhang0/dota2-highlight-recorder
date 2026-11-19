//! 应用日志接到界面上：界面写日志（播放情况、页面报错）、打开日志文件夹。写文件的部分在 `pipeline::applog`，
//! 录制、播放、应用本身都记在同一个 `d2hr.log` 里（2026-11-19 用户定）

/// 界面能写的类别：播放（每次播放的开始、结束和出问题的那一秒，见 ui/js/videos.js）、界面（页面上没接住的报错）
const UI_KINDS: [&str; 2] = ["播放", "界面"];

/// 一行最多这么多字，免得一个异常对象把日志撑大
const MAX_CHARS: usize = 2000;

/// 应用启动时调：日志写到 `%APPDATA%\dota2-highlight-recorder\logs`，记一行启动
pub fn init() {
    if let Some(dir) = crate::settings::logs_dir() {
        pipeline::applog::init(dir);
    }
    pipeline::applog::write("应用", &format!("启动 v{}", env!("CARGO_PKG_VERSION")));
}

/// 记一行「应用」类的日志
pub fn app(text: &str) {
    pipeline::applog::write("应用", text);
}

#[tauri::command]
pub fn write_log(kind: String, text: String) -> Result<(), String> {
    if !UI_KINDS.contains(&kind.as_str()) {
        return Err(format!("界面不能写这类日志：{kind}"));
    }
    let text: String = text.chars().take(MAX_CHARS).collect();
    pipeline::applog::write(&kind, &text);
    Ok(())
}

/// 在资源管理器里打开日志文件夹（还没有就先建好）
#[tauri::command]
pub fn open_logs() -> Result<(), String> {
    let dir = crate::settings::logs_dir().ok_or("找不到 APPDATA 目录")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::process::Command::new("explorer").arg(&dir).spawn().map(drop).map_err(|e| e.to_string())
}
