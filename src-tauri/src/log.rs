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

/// 主线程多久没响应就记一笔
const MAIN_SLOW: std::time::Duration = std::time::Duration::from_secs(2);
/// 主线程这么久还没响应，先记一笔「还卡着」，等它恢复再记一共卡了多久
const MAIN_STUCK: std::time::Duration = std::time::Duration::from_secs(5);

/// 每秒往主线程（窗口消息循环）塞一个空任务，看它多久才执行。页面和后台之间的消息都要经过主线程：
/// 2026-11-18/19 录完后页面要点一下窗口才收到「录完了」，那段时间页面自己的定时器照常在走，
/// 查是不是主线程被什么卡住了
pub fn watch_main_thread(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        loop {
            let (tx, rx) = std::sync::mpsc::channel();
            let t0 = std::time::Instant::now();
            if app
                .run_on_main_thread(move || {
                    let _ = tx.send(());
                })
                .is_err()
            {
                return;
            }
            match rx.recv_timeout(MAIN_STUCK) {
                Ok(()) if t0.elapsed() >= MAIN_SLOW => self::app(&format!("主线程 {:.1} 秒没响应", t0.elapsed().as_secs_f32())),
                Ok(()) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    self::app(&format!("主线程已经 {} 秒没响应", MAIN_STUCK.as_secs()));
                    if rx.recv().is_err() {
                        return;
                    }
                    self::app(&format!("主线程恢复了，一共 {:.1} 秒没响应", t0.elapsed().as_secs_f32()));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
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
    let dir = crate::settings::logs_dir().ok_or_else(|| crate::uierr::key("err.noAppData"))?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::process::Command::new("explorer").arg(&dir).spawn().map(drop).map_err(|e| e.to_string())
}
