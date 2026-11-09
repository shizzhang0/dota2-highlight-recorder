//! 在应用里录制：把 `pipeline` 的进度事件转成界面事件，确认请求弹给界面等用户回答。
//!
//! 事件：`record-event`（`pipeline::Event`）、`record-confirm`（{id, question}）、`record-finished`（`Finished`）、
//! `record-paused` / `record-resumed` / `record-stopping`。一次只能录一场；录制在单独的线程里跑，命令立刻返回。
//!
//! 录制期间用户可能在用别的程序：进度显示在主窗口，录完或出错弹系统通知。
//! 要用户拍板的事（暂停后继续还是停止、要不要重启游戏）弹一个置顶的小对话框（`prompt` 窗口）；
//! 主窗口上同样能操作，哪边先回答都行，另一边跟着收起。

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};

use game::cancel::{self, Reason};
use media::ffmpeg::Ffmpeg;
use pipeline::{Camera, Event, Host, Options, Selected};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::settings::Settings;

static RUNNING: AtomicBool = AtomicBool::new(false);
/// 上次录完后游戏留着没退
static GAME_OPEN: AtomicBool = AtomicBool::new(false);
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

/// 开始录制时发给界面
#[derive(Serialize, Clone)]
struct Started {
    clips: usize,
    /// 预计总耗时（秒，不含启动游戏），按这台机器实测的录制速度估
    estimated_secs: f32,
}

/// 录完或出错时弹系统通知（用户可能在用别的程序）。`lang` 是界面语言
fn notify(app: &AppHandle, f: &Finished, lang: &str) {
    let en = lang.starts_with("en");
    let (title, body) = match (&f.error, &f.cancelled) {
        (Some(e), _) => (if en { "Recording failed" } else { "录制出错了" }.to_owned(), e.clone()),
        (None, Some(reason)) => (
            if en { "Recording stopped" } else { "录制已停止" }.to_owned(),
            if en { format!("{} of {} clips done ({reason})", f.completed.len(), f.selected.len()) } else { format!("完成 {}/{} 个片段（{reason}）", f.completed.len(), f.selected.len()) },
        ),
        (None, None) => (
            if en { "Recording finished" } else { "录完了" }.to_owned(),
            if en { format!("{} clips, {:.1} min", f.completed.len(), f.elapsed_secs / 60.0) } else { format!("{} 个片段，用时 {:.1} 分钟", f.completed.len(), f.elapsed_secs / 60.0) },
        ),
    };
    let _ = app.notification().builder().title(title).body(body).show();
}

/// 录制结束（录完、停下或出错）时发给界面
#[derive(Serialize, Clone)]
struct Finished {
    match_id: u64,
    /// 写录制记录失败的原因（界面提示一下，不影响已经录好的文件）
    history_error: Option<String>,
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
    /// 这次实测的录制速度（每秒墙钟录多少秒画面）
    speed: Option<f32>,
}

fn options(s: &Settings) -> Options {
    Options {
        compile: s.compile,
        resolution: s.capture_size(),
        out: PathBuf::from(&s.out_dir),
        gsi_port: game::gsi::DEFAULT_PORT,
        quit: s.quit_when_done,
        camera: if s.camera == "player_view" { Camera::PlayerView } else { Camera::HeroChase },
        on_input: Some(game::input_guard::OnInput::Pause),
        log_dir: crate::settings::logs_dir(),
    }
}

/// 开始录制 `clips`（计划里的序号）。`camera` 是比赛页上选的镜头（存进设置，下次默认用它），`lang` 是界面语言（通知用）。
/// 其他设置读当前保存的
#[tauri::command]
pub async fn start_recording(app: AppHandle, match_id: u64, player: usize, clips: Vec<usize>, camera: String, lang: String) -> Result<(), String> {
    if clips.is_empty() {
        return Err("没有选片段".into());
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Err("已经在录了".into());
    }
    let prepared = async {
        let mut settings = crate::settings::get_settings();
        if settings.camera != camera {
            settings.camera = camera;
            crate::settings::save_settings(settings.clone())?;
        }
        let o = options(&settings);
        let ffmpeg = settings.ffmpeg.trim();
        let ff = Ffmpeg::locate((!ffmpeg.is_empty()).then(|| std::path::Path::new(ffmpeg))).map_err(|e| format!("{e:#}"))?;
        let r = crate::matches::parsed(match_id).await?;
        let p = settings.params();
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
    let _ = app.emit("record-started", Started { clips: selected.len(), estimated_secs });
    // 盯着暂停和停止：用户在游戏里一动就马上告诉界面"已暂停"、弹置顶对话框（流程要停掉手上这一段才真正停住）；
    // 点了停止就告诉界面"正在停下"（恢复设置要几秒）
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
                // 用户刚在游戏里操作过：在最上面弹置顶对话框问继续还是停止，不把用户切回主窗口
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
        let time = crate::matches::now();
        let report = pipeline::run(&host, &r, player, &selected, &ff, &o);
        // 记下这台机器的录制速度，下次估耗时用（明显不合理的不要）
        if let Some(speed) = report.speed.filter(|s| (0.05..5.0).contains(s)) {
            let mut settings = crate::settings::get_settings();
            settings.measured_speed = Some(speed);
            let _ = crate::settings::save_settings(settings);
        }
        // 记进"已录视频"：files 和 completed 一一对应，多出来的最后一个是合集
        let done: std::collections::HashMap<usize, &PathBuf> = report.completed.iter().copied().zip(&report.files).collect();
        let hero = |i: usize| r.players[i].hero.trim_start_matches("npc_dota_hero_").to_owned();
        let run = crate::history::Run {
            time,
            match_id,
            player,
            hero: hero(player),
            player_name: r.players[player].name.clone(),
            is_me: game::steam::me() == Some(r.players[player].account_id),
            clips: selected
                .iter()
                .map(|s| crate::history::RunClip {
                    index: s.index,
                    file_name: clips::file_name(&r, &s.clip),
                    start: s.clip.start(),
                    end: s.clip.end(),
                    victims: s.clip.kills.iter().map(|&k| hero(r.kills[k].victim)).collect(),
                    multi_kill: s.clip.multi_kill,
                    path: done.get(&s.index).map(|p| (*p).clone()),
                })
                .collect(),
            compilation: report.files.get(report.completed.len()).cloned(),
            cancelled: report.cancelled.map(|c| c.describe().to_owned()),
            error: report.error.as_ref().map(|e| format!("{e:#}")),
            elapsed_secs: report.elapsed.as_secs_f32(),
        };
        let history_error = crate::history::add(run).err();
        let game_open = !report.game_quit && game::window::dota_window().is_ok();
        GAME_OPEN.store(game_open, Ordering::SeqCst);
        let finished = Finished {
            match_id,
            history_error,
            files: report.files,
            completed: report.completed,
            selected: clips,
            cancelled: report.cancelled.map(|c| c.describe().to_owned()),
            error: report.error.map(|e| format!("{e:#}")),
            elapsed_secs: report.elapsed.as_secs_f32(),
            out_dir: o.out.to_string_lossy().into_owned(),
            game_open,
            speed: report.speed,
        };
        RUNNING.store(false, Ordering::SeqCst);
        notify(&app, &finished, &lang);
        // 主窗口显示这场比赛的录制详情；不抢前台（用户可能在用别的程序），点通知或任务栏图标就能看到
        let _ = app.emit("record-finished", finished);
    });
    Ok(())
}

/// 让录制用的游戏退出（导航栏的"退出"）
#[tauri::command]
pub fn quit_game() -> Result<(), String> {
    game::netcon::Netcon::connect(2121).and_then(|mut c| c.send("quit")).map_err(|e| format!("{e:#}"))?;
    GAME_OPEN.store(false, Ordering::SeqCst);
    Ok(())
}

/// 录制用的游戏还开着（录完留着的那个）。用户自己关掉游戏后变 false。导航栏定时问
#[tauri::command]
pub fn game_open() -> bool {
    if GAME_OPEN.load(Ordering::SeqCst) && game::window::dota_window().is_err() {
        GAME_OPEN.store(false, Ordering::SeqCst);
    }
    GAME_OPEN.load(Ordering::SeqCst)
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
