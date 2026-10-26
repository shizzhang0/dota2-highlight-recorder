//! 在应用里录制：把 `pipeline` 的进度事件转成界面事件，确认请求弹给界面等用户回答。
//!
//! 事件：`record-event`（`pipeline::Event`）、`record-confirm`（{id, question}）、`record-finished`（`Finished`）。
//! 一次只能录一场；录制在单独的线程里跑，命令立刻返回。

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
        // 要用户拍板时把应用切到前面
        bring_to_front(&self.app);
        let _ = self.app.emit("record-confirm", Question { id, question: question.to_owned() });
        rx.recv().unwrap_or(false)
    }
}

fn bring_to_front(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 界面回答确认请求
#[tauri::command]
pub fn answer_confirm(id: u64, yes: bool) {
    let mut pending = PENDING.lock().unwrap();
    if pending.as_ref().is_some_and(|(i, _)| *i == id)
        && let Some((_, tx)) = pending.take()
    {
        let _ = tx.send(yes);
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
        quit: s.quit_after,
        input_guard: true,
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
        };
        RUNNING.store(false, Ordering::SeqCst);
        bring_to_front(&app);
        let _ = app.emit("record-finished", finished);
    });
    Ok(())
}

/// 界面上点了停止。录制期间点鼠标本身也会被输入检测发现，这个主要给"等用户退出 Dota"这类没开检测的阶段用
#[tauri::command]
pub fn stop_recording() {
    cancel::request(Reason::Stop);
    // 正在等确认的话当作"不同意"
    if let Some((_, tx)) = PENDING.lock().unwrap().take() {
        let _ = tx.send(false);
    }
}

#[tauri::command]
pub fn is_recording() -> bool {
    RUNNING.load(Ordering::SeqCst)
}
