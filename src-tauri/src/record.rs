//! 在应用里录制：把 `pipeline` 的进度事件转成界面事件，确认请求弹给界面等用户回答。
//!
//! 事件：`record-event`（`pipeline::Event`）、`record-confirm`（{id, question}）、`record-finished`（`Finished`）、
//! `record-paused` / `record-resumed` / `record-stopping`。一次只能录一场；录制在单独的线程里跑，命令立刻返回。
//!
//! 游戏开着时基本是全屏，要用户拍板的事（暂停后继续还是停止、要不要重启游戏）弹一个置顶的小对话框（`prompt` 窗口），
//! 不把用户切回主窗口；主窗口上同样能操作，哪边先回答都行，另一边跟着收起。

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};

use game::cancel::{self, Reason};
use media::ffmpeg::Ffmpeg;
use pipeline::{Event, Host, Options, Selected};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::settings::Settings;

static RUNNING: AtomicBool = AtomicBool::new(false);
static NEXT_QUESTION: AtomicU64 = AtomicU64::new(1);
/// 正在等用户回答的确认请求
static PENDING: Mutex<Option<(u64, Sender<bool>)>> = Mutex::new(None);

struct AppHost {
    app: AppHandle,
}

#[derive(Serialize, Clone)]
struct Question {
    id: u64,
    question: String,
}

impl Host for AppHost {
    fn event(&self, e: Event) {
        let _ = self.app.emit("record-event", e);
    }

    fn confirm(&self, question: &str) -> bool {
        let id = NEXT_QUESTION.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        *PENDING.lock().unwrap() = Some((id, tx));
        let q = Question { id, question: question.to_owned() };
        let _ = self.app.emit("record-confirm", q.clone());
        show_prompt(&self.app, Prompt::Confirm(q));
        rx.recv().unwrap_or(false)
    }
}

fn bring_to_front(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 置顶小对话框里显示什么
#[derive(Serialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Prompt {
    /// 暂停了：继续还是停止
    Pause,
    /// 录制流程要用户回答是/否
    Confirm(Question),
}

fn show_prompt(app: &AppHandle, p: Prompt) {
    let Some(w) = app.get_webview_window("prompt") else { return };
    let _ = app.emit_to("prompt", "prompt-show", p);
    let _ = w.center();
    let _ = w.show();
    let _ = w.set_focus();
}

fn hide_prompt(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("prompt") {
        let _ = w.hide();
    }
}

/// 回答确认请求（主窗口或置顶对话框）
#[tauri::command]
pub fn answer_confirm(app: AppHandle, id: u64, yes: bool) {
    let mut pending = PENDING.lock().unwrap();
    if pending.as_ref().is_some_and(|(i, _)| *i == id)
        && let Some((_, tx)) = pending.take()
    {
        let _ = tx.send(yes);
        hide_prompt(&app);
        // 两边都弹了，告诉另一边收起
        let _ = app.emit("record-confirm-done", id);
    }
}

/// 开始录制时发给界面（进度窗用来算还剩多久）
#[derive(Serialize, Clone)]
struct Started {
    clips: usize,
    /// 预计总耗时（秒，不含启动游戏）
    estimated_secs: f32,
}

/// 录制期间的置顶进度窗：录制时游戏在前台，主窗口看不到，用户需要知道录到哪了、怎么停。
/// 鼠标穿透、不抢焦点，放在屏幕左侧中上，避开程序要点的地方（右上角观战菜单、底部剪辑工具、中间对话框）。
/// 剪辑工具在引擎里渲染，不截屏幕，所以它不会出现在视频里
fn show_overlay(app: &AppHandle) {
    let Some(w) = app.get_webview_window("overlay") else { return };
    if let Ok(Some(m)) = app.primary_monitor() {
        let (pos, size) = (m.position(), m.size());
        let x = pos.x + (size.width as f64 * 0.012) as i32;
        let y = pos.y + (size.height as f64 * 0.30) as i32;
        let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
    }
    let _ = w.set_ignore_cursor_events(true);
    let _ = w.show();
}

fn hide_overlay(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.hide();
    }
}

/// 录制结束（录完、停下或出错）时发给界面
#[derive(Serialize, Clone)]
struct Finished {
    files: Vec<PathBuf>,
    /// 做好的片段序号（计划里的序号）
    completed: Vec<usize>,
    /// 这次选了要录的片段序号
    selected: Vec<usize>,
    /// 停下的原因（文字）；正常录完或出错是 null
    cancelled: Option<String>,
    error: Option<String>,
    elapsed_secs: f32,
    out_dir: String,
    /// 录制用的游戏还开着（停下或出错时保留，或者设置了录完不退出）
    game_open: bool,
}

fn options(s: &Settings) -> Result<Options, String> {
    let resolution = match s.resolution.trim() {
        "" => None,
        r => {
            let (w, h) = r.split_once(['x', 'X']).ok_or(format!("分辨率要写成 宽x高：{r}"))?;
            Some((w.trim().parse().map_err(|_| format!("分辨率不对：{r}"))?, h.trim().parse().map_err(|_| format!("分辨率不对：{r}"))?))
        }
    };
    Ok(Options {
        compile: s.compile,
        resolution,
        out: PathBuf::from(&s.out_dir),
        gsi_port: game::gsi::DEFAULT_PORT,
        quit: s.quit_when_done,
        on_input: Some(game::input_guard::OnInput::Pause),
    })
}

/// 开始录制 `clips`（计划里的序号）。设置读当前保存的
#[tauri::command]
pub async fn start_recording(app: AppHandle, match_id: u64, player: usize, clips: Vec<usize>) -> Result<(), String> {
    if clips.is_empty() {
        return Err("没有选片段".into());
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Err("已经在录了".into());
    }
    let prepared = async {
        let settings = crate::settings::get_settings();
        let o = options(&settings)?;
        let ffmpeg = settings.ffmpeg.trim();
        let ff = Ffmpeg::locate((!ffmpeg.is_empty()).then(|| std::path::Path::new(ffmpeg))).map_err(|e| format!("{e:#}"))?;
        let r = crate::matches::parsed(match_id).await?;
        let p = settings.rules.params();
        let plan = clips::plan(&r, player, &p);
        let selected: Vec<Selected> = clips
            .iter()
            .map(|&i| plan.get(i.wrapping_sub(1)).map(|c| Selected { index: i, clip: c.clone() }).ok_or(format!("没有第 {i} 个片段")))
            .collect::<Result<_, _>>()?;
        Ok::<_, String>((o, ff, r, p, selected))
    }
    .await;
    let (o, ff, r, p, selected) = match prepared {
        Ok(v) => v,
        Err(e) => {
            RUNNING.store(false, Ordering::SeqCst);
            return Err(e);
        }
    };

    cancel::reset();
    let estimated_secs: f32 = selected.iter().map(|s| s.clip.estimated_seconds(&p)).sum();
    show_overlay(&app);
    let _ = app.emit("record-started", Started { clips: selected.len(), estimated_secs });
    // 盯着暂停和停止：用户一动就马上告诉界面"已暂停"并把应用切到前面（流程要做完手上这一步才真正停住）；
    // 点了停止就告诉进度窗"正在停下"（恢复设置要几秒）
    let watcher = app.clone();
    std::thread::spawn(move || {
        let mut was_paused = false;
        while RUNNING.load(Ordering::SeqCst) {
            if let Some(reason) = cancel::pending() {
                let _ = watcher.emit("record-stopping", reason.describe());
                hide_prompt(&watcher);
                break;
            }
            let paused = cancel::paused();
            if paused != was_paused {
                was_paused = paused;
                let _ = watcher.emit(if paused { "record-paused" } else { "record-resumed" }, ());
                // 游戏基本是全屏的：在游戏上面弹置顶对话框问继续还是停止，不把用户切回主窗口
                if paused {
                    show_prompt(&watcher, Prompt::Pause);
                } else {
                    hide_prompt(&watcher);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    });
    std::thread::spawn(move || {
        let host = AppHost { app: app.clone() };
        let report = pipeline::run(&host, &r, player, &selected, &p, &ff, &o);
        let finished = Finished {
            files: report.files,
            completed: report.completed,
            selected: clips,
            cancelled: report.cancelled.map(|c| c.describe().to_owned()),
            error: report.error.map(|e| format!("{e:#}")),
            elapsed_secs: report.elapsed.as_secs_f32(),
            out_dir: o.out.to_string_lossy().into_owned(),
            game_open: !report.game_quit && game::window::dota_window().is_ok(),
        };
        RUNNING.store(false, Ordering::SeqCst);
        // 进度窗先显示结果，停留几秒再收起；主窗口切到前面显示结果页
        let _ = app.emit("record-finished", finished);
        bring_to_front(&app);
        std::thread::sleep(std::time::Duration::from_secs(3));
        if !RUNNING.load(Ordering::SeqCst) {
            hide_overlay(&app);
        }
    });
    Ok(())
}

/// 让录制用的游戏退出（结果页的"退出游戏"按钮）
#[tauri::command]
pub fn quit_game() -> Result<(), String> {
    game::netcon::Netcon::connect(2121).and_then(|mut c| c.send("quit")).map_err(|e| format!("{e:#}"))
}

/// 暂停后点了"继续"
#[tauri::command]
pub fn resume_recording(app: AppHandle) {
    cancel::resume();
    hide_prompt(&app);
}

/// 点了停止（暂停中、或者"等用户退出 Dota"这类没开检测的阶段）
#[tauri::command]
pub fn stop_recording(app: AppHandle) {
    cancel::request(Reason::Stop);
    hide_prompt(&app);
    // 正在等确认的话当作"不同意"
    if let Some((_, tx)) = PENDING.lock().unwrap().take() {
        let _ = tx.send(false);
    }
}

#[tauri::command]
pub fn is_recording() -> bool {
    RUNNING.load(Ordering::SeqCst)
}
