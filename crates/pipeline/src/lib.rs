//! 录制流程：录一个玩家选定的高光片段，每个片段输出一个视频，可选拼成合集。
//! 命令行（`d2hr record`）和桌面应用共用。
//!
//! 用控制台录（2026-11 起，见 docs/prototype-walkthrough.md）：控制台命令切镜头，`startmovie` 逐帧输出画面，
//! 边录边用显卡编码；游戏放在其他窗口后面，不点界面，录制期间用户可以用电脑。
//! 回放每次开始播放 Dota 都会抢一下前台（最小化也会自己恢复），由 `FocusKeeper` 马上还回去。
//! 原先点游戏自带剪辑工具的做法见 docs/phase1-plan.md「录制流程」。
//!
//! 和外界只通过 `Host` 打交道：进度用事件报告，需要用户拍板的事（例如重启游戏）用确认请求。

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clips::{Clip, LEAD};
use game::cancel::{self, Reason};
use game::clip_builder;
use game::gsi;
use game::movie::{self, Movie};
use game::netcon::Netcon;
use game::session::Session;
pub use game::spectator::Camera;
use game::spectator::{self, Team};
use game::ui::Ui;
use media::encode::{Codec, EncodeHow, Encoder, Format};
use media::ffmpeg::Ffmpeg;
use replay::Replay;
use serde::Serialize;
use serde_json::Value;

const NETCON_PORT: u16 = 2121;
/// 录出来的时长和计划长度允许差多少秒，超了就重录一次
const DURATION_TOLERANCE: f64 = 1.0;
/// 录制帧率
const FPS: u32 = 60;
/// 录像每秒的 tick 数
const TICKS_PER_SECOND: f32 = 30.0;
/// `startmovie` 输出文件名的前缀，开始录制前清掉上一轮崩溃留下的
const MOVIE_PREFIX: &str = "d2hr_";
/// 画面中间亮度低于这个就当世界画面没画出来（只有 HUD）。正常约 70–80，黑的约 30（2026-11-08 实测）
const BLACK_BRIGHTNESS: f32 = 45.0;
/// 每段开录后在这几帧（开头丢掉的部分之后）查一次亮度，都低于 `BLACK_BRIGHTNESS` 才算黑
const BLACK_CHECKS: [u32; 3] = [15, 30, 60];

/// 录到的世界画面是黑的（只有 HUD）：重新设好画面、渲染一下，重录这一段
#[derive(Debug)]
struct BlackWorld(f32);

impl std::fmt::Display for BlackWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "录到的画面中间是黑的（亮度 {:.0}），游戏没画出场景", self.0)
    }
}

impl std::error::Error for BlackWorld {}

pub struct Options {
    pub compile: bool,
    pub resolution: Option<(u32, u32)>,
    pub out: PathBuf,
    pub gsi_port: u16,
    /// 全部录完后让游戏退出；中途停下或出错时保留，方便接着录。
    /// 用 `resolution` 启动的游戏无论如何都会退出：它退出时会把这个窗口大小写进显示设置，要等它退出后改回去
    pub quit: bool,
    /// 镜头：英雄追踪或玩家视角
    pub camera: Camera,
    /// 录制期间检测用户在 Dota 里的操作（Dota 在前台时按键、动鼠标）：停止（命令行）或暂停（应用）；None 不检测
    pub on_input: Option<game::input_guard::OnInput>,
    /// 运行日志放在这里（每次录制一个文件，只留最近 20 个）；None 不写。命令行的进度已经打在终端上，不用
    pub log_dir: Option<PathBuf>,
}

/// 把每个事件、每次问答、最后的结果记进日志文件，再转给真正的调用方。出问题时用来还原经过
struct Logged<'a> {
    inner: &'a dyn Host,
    file: Option<std::sync::Arc<std::sync::Mutex<std::fs::File>>>,
    started: Instant,
}

fn write_line(f: &std::sync::Mutex<std::fs::File>, started: Instant, text: &str) {
    use std::io::Write;
    let _ = writeln!(f.lock().unwrap(), "{:8.1}  {text}", started.elapsed().as_secs_f32());
}

impl Logged<'_> {
    fn line(&self, text: &str) {
        if let Some(f) = &self.file {
            write_line(f, self.started, text);
        }
    }

    /// 暂停可能发生在任何一步（包括渲染途中，那时流程不发事件），单开一个线程盯着，把暂停和继续的时间点记下来。
    /// 返回的标记设成 true 时线程退出
    fn watch_pause(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        if let Some(f) = self.file.clone() {
            let (stop, started) = (stop.clone(), self.started);
            std::thread::spawn(move || {
                let mut was = false;
                while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                    let now = cancel::paused();
                    if now != was {
                        was = now;
                        write_line(&f, started, if now { "暂停（检测到在游戏里的操作）" } else { "继续" });
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            });
        }
        stop
    }
}

impl Host for Logged<'_> {
    fn event(&self, e: Event) {
        self.line(&serde_json::to_string(&e).unwrap_or_default());
        self.inner.event(e);
    }

    fn confirm(&self, question: &str) -> bool {
        self.line(&format!("问：{question}"));
        let yes = self.inner.confirm(question);
        self.line(&format!("答：{}", if yes { "同意" } else { "不同意" }));
        yes
    }
}

const KEEP_LOGS: usize = 20;

/// 新建这次的日志文件，删掉多出来的旧日志
fn open_log(dir: &Path) -> Option<std::fs::File> {
    std::fs::create_dir_all(dir).ok()?;
    let mut old: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "log"))
        .collect();
    old.sort();
    while old.len() >= KEEP_LOGS {
        let _ = std::fs::remove_file(old.remove(0));
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    std::fs::File::create(dir.join(format!("record-{stamp}.log"))).ok()
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
    /// 录像加载好了
    ReplayLoaded,
    /// 镜头设好、游戏已放到后台，开始录。`input_guard` 为真时，在 Dota 里操作就会停止或暂停；
    /// `encoder` 是用的编码器，`total_secs` 是要录的画面总长（秒）
    Recording { input_guard: bool, encoder: String, total_secs: f32 },
    /// 暂停了（桌面应用里用户在 Dota 里操作了），等用户点"继续"或"停止"
    Paused,
    /// 用户点了"继续"：重设镜头、把游戏放回后面，再录被打断的那一段（不重新加载录像）
    Resuming,
    /// 开始录第 `n` 个（共 `total` 个）选定的片段，它在计划里是第 `index` 个
    ClipStarted { n: usize, total: usize, index: usize, name: String },
    /// 开始录一段：游戏时间范围和要录的帧数
    SegmentStarted { from: f32, to: f32, frames: u32 },
    /// 这一段录了多少帧（大约每半秒报一次）
    SegmentProgress { frames: u32, total: u32 },
    /// 一段录完：视频时长、用了多少秒墙钟
    SegmentDone { from: f32, to: f32, secs: f64, wall_secs: f32 },
    /// 录得不对，重来一次
    Retrying { reason: String },
    /// 游戏窗口的状态（排查用，界面不显示）：`at` 是在哪一步记的
    GameWindow { at: String, minimized: bool, foreground: bool },
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
    /// 出错时保留了排查用的控制台日志
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
    /// 结束时让游戏退出了（录完且设了 `quit`，或者用了 `resolution`）
    pub game_quit: bool,
    /// 这台机器的录制速度：每秒墙钟录了多少秒画面（没录成一段时为 None）。桌面应用存下来估下次的耗时
    pub speed: Option<f32>,
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
fn prepare(host: &dyn Host, o: &Options, diag: &Path) -> Result<(Netcon, gsi::Listener, bool)> {
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
pub fn run(host: &dyn Host, r: &Replay, player: usize, selected: &[Selected], ff: &Ffmpeg, o: &Options) -> Report {
    let started = Instant::now();
    let log = Logged { inner: host, file: o.log_dir.as_deref().and_then(open_log).map(|f| std::sync::Arc::new(std::sync::Mutex::new(f))), started };
    let pause_watch = log.watch_pause();
    let indices: Vec<usize> = selected.iter().map(|s| s.index).collect();
    log.line(&format!(
        "开始：比赛 {}，玩家 #{player}，片段 {indices:?}，输出到 {}，分辨率 {:?}，镜头 {:?}，合集 {}，录完退出 {}",
        r.match_id,
        o.out.display(),
        o.resolution,
        o.camera,
        o.compile,
        o.quit
    ));
    // 开始时的前台窗口（终端或应用）：不管怎么结束，最后都切回它，用户回来能直接看到结果
    let origin = game::ui::foreground();
    let mut done = Done::default();
    let error = run_inner(&log, r, player, selected, ff, o, origin, &mut done).err();
    pause_watch.store(true, std::sync::atomic::Ordering::SeqCst);
    game::ui::bring_to_front(origin);
    // 停下的原因要在 hold 之外读（restore 期间读不到）
    let cancelled = cancel::reason();
    log.line(&format!(
        "结束：完成 {:?}，文件 {:?}，停下 {:?}，出错 {}，退出游戏 {}，录制速度 {:?}",
        done.completed,
        done.files,
        cancelled.map(|c| c.describe()),
        error.as_ref().map_or("无".into(), |e| format!("{e:#}")),
        done.game_quit,
        done.speed
    ));
    Report {
        files: done.files,
        completed: done.completed,
        cancelled,
        error: if cancelled.is_some() { None } else { error },
        elapsed: started.elapsed(),
        game_quit: done.game_quit,
        speed: done.speed,
    }
}

/// 已经做好的部分
#[derive(Default)]
struct Done {
    files: Vec<PathBuf>,
    completed: Vec<usize>,
    game_quit: bool,
    speed: Option<f32>,
}

#[allow(clippy::too_many_arguments)]
fn run_inner(host: &dyn Host, r: &Replay, player: usize, selected: &[Selected], ff: &Ffmpeg, o: &Options, origin: isize, done: &mut Done) -> Result<()> {
    if selected.is_empty() {
        bail!("没有选定要录的片段");
    }
    let team = Team::from_id(r.players[player].team).context("玩家阵营不是天辉也不是夜魇")?;
    let tmp = o.out.join("tmp");
    let diag = o.out.join("diag");
    std::fs::create_dir_all(&tmp)?;
    std::fs::create_dir_all(&diag)?;
    // 上一轮出错时留下的控制台日志，清掉（排查用的运行日志另存在 log_dir）
    for e in std::fs::read_dir(&diag)?.flatten() {
        let _ = std::fs::remove_file(e.path());
    }
    // tmp 只放我们的中间文件；上一轮程序崩溃时可能还留着，游戏的 movie 目录里也可能留着帧
    for e in std::fs::read_dir(&tmp)?.flatten() {
        if e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("mp4")) {
            let _ = std::fs::remove_file(e.path());
        }
    }
    movie::clean_leftovers(MOVIE_PREFIX);

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
    // 设置守护：离开作用域（包括出错）时恢复，并删掉 GSI 配置。改的设置在 `spectator::console_setup` 里
    let mut s = Session::new(con);
    s.remove_gsi_on_exit();

    let result = record_all(host, r, player, team, selected, ff, o, &mut s, &rx, &tmp, origin, done);
    let restored = s.restore();
    // 下面这些收尾也不能被取消打断
    let hold = cancel::hold();
    // 中途停下或出错时保留游戏，方便接着录（重新启动要多等约 40 秒）；保留的话把窗口恢复（万一被最小化了）
    if (o.quit && result.is_ok()) || sized {
        let _ = s.con().send("quit");
        done.game_quit = true;
    } else if let Ok(w) = game::window::dota_window() {
        game::window::restore(w);
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

/// 录制时共用的东西
struct Rec<'a> {
    host: &'a dyn Host,
    r: &'a Replay,
    rx: &'a Receiver<Value>,
    ff: &'a Ffmpeg,
    codec: Codec,
    player: usize,
    camera: Camera,
    team: Team,
    tmp: &'a Path,
    /// 录像里游戏时钟的范围
    first: f32,
    last: f32,
    /// 录了多少秒画面、用了多少秒墙钟，算这台机器的录制速度
    game_secs: f32,
    wall_secs: f32,
    /// 每段录制的编号（`startmovie` 的文件名前缀）
    seq: u32,
    /// 开始录制时的前台窗口（应用或终端）：游戏最小化后若自己跑到前台，把前台还给它
    origin: isize,
    /// 游戏窗口应有的大小（带 `-w -h` 启动的）；None = 跟用户的设置一致，只等它稳定
    size: Option<(u32, u32)>,
}

#[allow(clippy::too_many_arguments)]
fn record_all(
    host: &dyn Host,
    r: &Replay,
    player: usize,
    team: Team,
    selected: &[Selected],
    ff: &Ffmpeg,
    o: &Options,
    s: &mut Session,
    rx: &Receiver<Value>,
    tmp: &Path,
    origin: isize,
    done: &mut Done,
) -> Result<()> {
    load_replay(host, r, s, rx)?;
    let codec = ff.pick_codec();
    let mut rec = Rec {
        host,
        r,
        rx,
        ff,
        codec,
        player,
        camera: o.camera,
        team,
        tmp,
        first: r.clock.first().map_or(f32::MIN, |c| c.game_time),
        last: r.clock.last().map_or(f32::MAX, |c| c.game_time),
        game_secs: 0.0,
        wall_secs: 0.0,
        seq: 0,
        origin,
        size: o.resolution,
    };
    // 先跳到第一段附近再设镜头（刚加载完镜头是自由视角），然后把游戏放到后面
    let at = (selected[0].clip.start() - 5.0).max(rec.first);
    seek(&rec, s, at)?;
    setup_view(&rec, s)?;

    // 从这里起检测用户在 Dota 里的操作：一动就停止（命令行）或暂停（应用）。离开这个函数时卸掉
    let _guard = match o.on_input {
        Some(mode) => Some(game::input_guard::start(mode)?),
        None => None,
    };
    let _keeper = FocusKeeper::start(origin);
    let total_secs = selected.iter().map(|s| s.clip.duration()).sum();
    host.event(Event::Recording { input_guard: o.on_input.is_some(), encoder: codec.name().into(), total_secs });

    let mut outputs = Vec::new();
    let result = (|| -> Result<()> {
        for (n, sel) in selected.iter().enumerate() {
            let clip = &sel.clip;
            let name = clips::file_name(r, clip);
            host.event(Event::ClipStarted { n: n + 1, total: selected.len(), index: sel.index, name: name.clone() });
            let mut parts = Vec::new();
            for span in &clip.spans {
                parts.push(record_span(&mut rec, s, span.start, span.end)?);
            }
            let dest = o.out.join(format!("{name}.mp4"));
            ff.concat(&parts, &dest)?;
            for p in &parts {
                let _ = std::fs::remove_file(p);
            }
            host.event(Event::ClipDone { index: sel.index, path: dest.clone(), secs: media::mp4_duration(&dest)? });
            done.files.push(dest.clone());
            done.completed.push(sel.index);
            outputs.push(dest);
        }
        Ok(())
    })();
    if rec.wall_secs > 0.0 {
        done.speed = Some(rec.game_secs / rec.wall_secs);
    }
    result?;

    if o.compile && outputs.len() > 1 {
        let hero = r.players[player].hero.trim_start_matches("npc_dota_hero_");
        let dest = o.out.join(format!("{}_{player}_{hero}_合集.mp4", r.match_id));
        ff.concat(&outputs, &dest)?;
        host.event(Event::CompilationDone { path: dest.clone(), secs: media::mp4_duration(&dest)? });
        done.files.push(dest);
    }
    Ok(())
}

/// 加载录像，等到能跳转为止。重新加载会把镜头重置成自由视角
fn load_replay(host: &dyn Host, r: &Replay, s: &mut Session, rx: &Receiver<Value>) -> Result<()> {
    s.con().send(&format!("playdemo replays/{}", r.match_id))?;
    s.con()
        .wait_for(Duration::from_secs(120), |l| l.contains("OnSwitchLoopModeFinished( game : success )"))
        .context("录像没有加载起来（游戏里有这场录像吗？）")?;
    // 跳转太早会让游戏崩溃（CopyNewEntity），等到这场的 GSI 时钟再动
    let id = r.match_id;
    gsi::wait_for(rx, Duration::from_secs(60), "这场比赛的 GSI 时钟", |st| st.match_id == Some(id) && st.clock.is_some())?;
    host.event(Event::ReplayLoaded);
    Ok(())
}

/// 用控制台命令设镜头、迷雾、收面板、隐藏鼠标指针，然后把游戏最小化（照样出帧，用户也不会误点进去）。
/// 暂停后继续时再调一次：用户可能在游戏里点过东西、把窗口恢复了
fn setup_view(rec: &Rec, s: &mut Session) -> Result<()> {
    // 游戏放在其他窗口后面（不最小化：回放一播放它就自己恢复、抢前台，2026-11-08 实测）。
    // 用户暂停时可能把窗口挪到了前面，这里放回去；等窗口大小稳定，设好镜头、渲染 2 秒再录
    // （一直最小化时偶尔录到黑的世界画面，原因没查清；万一还是黑的，`capture` 会查出来重录）
    let dota = game::window::dota_window()?;
    game::window::restore(dota);
    game::window::send_to_back(dota);
    if game::ui::foreground() == dota.0 as isize {
        game::ui::bring_to_front(rec.origin);
    }
    wait_window_settled(rec.size);
    spectator::console_setup(s, rec.player, rec.camera, rec.team)?;
    std::thread::sleep(Duration::from_secs(2));
    window_state(rec, "设好画面后");
    Ok(())
}

/// 录制期间盯着前台：程序刚让回放播放、Dota 抢走了前台，就把前台还给开始录制时的窗口、把 Dota 放回后面。
/// 用户自己点进 Dota（不在那个时间窗里）不管，交给输入检测暂停
struct FocusKeeper {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FocusKeeper {
    fn start(origin: isize) -> FocusKeeper {
        use std::sync::atomic::Ordering;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                if game::window::focus_steal_expected()
                    && let Ok(dota) = game::window::dota_window()
                    && game::ui::foreground() == dota.0 as isize
                {
                    game::ui::bring_to_front(origin);
                    game::window::send_to_back(dota);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        FocusKeeper { stop, thread: Some(thread) }
    }
}

impl Drop for FocusKeeper {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// 把游戏窗口现在是否最小化、是否在前台记进日志（2026-11-08：用户问录制时是不是真的最小化了，之前没核实过）
fn window_state(rec: &Rec, at: &str) {
    if let Ok(dota) = game::window::dota_window() {
        rec.host.event(Event::GameWindow {
            at: at.into(),
            minimized: game::window::is_minimized(dota),
            foreground: game::ui::foreground() == dota.0 as isize,
        });
    }
}

/// 等游戏窗口大小稳定（1 秒内不变；给了 `want` 的话还要等于它），最多等 15 秒
fn wait_window_settled(want: Option<(u32, u32)>) {
    let started = Instant::now();
    let mut last = None;
    let mut since = Instant::now();
    while started.elapsed() < Duration::from_secs(15) {
        let now = client_size();
        if now != last {
            last = now;
            since = Instant::now();
        } else if now.is_some() && want.is_none_or(|w| now == Some(w)) && since.elapsed() >= Duration::from_secs(1) {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// 跳到游戏时钟 `at` 并暂停，重新追踪玩家：跳转会取消选中英雄，底部技能栏、装备栏就空了，
/// 要补按追踪热键（PostMessage，游戏最小化也行；2026-11-07 实测）
fn seek(rec: &Rec, s: &mut Session, at: f32) -> Result<()> {
    clip_builder::goto(s.con(), rec.rx, rec.r.tick_at(at).context("超出录像范围")?, at)?;
    s.con().send(&format!("dota_spectator_hero_index {}", rec.player))?;
    s.con().send(&format!("dota_camera_focus_player {}", rec.player))?;
    spectator::focus(rec.player, rec.rx)
}

/// 录一段，核对时长；失败重来一次。
/// 暂停（桌面应用里用户在 Dota 里操作了）不算失败：这一段丢掉，等用户点"继续"，重设镜头、把游戏放回后面，再录这一段（不重新加载录像）
fn record_span(rec: &mut Rec, s: &mut Session, from: f32, to: f32) -> Result<PathBuf> {
    let mut failures = 0;
    let mut black = false;
    loop {
        if cancel::paused() {
            rec.host.event(Event::Paused);
            cancel::wait_while_paused()?;
            rec.host.event(Event::Resuming);
        }
        let mut tried = Ok(());
        // 暂停过（用户可能在游戏里点过东西、把窗口挪到了前面），或者上一次录到的画面是黑的：重设镜头、放回后面
        if cancel::take_pause_seen() || black {
            tried = setup_view(rec, s);
        }
        let err = match tried.and_then(|()| capture(rec, s, from, to)) {
            Ok(part) => return Ok(part),
            Err(e) => e,
        };
        black = err.downcast_ref::<BlackWorld>().is_some();
        // 停下了就不重来；被暂停打断的不算一次失败（用户可能已经点了"继续"，所以看暂停过没有，不只看现在是不是暂停着；
        // 2026-11-08 实测：继续后重设镜头期间又被检测到操作，等发现时已经又点了继续，被当成了一次失败）
        cancel::check()?;
        if cancel::paused() || cancel::pause_seen() {
            continue;
        }
        failures += 1;
        if failures >= 2 {
            return Err(err);
        }
        rec.host.event(Event::Retrying { reason: format!("{err:#}") });
    }
}

/// 录游戏时钟 `from`–`to` 这一段：跳到 `from` 前 `LEAD` 秒，`startmovie` 逐帧输出 TGA，边出帧边喂给编码器，
/// 开头 `LEAD` 秒丢掉（第一帧上有"回放已暂停"字样），最后合进声音。返回带声音的 mp4
fn capture(rec: &mut Rec, s: &mut Session, from: f32, to: f32) -> Result<PathBuf> {
    let start = (from - LEAD).max(rec.first);
    let end = to.min(rec.last);
    seek(rec, s, start)?;
    let (t0, t1) = (rec.r.tick_at(start).context("超出录像范围")?, rec.r.tick_at(end).context("超出录像范围")?);
    let frames = |ticks: u32| (ticks as f32 / TICKS_PER_SECOND * FPS as f32).round() as u32;
    let expected = frames(t1 - t0);
    let skip = ((from.max(start) - start) * FPS as f32).round() as u32;
    rec.seq += 1;
    let prefix = format!("{MOVIE_PREFIX}{}_{}", rec.r.match_id, rec.seq);
    let video = rec.tmp.join(format!("{prefix}.mp4"));
    let mut movie = Movie::start(s.con(), &prefix, FPS)?;

    let result = (|| -> Result<PathBuf> {
        // 万一我们读帧慢了，回放也停在终点，不会多录
        s.con().send(&format!("demo_pauseatservertick {t1}"))?;
        game::window::expect_focus_steal();
        s.con().send("demo_resume")?;
        rec.host.event(Event::SegmentStarted { from, to: end, frames: expected - skip });
        window_state(rec, "开录");
        let started = Instant::now();
        // 正常是实时的 0.5–0.8 倍；按 0.1 倍算还没录完就当卡住了
        let timeout = Duration::from_secs_f32((end - start) / 0.1 + 60.0);
        let mut reported = Instant::now();
        let mut enc: Option<Encoder> = None;
        let mut dark = Vec::new();
        while movie.taken() < expected {
            // 用户在 Dota 里操作了（暂停）或点了停止：这一段不要了
            cancel::check_not_paused()?;
            let Some(frame) = movie.take()? else {
                if started.elapsed() > timeout {
                    bail!("等了 {timeout:?} 只录了 {} 帧（共 {expected} 帧），游戏卡住了？", movie.taken());
                }
                std::thread::sleep(Duration::from_millis(2));
                continue;
            };
            let n = movie.taken() - 1;
            if n < skip {
                continue;
            }
            // 世界画面有没有画出来：开头几帧都暗得不正常就不录了
            if let Some(k) = BLACK_CHECKS.iter().position(|&c| n == skip + c) {
                dark.push(frame.center_brightness());
                if k + 1 == BLACK_CHECKS.len() && dark.iter().all(|&b| b < BLACK_BRIGHTNESS) {
                    let max = dark.iter().copied().fold(0.0, f32::max);
                    return Err(BlackWorld(max).into());
                }
            }
            let e = match &mut enc {
                Some(e) => e,
                None => {
                    let f = Format { width: frame.width, height: frame.height, fps: FPS, bottom_up: frame.bottom_up };
                    enc.insert(rec.ff.encoder(rec.codec, f, &video, EncodeHow::default())?)
                }
            };
            e.write(&frame.bgra)?;
            if reported.elapsed() > Duration::from_millis(500) {
                rec.host.event(Event::SegmentProgress { frames: n + 1 - skip, total: expected - skip });
                reported = Instant::now();
            }
        }
        let wall = started.elapsed().as_secs_f32();
        movie.end(s.con())?;
        s.con().send("demo_pause")?;
        // 数够了之后、endmovie 之前多出的帧不要
        while movie.take()?.is_some() {}
        enc.context("一帧都没录到")?.finish()?;
        let part = rec.tmp.join(format!("{prefix}_av.mp4"));
        rec.ff.mux_audio(&video, &movie.wav(), skip as f32 / FPS as f32, &part)?;
        let secs = media::mp4_duration(&part)?;
        let planned = (end - from) as f64;
        if (secs - planned).abs() > DURATION_TOLERANCE {
            let _ = std::fs::remove_file(&part);
            bail!("录出来 {secs:.1} 秒，计划 {planned:.1} 秒");
        }
        rec.game_secs += (end - start).max(0.0);
        rec.wall_secs += wall;
        rec.host.event(Event::SegmentDone { from, to: end, secs, wall_secs: wall });
        Ok(part)
    })();

    if result.is_err() {
        // 中途放弃：停掉录制、暂停回放（编码进程在 Encoder 被丢掉时就结束了）
        if !movie.ended() {
            let _ = movie.end(s.con());
        }
        let _ = s.con().send("demo_pause");
    }
    movie.remove();
    let _ = std::fs::remove_file(&video);
    result
}
