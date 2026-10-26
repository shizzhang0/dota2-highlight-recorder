//! 录制流程：录一个玩家选定的高光片段，每个片段输出一个视频，可选拼成合集。
//! 命令行（`d2hr record`）和桌面应用共用。流程见 docs/phase1-plan.md「录制流程」。
//!
//! 和外界只通过 `Host` 打交道：进度用事件报告，需要用户拍板的事（例如重启游戏）用确认请求。

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clips::{Clip, Params};
use game::cancel::{self, Reason};
use game::clip_builder::{self, Job};
use game::gsi;
use game::netcon::Netcon;
use game::session::Session;
use game::spectator::{self, Panel, Team};
use game::ui::Ui;
use media::ffmpeg::Ffmpeg;
use replay::Replay;
use serde::Serialize;
use serde_json::Value;

const NETCON_PORT: u16 = 2121;
/// 导出文件实际时长和计划长度允许差多少秒，超了就重新导出一次
const DURATION_TOLERANCE: f64 = 2.0;
/// 剪辑工具的时间轴窗口（秒）：选区落在中间，离两边远，把手不容易被夹住
const WINDOW: f32 = 120.0;
/// 开始操作游戏界面前的倒计时（秒）
const COUNTDOWN: u32 = 5;

pub struct Options {
    pub compile: bool,
    pub resolution: Option<(u32, u32)>,
    pub out: PathBuf,
    pub gsi_port: u16,
    /// 录完让游戏退出。用 `resolution` 启动的游戏无论如何都会退出：
    /// 它退出时会把这个窗口大小写进显示设置，要等它退出后改回去
    pub quit: bool,
    /// 录制期间检测用户的鼠标键盘操作，一动就停下（另有取消快捷键）
    pub input_guard: bool,
}

/// 选定的片段和它在 `plan` 里的序号（从 1 开始）
pub struct Selected {
    pub index: usize,
    pub clip: Clip,
}

/// 录制过程中的进度
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// Dota 以普通模式开着，等用户自己退出
    WaitingForDotaToQuit,
    /// 启动游戏，带这些启动参数
    LaunchingGame { args: Vec<String> },
    /// 录像加载好了，马上倒计时
    ReplayLoaded,
    /// 倒计时：还有几秒开始操作游戏界面
    Countdown { secs: u32, hotkey: Option<String> },
    /// 取消快捷键没注册上（被别的程序占用），只能靠检测鼠标键盘停下
    HotkeyUnavailable { hotkey: String },
    /// 开始录第 `n` 个（共 `total` 个）选定的片段，它在计划里是第 `index` 个
    ClipStarted { n: usize, total: usize, index: usize, name: String },
    /// 开始导出一段：游戏时间范围和预计渲染秒数
    ExportStarted { from: f32, to: f32, estimated_secs: f32 },
    /// 一段导出完成
    Exported { from: f32, to: f32, secs: f64, render_secs: f32 },
    /// 导出不对，重来一次
    Retrying { reason: String },
    /// 一个片段做好了
    ClipDone { index: usize, path: PathBuf, secs: f64 },
    /// 合集做好了
    CompilationDone { path: PathBuf, secs: f64 },
    /// 让游戏退出，把显示设置里的窗口大小改回去
    RestoringWindowSize { width: u32, height: u32 },
    WindowSizeRestored { width: u32, height: u32 },
    WindowSizeNotRestored { width: u32, height: u32, error: String },
    /// 游戏设置没能全部恢复
    RestoreFailed { error: String },
    /// 出错时保留了排查用的截图和日志
    DiagnosticsKept { dir: PathBuf },
}

/// 录制流程的调用方：命令行打印事件、读标准输入；桌面应用转成界面事件、弹对话框
pub trait Host {
    fn event(&self, e: Event);
    /// 问用户一个是/否的问题
    fn confirm(&self, question: &str) -> bool;
}

/// 录制结果。出错或停下时也会给出已经做好的部分，方便从没录完的片段继续
#[derive(Debug)]
pub struct Report {
    /// 输出的文件（最后一个可能是合集）
    pub files: Vec<PathBuf>,
    /// 已经做好的片段序号（计划里的序号，从 1 开始）
    pub completed: Vec<usize>,
    /// 停下的原因；正常录完或出错是 None
    pub cancelled: Option<Reason>,
    pub error: Option<anyhow::Error>,
    pub elapsed: Duration,
}

fn dota_running() -> bool {
    game::window::dota_window().is_ok()
}

/// 等游戏窗口消失（用户自己退出，或者我们发了 quit）
fn wait_dota_gone(timeout: Duration) -> Result<()> {
    let started = Instant::now();
    while dota_running() {
        cancel::check()?;
        if started.elapsed() > timeout {
            bail!("等了 {timeout:?} 游戏还没退出");
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    // 窗口没了进程可能还在收尾，再等一会儿免得新启动的撞上
    std::thread::sleep(Duration::from_secs(3));
    Ok(())
}

/// 让带 netcon 的游戏退出（先征得同意）
fn restart_needed(host: &dyn Host, reason: &str) -> Result<()> {
    if !host.confirm(&format!("{reason}现在让游戏退出，再重新启动？")) {
        bail!("用户没同意重启游戏");
    }
    Netcon::connect(NETCON_PORT)?.send("quit")?;
    wait_dota_gone(Duration::from_secs(60))
}

/// 游戏客户区现在多大
fn client_size() -> Option<(u32, u32)> {
    Ui::attach().ok().map(|u| (u.width as u32, u.height as u32))
}

/// 准备好游戏：GSI 配置、带 netcon 启动（必要时先请用户退出普通模式的游戏）、连上控制台。
/// 第三个返回值：是不是我们带 `-w -h` 启动的（是的话录完要让游戏退出、把窗口大小改回去）
fn prepare(host: &dyn Host, o: &Options, diag: &Path) -> Result<(Netcon, Receiver<Value>, bool)> {
    gsi::check_port(o.gsi_port)?;
    let changed = gsi::install(o.gsi_port)?;
    let rx = gsi::listen(o.gsi_port)?;

    if dota_running() && !game::launch::netcon_ready(NETCON_PORT) {
        host.event(Event::WaitingForDotaToQuit);
        wait_dota_gone(Duration::from_secs(600))?;
    }
    if dota_running() {
        // 配置刚写入：除非游戏启动时就有同样的配置（上一轮写过、结束时删了），否则收不到包。
        // 暂停、尤其是游戏在后台时几秒才一个包，等久一点
        if changed && gsi::wait_for(&rx, Duration::from_secs(15), "GSI 包", |_| true).is_err() {
            restart_needed(host, "游戏开着，但没读到我们的 GSI 配置（配置只在游戏启动时读）。")?;
        } else if let (Some(want), Some(now)) = (o.resolution, client_size())
            && want != now
        {
            restart_needed(host, &format!("游戏窗口现在是 {}×{}，要录 {}×{} 得重新启动游戏。", now.0, now.1, want.0, want.1))?;
        }
    }
    let mut sized = false;
    if !dota_running() {
        let account = game::steam::me().context("认不出 Steam 账号（Steam 没登录？）")?;
        // 带上用户在 Steam 里设的启动选项（例如国服的 -perfectworld），再加分辨率
        let mut extra = game::steam::launch_options(account).unwrap_or_default();
        if let Some((w, h)) = o.resolution {
            extra.extend(["-w".into(), w.to_string(), "-h".into(), h.to_string()]);
            sized = true;
        }
        host.event(Event::LaunchingGame { args: extra.clone() });
        game::launch::launch(NETCON_PORT, &extra)?;
        game::launch::wait_netcon(NETCON_PORT, Duration::from_secs(180))?;
    }
    let mut con = Netcon::connect_logged(NETCON_PORT, &diag.join("console.log"))?;
    con.wait_responsive(Duration::from_secs(120))?;
    con.drain(Duration::from_millis(300), Duration::from_secs(2));
    Ok((con, rx, sized))
}

/// 录制。调用方负责在开始前清掉上一轮的取消标记（`game::cancel::reset`），命令行另外负责接管 Ctrl+C
pub fn run(host: &dyn Host, r: &Replay, player: usize, selected: &[Selected], p: &Params, ff: &Ffmpeg, o: &Options) -> Report {
    let started = Instant::now();
    // 开始时的前台窗口（终端或应用）：不管怎么结束，最后都切回它，用户回来能直接看到结果
    let origin = game::ui::foreground();
    let mut done = Done::default();
    let error = run_inner(host, r, player, selected, p, ff, o, &mut done).err();
    game::ui::bring_to_front(origin);
    // 停下的原因要在 hold 之外读（restore 期间读不到）
    let cancelled = cancel::reason();
    Report {
        files: done.files,
        completed: done.completed,
        cancelled,
        error: if cancelled.is_some() { None } else { error },
        elapsed: started.elapsed(),
    }
}

/// 已经做好的部分
#[derive(Default)]
struct Done {
    files: Vec<PathBuf>,
    completed: Vec<usize>,
}

#[allow(clippy::too_many_arguments)]
fn run_inner(
    host: &dyn Host,
    r: &Replay,
    player: usize,
    selected: &[Selected],
    p: &Params,
    ff: &Ffmpeg,
    o: &Options,
    done: &mut Done,
) -> Result<()> {
    if selected.is_empty() {
        bail!("没有选定要录的片段");
    }
    let team = Team::from_id(r.players[player].team).context("玩家阵营不是天辉也不是夜魇")?;
    let tmp = o.out.join("tmp");
    let diag = o.out.join("diag");
    std::fs::create_dir_all(&tmp)?;
    std::fs::create_dir_all(&diag)?;
    // 上一轮在渲染途中停下时，游戏会把渲染完的视频留在 tmp 里；这个目录只放我们的中间文件，清掉
    for e in std::fs::read_dir(&tmp)?.flatten() {
        if e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("mp4")) {
            let _ = std::fs::remove_file(e.path());
        }
    }

    // 带 -w -h 启动的游戏退出时会把这个窗口大小写进显示设置，先记下原来的
    let account = game::steam::me();
    let original_size = account.and_then(|a| game::steam::video(a).ok()).and_then(|v| v.width.zip(v.height));
    let (con, rx, sized) = match prepare(host, o, &diag) {
        Ok(v) => v,
        Err(e) => {
            // 还没开始改设置，只要删掉刚写的 GSI 配置
            let _ = gsi::remove();
            return Err(e);
        }
    };
    // 设置守护：离开作用域（包括出错）时恢复，并删掉 GSI 配置
    let mut s = Session::new(con);
    s.remove_gsi_on_exit();
    // 第三个参数是游戏默认值：当前值已经是我们要设的值时（上一轮的残留），结束时恢复成它
    s.set_or_default("dota_spectator_enable_edgepan_temporary_free_camera", "0", Some("true"))?;
    s.set_or_default("dota_clip_builder_max_duration", &WINDOW.to_string(), Some("60"))?;
    s.set_or_default("dota_clip_builder_output_folder", &tmp.to_string_lossy().replace('\\', "/"), Some(""))?;

    let result = record_all(host, r, player, team, selected, p, ff, o, &mut s, &rx, &tmp, &diag, done);
    let restored = s.restore();
    // 下面这些收尾也不能被取消打断
    let hold = cancel::hold();
    if o.quit || sized {
        let _ = s.con().send("quit");
    }
    if sized && let (Some(account), Some((width, height))) = (account, original_size) {
        host.event(Event::RestoringWindowSize { width, height });
        match wait_dota_gone(Duration::from_secs(60)).and_then(|()| game::steam::set_video_size(account, width, height)) {
            Ok(()) => host.event(Event::WindowSizeRestored { width, height }),
            Err(e) => host.event(Event::WindowSizeNotRestored { width, height, error: format!("{e:#}") }),
        }
    }
    match (&result, &restored) {
        (Ok(()), Ok(())) => {
            // 成功就不留排查用的截图和日志
            let _ = std::fs::remove_dir_all(&diag);
            let _ = std::fs::remove_dir(&tmp);
        }
        _ => host.event(Event::DiagnosticsKept { dir: diag.clone() }),
    }
    if let Err(e) = &restored {
        host.event(Event::RestoreFailed { error: format!("{e:#}") });
    }
    drop(hold);
    result
}

#[allow(clippy::too_many_arguments)]
fn record_all(
    host: &dyn Host,
    r: &Replay,
    player: usize,
    team: Team,
    selected: &[Selected],
    p: &Params,
    ff: &Ffmpeg,
    o: &Options,
    s: &mut Session,
    rx: &Receiver<Value>,
    tmp: &Path,
    diag: &Path,
    done: &mut Done,
) -> Result<()> {
    // 只加载一次录像：重新加载会把镜头重置成自由视角
    s.con().send(&format!("playdemo replays/{}", r.match_id))?;
    s.con()
        .wait_for(Duration::from_secs(120), |l| l.contains("OnSwitchLoopModeFinished( game : success )"))
        .context("录像没有加载起来（游戏里有这场录像吗？）")?;
    // 跳转太早会让游戏崩溃（CopyNewEntity），等到这场的 GSI 时钟再动
    let id = r.match_id;
    gsi::wait_for(rx, Duration::from_secs(60), "这场比赛的 GSI 时钟", |st| st.match_id == Some(id) && st.clock.is_some())?;
    host.event(Event::ReplayLoaded);

    // 从这里起用户不能碰鼠标键盘：开着输入检测（含取消快捷键），一动就停下。离开这个函数时卸掉
    let _guard = if o.input_guard {
        let (guard, hotkey) = game::input_guard::start()?;
        if !hotkey {
            host.event(Event::HotkeyUnavailable { hotkey: game::input_guard::HOTKEY.into() });
        }
        Some(guard)
    } else {
        None
    };
    let hotkey = o.input_guard.then(|| game::input_guard::HOTKEY.to_string());
    for secs in (1..=COUNTDOWN).rev() {
        host.event(Event::Countdown { secs, hotkey: hotkey.clone() });
        std::thread::sleep(Duration::from_secs(1));
        cancel::check()?;
    }
    let mut ui = Ui::attach()?;
    ui.set_diag_dir(diag)?;

    let first = r.clock.first().map_or(f32::MIN, |c| c.game_time);
    let last = r.clock.last().map_or(f32::MAX, |c| c.game_time);
    // 先跳到第一段起点前 5 秒设镜头和迷雾（不能直接跳到导出段中点：暂停在同一 tick 时再跳，游戏不输出完成标志）
    let prep = (selected[0].clip.exports[0].span.start - 5.0).max(first);
    clip_builder::goto(s.con(), rx, r.tick_at(prep).context("超出录像范围")?, prep)?;
    {
        let _front = ui.front()?;
        spectator::setup(&mut ui, Panel::Collapsed, team)?;
    }

    let mut outputs = Vec::new();
    for (n, sel) in selected.iter().enumerate() {
        let clip = &sel.clip;
        let name = clips::file_name(r, clip);
        host.event(Event::ClipStarted { n: n + 1, total: selected.len(), index: sel.index, name: name.clone() });
        let mut cuts = Vec::new();
        for (k, e) in clip.exports.iter().enumerate() {
            let from = (e.span.start - p.export_lead).max(first);
            let to = (e.span.end + p.export_margin).min(last);
            let job = Job {
                from,
                to,
                center_tick: r.tick_at((from + to) / 2.0).context("超出录像范围")?,
                window: WINDOW,
                out_dir: tmp,
                background_render: false,
                player,
            };
            host.event(Event::ExportStarted { from, to, estimated_secs: (to - from) * p.render_factor });
            let part = export_checked(host, &mut ui, s, rx, &job, player)?;
            // 裁掉两头多留的部分：起点落在关键帧上（每整秒一个），所以多加一点点，免得落到前一个关键帧
            let cut = tmp.join(format!("cut_{n}_{k}.mp4"));
            ff.cut(&part, (e.span.start - from) as f64 + 0.02, e.span.len() as f64, &cut)?;
            let _ = std::fs::remove_file(&part);
            cuts.push(cut);
        }
        let dest = o.out.join(format!("{name}.mp4"));
        ff.concat(&cuts, &dest)?;
        for c in &cuts {
            let _ = std::fs::remove_file(c);
        }
        host.event(Event::ClipDone { index: sel.index, path: dest.clone(), secs: media::mp4_duration(&dest)? });
        done.files.push(dest.clone());
        done.completed.push(sel.index);
        outputs.push(dest);
    }

    if o.compile && outputs.len() > 1 {
        let hero = r.players[player].hero.trim_start_matches("npc_dota_hero_");
        let dest = o.out.join(format!("{}_{player}_{hero}_合集.mp4", r.match_id));
        ff.concat(&outputs, &dest)?;
        host.event(Event::CompilationDone { path: dest.clone(), secs: media::mp4_duration(&dest)? });
        done.files.push(dest);
    }
    Ok(())
}

/// 导出一段并核对时长；导出失败、文件 0 字节或时长偏差超过容差时重来一次
fn export_checked(host: &dyn Host, ui: &mut Ui, s: &mut Session, rx: &Receiver<Value>, job: &Job, player: usize) -> Result<PathBuf> {
    let planned = (job.to - job.from) as f64;
    let mut last_err = None;
    for attempt in 1..=2 {
        let tried = spectator::focus(player, rx).and_then(|()| clip_builder::export(ui, s.con(), rx, job));
        match tried {
            Ok(exported) => {
                let secs = media::mp4_duration(&exported.path)?;
                if (secs - planned).abs() <= DURATION_TOLERANCE {
                    host.event(Event::Exported { from: job.from, to: job.to, secs, render_secs: exported.render.as_secs_f32() });
                    let part = job.out_dir.join(format!("part_{}.mp4", job.center_tick));
                    std::fs::rename(&exported.path, &part)?;
                    return Ok(part);
                }
                let _ = std::fs::remove_file(&exported.path);
                last_err = Some(anyhow::anyhow!("导出的视频 {secs:.1} 秒，计划 {planned:.1} 秒"));
            }
            Err(e) => last_err = Some(e),
        }
        // 停下了就不重来
        cancel::check()?;
        if attempt == 1 {
            host.event(Event::Retrying { reason: format!("{:#}", last_err.as_ref().unwrap()) });
        }
    }
    Err(last_err.unwrap())
}
