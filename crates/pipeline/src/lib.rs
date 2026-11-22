//! 录制流程：录一个玩家选定的高光片段，每个片段输出一个视频，可选拼成合集。
//! 命令行（`d2hr record`）和桌面应用共用。
//!
//! 用控制台录（2026-11 起，见 docs/prototype-walkthrough.md）：控制台命令切镜头，`startmovie` 逐帧输出画面，
//! 边录边用显卡编码；不点界面。游戏放在最前面录（2026-11-09 定，见 `setup_view`），
//! 用户发给 Dota 的点击和按键被拦下（`game::input_guard`），怎么操作都不会打乱录制。
//! 原先点游戏自带剪辑工具的做法见 docs/phase1-plan.md「录制流程」。
//!
//! 和外界只通过 `Host` 打交道：进度用事件报告，需要用户拍板的事（例如重启游戏）用确认请求。

pub mod applog;

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
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

/// 录制失败的几类常见原因（2026-11-21 起）：挂在出错的那一步上（`.context(Fail::X)`），桌面应用按它显示翻译好的说明，
/// 原始报错留在详细信息和日志里。只挂在最里面出错的地方：外层再挂一个的话，认出来的是外层那个
#[derive(Debug, Clone, Copy)]
pub enum Fail {
    /// Steam 没开、没登录，或者用户不让打开
    Steam,
    /// 要重启游戏，用户没同意
    RestartDeclined,
    /// 游戏没能启动、控制台连不上
    GameStart,
    /// 录像没加载起来
    ReplayLoad,
    /// 跳转、出帧时游戏没反应
    GameStuck,
    /// 编码、合成视频出错
    Encode,
    /// 输出目录写不了
    Disk,
}

impl Fail {
    /// 语言文件里的 `rec.fail.{key}`
    pub fn key(self) -> &'static str {
        match self {
            Fail::Steam => "steam",
            Fail::RestartDeclined => "restart_declined",
            Fail::GameStart => "game_start",
            Fail::ReplayLoad => "replay_load",
            Fail::GameStuck => "game_stuck",
            Fail::Encode => "encode",
            Fail::Disk => "disk",
        }
    }
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(match self {
            Fail::Steam => "Steam 没有登录",
            Fail::RestartDeclined => "没有同意重启游戏",
            Fail::GameStart => "游戏没能启动",
            Fail::ReplayLoad => "录像没能加载",
            Fail::GameStuck => "游戏没有反应",
            Fail::Encode => "视频编码出错",
            Fail::Disk => "输出目录无法写入",
        })
    }
}

/// 这个错误属于哪一类（`Fail::key`，录到黑画面是 "black_screen"）；认不出来是 None
pub fn fail_kind(e: &anyhow::Error) -> Option<&'static str> {
    e.downcast_ref::<Fail>().map(|f| f.key()).or_else(|| e.downcast_ref::<BlackWorld>().map(|_| "black_screen"))
}

/// 要用户拍板的问题（2026-11-21 起）：`text` 是中文原句（命令行、日志用），
/// `key` / `params` 给桌面应用按语言翻译（语言文件里的键和 `{占位符}`）
pub struct Ask {
    pub key: &'static str,
    pub params: Value,
    pub text: String,
}

/// 录完后要做的一个成品：用哪几段（`selected` 里的位置），文件名（不含扩展名）。
/// 只有一段的直接用那段的文件（名字不同就复制一份），两段以上按顺序拼起来
#[derive(Debug, Clone)]
pub struct Output {
    pub name: String,
    pub parts: Vec<usize>,
}

pub struct Options {
    /// 录完后要做的成品（集锦、改名的单个时刻）；空 = 只要每段各自的文件
    pub outputs: Vec<Output>,
    /// 成品都做好后，还留着每段各自的文件。桌面应用不留（2026-11-18 用户定：拼完就删单段）；命令行留
    pub keep_parts: bool,
    pub resolution: Option<(u32, u32)>,
    pub out: PathBuf,
    pub gsi_port: u16,
    /// 全部录完后让游戏退出；中途停下或出错时保留，方便接着录。
    /// 用 `resolution` 启动的游戏无论如何都会退出：它退出时会把这个窗口大小写进显示设置，要等它退出后改回去
    pub quit: bool,
    /// 镜头：英雄追踪或玩家视角
    pub camera: Camera,
    /// 录制期间拦下用户发给 Dota 的点击和按键（`game::input_guard`）
    pub block_input: bool,
    /// 结束后把开始时的前台窗口切回前台（模拟按一下 Alt 再切）。命令行要（切回终端）；桌面应用不要：
    /// Dota 退出后应用窗口自己就回到前台了，再切一次之后页面和后台之间的消息就卡住、要点一下窗口才通
    /// （2026-11-18/19 三次录制都是这样，单独重放这一步又复现不了，原因没查清）
    pub return_focus: bool,
}

/// 录制的进度每段每这么多秒记一行（游戏每 0.5 秒报一次，全记的话一次录制两三百行，挤掉应用日志里别的内容）
const PROGRESS_LOG_EVERY: Duration = Duration::from_secs(5);

/// 把每个事件、每次问答、最后的结果记进应用日志（`applog`，类别「录制」），再转给真正的调用方。出问题时用来还原经过
struct Logged<'a> {
    inner: &'a dyn Host,
    /// 这一段上次记进度的时间；None = 这一段还没记过
    last_progress: std::sync::Mutex<Option<Instant>>,
}

fn line(text: &str) {
    applog::write("录制", text);
}

impl Logged<'_> {
    /// 用户的操作随时可能被拦下（流程不发事件），单开一个线程盯着，把拦下的时间点记下来，排查"录到的画面被动过"用。
    /// 返回的标记设成 true 时线程退出
    fn watch_blocked(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop2 = stop.clone();
        std::thread::spawn(move || {
            let mut was = game::input_guard::blocked();
            while !stop2.load(std::sync::atomic::Ordering::SeqCst) {
                let now = game::input_guard::blocked();
                if now != was {
                    was = now;
                    line(&format!("拦下了发给 Dota 的点击或按键（共 {now} 次）"));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        stop
    }

    /// 这个事件要不要记：进度每段每 `PROGRESS_LOG_EVERY` 记一次，别的都记
    fn worth_logging(&self, e: &Event) -> bool {
        let mut last = self.last_progress.lock().unwrap_or_else(|e| e.into_inner());
        match e {
            Event::SegmentStarted { .. } => {
                *last = None;
                true
            }
            Event::SegmentProgress { .. } => {
                let due = last.is_none_or(|t| t.elapsed() >= PROGRESS_LOG_EVERY);
                if due {
                    *last = Some(Instant::now());
                }
                due
            }
            _ => true,
        }
    }
}

impl Host for Logged<'_> {
    fn event(&self, e: Event) {
        if self.worth_logging(&e) {
            line(&serde_json::to_string(&e).unwrap_or_default());
        }
        self.inner.event(e);
    }

    fn confirm(&self, ask: &Ask) -> bool {
        line(&format!("问：{}", ask.text));
        let yes = self.inner.confirm(ask);
        line(&format!("答：{}", if yes { "同意" } else { "不同意" }));
        yes
    }
}

/// 选定的片段和它在 `plan` 里的序号（从 1 开始）
pub struct Selected {
    pub index: usize,
    pub clip: Clip,
    /// 录成的文件叫什么（不含扩展名，`clips::file_name`；语言、英雄显示名由调用方定）
    pub name: String,
}

/// 录制过程中的进度
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// Dota 以普通模式开着，等用户自己退出
    WaitingForDotaToQuit,
    /// Steam 没开或没登录，用户同意后打开了 Steam，等它登录好
    WaitingForSteam,
    /// 启动游戏，带这些启动参数
    LaunchingGame { args: Vec<String> },
    /// 录像加载好了
    ReplayLoaded,
    /// 镜头设好、游戏已放到最前面，开始录。`block_input` 为真时，发给 Dota 的点击和按键会被拦下；
    /// `encoder` 是用的编码器，`total_secs` 是要录的画面总长（秒）
    Recording { block_input: bool, encoder: String, total_secs: f32 },
    /// 开始录第 `n` 个（共 `total` 个）选定的片段，它在计划里是第 `index` 个
    ClipStarted { n: usize, total: usize, index: usize, name: String },
    /// 开始录一段：游戏时间范围和要录的帧数
    SegmentStarted { from: f32, to: f32, frames: u32 },
    /// 这一段录了多少帧（大约每半秒报一次）
    SegmentProgress { frames: u32, total: u32 },
    /// 一段录完：视频时长、用了多少秒墙钟
    SegmentDone { from: f32, to: f32, secs: f64, wall_secs: f32 },
    /// 录得不对，重来一次
    /// `fail`：认得出的失败种类（`fail_kind`），界面按它翻译；认不出就显示 `reason` 原文
    Retrying { reason: String, fail: Option<&'static str> },
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
    fn confirm(&self, ask: &Ask) -> bool;
}

/// 录制结果。出错或停下时也会给出已经做好的部分，方便从没录完的片段继续
#[derive(Debug)]
pub struct Report {
    /// 录好的每段的文件，和 `completed` 一一对应（`removed` 里的已经删了）
    pub files: Vec<PathBuf>,
    /// 已经做好的片段序号（计划里的序号，从 1 开始）
    pub completed: Vec<usize>,
    /// 做好的成品：(`Options::outputs` 里的位置, 文件)
    pub products: Vec<(usize, PathBuf)>,
    /// 成品做好后删掉的单段文件（`keep_parts` 为 false 时）
    pub removed: Vec<PathBuf>,
    /// 停下的原因；正常录完或出错是 None
    pub cancelled: Option<Reason>,
    pub error: Option<anyhow::Error>,
    pub elapsed: Duration,
    /// 结束时让游戏退出了（录完且设了 `quit`，或者用了 `resolution`）
    pub game_quit: bool,
    /// 这台机器的录制速度：每秒墙钟录了多少秒画面（没录成一段时为 None）。桌面应用存下来估下次的耗时
    pub speed: Option<f32>,
    /// 一共录了多少秒画面（算 `speed` 用的那些）。录得太少时速度不准，桌面应用就不存
    pub recorded_secs: f32,
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

/// 让还在启动的游戏退出：每隔几秒连一次控制台发 `quit`，直到游戏窗口没了，返回它退没退。
/// 启动中途（还在加载）收到的 `quit` 游戏不理：2026-11-20 启动 7 秒时点了停止，发了一次 `quit`，
/// 游戏一直没退，60 秒后放弃，显示设置也没改回去；加载完再发一次 2 秒就退了
fn quit_until_gone(timeout: Duration) -> bool {
    let started = Instant::now();
    while dota_running() {
        if started.elapsed() > timeout {
            return false;
        }
        let _ = Netcon::connect(NETCON_PORT).and_then(|mut c| c.send("quit"));
        std::thread::sleep(Duration::from_secs(5));
    }
    true
}

/// 让带 netcon 的游戏退出（先征得同意）。`ask` 里说为什么要重启
fn restart_needed(host: &dyn Host, ask: Ask) -> Result<()> {
    if !host.confirm(&ask) {
        return Err(anyhow!("用户没同意重启游戏").context(Fail::RestartDeclined));
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
/// 准备好游戏：要启动（或重启）的话带上分辨率启动，连上控制台。带 `-w -h` 启动了就把 `sized` 设成 true——
/// 出错或停下时调用方也要靠它决定让游戏退出、改回窗口大小
/// 第四个返回值是拦截输入（`block_input` 时）：游戏窗口一出现就开始拦（2026-11-18 用户定）。原来是镜头设好、开录时才拦，
/// 启动、加载录像那一两分钟里按键照样进游戏，用户按了打开控制台的键，控制台被录进了视频。
/// 游戏自己弹窗的话会点不掉，但 Dota 左上角的进度窗不在拦截范围里，那里的「停止录制」照样能点
fn prepare(host: &dyn Host, o: &Options, diag: &Path, sized: &mut bool) -> Result<(Netcon, gsi::Listener, Option<game::input_guard::Guard>)> {
    gsi::check_port(o.gsi_port).context(Fail::GameStart)?;
    let changed = gsi::install(o.gsi_port).context(Fail::GameStart)?;
    let rx = gsi::listen(o.gsi_port).context(Fail::GameStart)?;

    if dota_running() && !game::launch::netcon_ready(NETCON_PORT) {
        host.event(Event::WaitingForDotaToQuit);
        wait_dota_gone(Duration::from_secs(600)).context(Fail::GameStart)?;
    }
    if dota_running() {
        // 配置刚写入：除非游戏启动时就有同样的配置（上一轮写过、结束时删了），否则收不到包。
        // 暂停、尤其是游戏在后台时几秒才一个包，等久一点
        if changed && gsi::wait_for(&rx, Duration::from_secs(15), "GSI 包", |_| true).is_err() {
            restart_needed(
                host,
                Ask {
                    key: "confirm.restartGsi",
                    params: Value::Null,
                    text: "游戏开着，但没读到我们的 GSI 配置（配置只在游戏启动时读）。现在让游戏退出，再重新启动？".into(),
                },
            )?;
        } else if let (Some(want), Some(now)) = (o.resolution, client_size())
            && want != now
        {
            let (now, want) = (format!("{}×{}", now.0, now.1), format!("{}×{}", want.0, want.1));
            restart_needed(
                host,
                Ask {
                    key: "confirm.restartSize",
                    text: format!("游戏窗口现在是 {now}，要录 {want} 得重新启动游戏。现在让游戏退出，再重新启动？"),
                    params: serde_json::json!({ "now": now, "want": want }),
                },
            )?;
        }
    }
    let mut guard = None;
    if !dota_running() {
        ensure_steam(host)?;
        let account = game::steam::me().context("认不出 Steam 账号（Steam 没登录？）").context(Fail::Steam)?;
        // 带上用户在 Steam 里设的启动选项（例如国服的 -perfectworld），再加分辨率
        let mut extra = game::steam::launch_options(account).unwrap_or_default();
        if let Some((w, h)) = o.resolution {
            extra.extend(["-w".into(), w.to_string(), "-h".into(), h.to_string()]);
            *sized = true;
        }
        host.event(Event::LaunchingGame { args: extra.clone() });
        game::launch::launch(NETCON_PORT, &extra).context(Fail::GameStart)?;
        guard = guard_when_window(o, Duration::from_secs(180)).context(Fail::GameStart)?;
        game::launch::wait_netcon(NETCON_PORT, Duration::from_secs(180)).context(Fail::GameStart)?;
    }
    // 游戏本来就开着（上一轮停下后留着的）：现在就拦
    if guard.is_none() {
        guard = guard_when_window(o, Duration::from_secs(10)).context(Fail::GameStart)?;
    }
    let mut con = Netcon::connect_logged(NETCON_PORT, &diag.join("console.log")).context(Fail::GameStart)?;
    con.wait_responsive(Duration::from_secs(120)).context(Fail::GameStart)?;
    con.drain(Duration::from_millis(300), Duration::from_secs(2));
    Ok((con, rx, guard))
}

/// 准备好带 netcon 的游戏，给 `d2hr watch`（整局放录像，不录视频）用：同 `prepare`，但不改窗口大小、不拦输入。
/// 出错时删掉刚写的 GSI 配置；成功后由调用方负责（`Session::remove_gsi_on_exit`）
pub fn prepare_plain(host: &dyn Host, gsi_port: u16, diag: &Path) -> Result<(Netcon, gsi::Listener)> {
    let o = Options {
        outputs: Vec::new(),
        keep_parts: true,
        resolution: None,
        out: diag.to_path_buf(),
        gsi_port,
        quit: false,
        camera: Camera::HeroChase,
        block_input: false,
        return_focus: false,
    };
    let mut sized = false;
    match prepare(host, &o, diag, &mut sized) {
        Ok((con, rx, _)) => Ok((con, rx)),
        Err(e) => {
            let _ = gsi::remove();
            Err(e)
        }
    }
}

/// 启动 Dota 前要 Steam 开着并登录了，不然游戏不发游戏状态（GSI），等一分多钟才报错（2026-11-18 用户遇到）。
/// 没开就问用户：同意就打开 Steam、等登录好（最多 5 分钟，可以停止）；不同意就停下，说清楚要先开 Steam
fn ensure_steam(host: &dyn Host) -> Result<()> {
    if game::steam::signed_in() {
        return Ok(());
    }
    let ask = Ask {
        key: "confirm.steam",
        params: Value::Null,
        text: "Steam 没在运行（或者没登录）。录制要 Steam 登录着才能读到比赛数据，要现在打开 Steam 吗？".into(),
    };
    if !host.confirm(&ask) {
        return Err(anyhow!("Steam 没在运行，请先打开 Steam 并登录，再开始录制").context(Fail::Steam));
    }
    game::steam::start().context(Fail::Steam)?;
    host.event(Event::WaitingForSteam);
    let started = Instant::now();
    while !game::steam::signed_in() {
        cancel::check()?;
        if started.elapsed() > Duration::from_secs(300) {
            return Err(anyhow!("等了 5 分钟 Steam 还没登录好，请登录 Steam 后再开始录制").context(Fail::Steam));
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    // 刚登录完 Steam 还在初始化，马上启动游戏可能还是连不上，多等一会儿
    std::thread::sleep(Duration::from_secs(5));
    Ok(())
}

/// 等游戏窗口出现（最多 `timeout`），出现了就开始拦截输入；不拦（`block_input` 为假）返回 None
fn guard_when_window(o: &Options, timeout: Duration) -> Result<Option<game::input_guard::Guard>> {
    if !o.block_input {
        return Ok(None);
    }
    let started = Instant::now();
    while !dota_running() {
        cancel::check()?;
        if started.elapsed() > timeout {
            bail!("等了 {timeout:?} 游戏窗口还没出来");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    game::input_guard::start().map(Some)
}

/// 把显示设置里的窗口大小改回 `original`（要等带 `-w -h` 启动的游戏退出后）
fn restore_window_size(host: &dyn Host, account: Option<u32>, original: Option<(u32, u32)>) {
    if let (Some(account), Some((width, height))) = (account, original) {
        host.event(Event::RestoringWindowSize { width, height });
        match wait_dota_gone(Duration::from_secs(60)).and_then(|()| game::steam::set_video_size(account, width, height)) {
            Ok(()) => host.event(Event::WindowSizeRestored { width, height }),
            Err(e) => host.event(Event::WindowSizeNotRestored { width, height, error: format!("{e:#}") }),
        }
    }
}

/// 录制。调用方负责在开始前清掉上一轮的取消标记（`game::cancel::reset`），命令行另外负责接管 Ctrl+C
pub fn run(host: &dyn Host, r: &Replay, player: usize, selected: &[Selected], ff: &Ffmpeg, o: &Options) -> Report {
    let started = Instant::now();
    let log = Logged { inner: host, last_progress: std::sync::Mutex::new(None) };
    let blocked_watch = log.watch_blocked();
    let indices: Vec<usize> = selected.iter().map(|s| s.index).collect();
    line(&format!(
        "开始：比赛 {}，玩家 #{player}，片段 {indices:?}，输出到 {}，分辨率 {:?}，镜头 {:?}，成品 {:?}，留单段 {}，录完退出 {}",
        r.match_id,
        o.out.display(),
        o.resolution,
        o.camera,
        o.outputs.iter().map(|x| (&x.name, &x.parts)).collect::<Vec<_>>(),
        o.keep_parts,
        o.quit
    ));
    // 开始时的前台窗口（终端）：不管怎么结束，最后都切回它，用户回来能直接看到结果
    let origin = game::ui::foreground();
    let mut done = Done::default();
    let error = run_inner(&log, r, player, selected, ff, o, &mut done).err();
    blocked_watch.store(true, std::sync::atomic::Ordering::SeqCst);
    if o.return_focus {
        let back = game::ui::bring_to_front(origin);
        line(&format!("切回开始时的前台窗口：{}", if back { "成功" } else { "没切成" }));
    }
    // 停下的原因要在 hold 之外读（restore 期间读不到）
    let cancelled = cancel::reason();
    line(&format!(
        "结束：完成 {:?}，文件 {:?}，成品 {:?}，删掉的单段 {:?}，停下 {:?}，出错 {}，退出游戏 {}，录制速度 {:?}",
        done.completed,
        done.files,
        done.products,
        done.removed,
        cancelled.map(|c| c.describe()),
        error.as_ref().map_or("无".into(), |e| format!("{e:#}")),
        done.game_quit,
        done.speed
    ));
    Report {
        files: done.files,
        completed: done.completed,
        products: done.products,
        removed: done.removed,
        cancelled,
        error: if cancelled.is_some() { None } else { error },
        elapsed: started.elapsed(),
        game_quit: done.game_quit,
        speed: done.speed,
        recorded_secs: done.recorded_secs,
    }
}

/// 已经做好的部分
#[derive(Default)]
struct Done {
    files: Vec<PathBuf>,
    completed: Vec<usize>,
    products: Vec<(usize, PathBuf)>,
    removed: Vec<PathBuf>,
    game_quit: bool,
    speed: Option<f32>,
    recorded_secs: f32,
}

#[allow(clippy::too_many_arguments)]
fn run_inner(host: &dyn Host, r: &Replay, player: usize, selected: &[Selected], ff: &Ffmpeg, o: &Options, done: &mut Done) -> Result<()> {
    if selected.is_empty() {
        bail!("没有选定要录的片段");
    }
    let team = Team::from_id(r.players[player].team).context("玩家阵营不是天辉也不是夜魇")?;
    let tmp = o.out.join("tmp");
    let diag = o.out.join("diag");
    std::fs::create_dir_all(&tmp).context(Fail::Disk)?;
    std::fs::create_dir_all(&diag).context(Fail::Disk)?;
    // 上一轮出错时留下的控制台日志，清掉（排查用的运行日志在应用日志里，见 `applog`）
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
    let mut sized = false;
    // 拦截输入从游戏窗口出现一直开到这个函数结束（录完、恢复设置、让游戏退出之后才卸掉）
    let (con, rx, _guard) = match prepare(host, o, &diag, &mut sized) {
        Ok(v) => v,
        Err(e) => {
            // 还没开始改设置，只要删掉刚写的 GSI 配置
            let _ = gsi::remove();
            // 已经带 -w -h 启动了游戏：留着的话，它退出时会把这个大小写进显示设置，要让它退出、再改回原来的大小。
            // 2026-11-10 实测：启动中点了停止，游戏留着，之后它退出时写进了 1920×1080，下一轮还把它当成了"原来的大小"
            if sized {
                let _hold = cancel::hold();
                done.game_quit = quit_until_gone(Duration::from_secs(180));
                restore_window_size(host, account, original_size);
            }
            return Err(e);
        }
    };
    // 设置守护：离开作用域（包括出错）时恢复，并删掉 GSI 配置。改的设置在 `spectator::hide_player_names` 和 `console_setup` 里
    let mut s = Session::new(con);
    s.remove_gsi_on_exit();

    let result = record_all(host, r, player, team, selected, ff, o, &mut s, &rx, &tmp, done);
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
    if sized {
        restore_window_size(host, account, original_size);
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
        // 号角前的片段也能录：最早跳到录像开头（`Replay::earliest_time`）
        first: r.earliest_time(),
        last: r.clock.last().map_or(f32::MAX, |c| c.game_time),
        game_secs: 0.0,
        wall_secs: 0.0,
        seq: 0,
        size: o.resolution,
    };
    // 先跳到第一段附近再设镜头（刚加载完镜头是自由视角），然后把游戏放到最前面
    let at = (selected[0].clip.start() - 5.0).max(rec.first);
    seek(&rec, s, at)?;
    setup_view(&rec, s)?;

    // 拦截输入在游戏窗口一出现时就开了（`prepare`），这里不再另开
    let total_secs = selected.iter().map(|s| s.clip.duration()).sum();
    host.event(Event::Recording { block_input: o.block_input, encoder: codec.name().into(), total_secs });

    let mut outputs = Vec::new();
    let result = (|| -> Result<()> {
        for (n, sel) in selected.iter().enumerate() {
            let clip = &sel.clip;
            let name = &sel.name;
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
        done.recorded_secs = rec.game_secs;
    }
    result?;

    make_outputs(host, ff, o, &outputs, done).context(Fail::Encode)
}

/// 全部段录完后做成品（2026-11-18 起：比赛页「推荐」的集锦和单个时刻，「自定义」的自定义集锦）。
/// 一个成品只有一段时复制那段（同名就不用动），多段按顺序拼。全部做好后，不留单段的话删掉不是成品的单段文件；
/// 中途出错就都留着，什么都不丢
fn make_outputs(host: &dyn Host, ff: &Ffmpeg, o: &Options, parts: &[PathBuf], done: &mut Done) -> Result<()> {
    for (k, out) in o.outputs.iter().enumerate() {
        let dest = o.out.join(format!("{}.mp4", out.name));
        let files: Vec<PathBuf> = out.parts.iter().map(|&i| parts.get(i).cloned().context("成品用到的片段没录成")).collect::<Result<_>>()?;
        match files.as_slice() {
            [one] if *one == dest => {}
            [one] => {
                std::fs::copy(one, &dest).with_context(|| format!("复制 {} 失败", dest.display()))?;
            }
            _ => ff.concat(&files, &dest)?,
        }
        host.event(Event::CompilationDone { path: dest.clone(), secs: media::mp4_duration(&dest)? });
        done.products.push((k, dest));
    }
    if !o.keep_parts && !o.outputs.is_empty() {
        for p in parts {
            if !done.products.iter().any(|(_, d)| d == p) && std::fs::remove_file(p).is_ok() {
                done.removed.push(p.clone());
            }
        }
    }
    Ok(())
}

/// 加载录像，等到能跳转为止。重新加载会把镜头重置成自由视角
fn load_replay(host: &dyn Host, r: &Replay, s: &mut Session, rx: &Receiver<Value>) -> Result<()> {
    spectator::hide_player_names(s)?;
    s.con().send(&format!("playdemo replays/{}", r.match_id))?;
    s.con()
        .wait_for(Duration::from_secs(120), |l| l.contains("OnSwitchLoopModeFinished( game : success )"))
        .context("录像没有加载起来（游戏里有这场录像吗？）")
        .context(Fail::ReplayLoad)?;
    // 跳转太早会让游戏崩溃（CopyNewEntity），等到这场的 GSI 时钟再动
    let id = r.match_id;
    gsi::wait_for(rx, Duration::from_secs(60), "这场比赛的 GSI 时钟", |st| st.match_id == Some(id) && st.clock.is_some()).context(Fail::ReplayLoad)?;
    host.event(Event::ReplayLoaded);
    Ok(())
}

/// 用控制台命令设镜头、迷雾、收面板、隐藏鼠标指针，把游戏放到最前面。
/// 录到黑画面、重录这一段前再调一次
fn setup_view(rec: &Rec, s: &mut Session) -> Result<()> {
    // 游戏放在最前面录（2026-11-09 定）。最小化保持不住；放在其他窗口后面的话，回放每次开始播放它都会自己跳到最前、
    // 抢走前台，程序再还回去，用户自己切过去看一眼也会被推回后面，体验差（2026-11-08～09 实测）。
    // 在前面渲染也可能更快：同一段在后面录 28 秒，在前面 18 秒（只碰巧测到一次）。
    // 等窗口大小稳定，设好镜头、渲染 2 秒再录（万一录到黑的世界画面，`capture` 会查出来重录）
    let dota = game::window::dota_window()?;
    // 拦截从游戏刚启动就开着，以防这之间游戏窗口换过，按现在的窗口来拦
    game::input_guard::retarget();
    game::window::restore(dota);
    game::ui::bring_to_front(dota.0 as isize);
    wait_window_settled(rec.size);
    spectator::console_setup(s, rec.player, rec.camera, rec.team)?;
    std::thread::sleep(Duration::from_secs(2));
    window_state(rec, "设好画面后");
    Ok(())
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
    clip_builder::goto(s.con(), rec.rx, rec.r.tick_at(at).context("超出录像范围")?, at).context(Fail::GameStuck)?;
    s.con().send(&format!("dota_spectator_hero_index {}", rec.player))?;
    s.con().send(&format!("dota_camera_focus_player {}", rec.player))?;
    spectator::focus(rec.player, rec.rx)
}

/// 录一段，核对时长；失败重来一次（上一次录到的画面是黑的，先重设画面）
fn record_span(rec: &mut Rec, s: &mut Session, from: f32, to: f32) -> Result<PathBuf> {
    let mut failures = 0;
    let mut black = false;
    loop {
        let tried = if black { setup_view(rec, s) } else { Ok(()) };
        let err = match tried.and_then(|()| capture(rec, s, from, to)) {
            Ok(part) => return Ok(part),
            Err(e) => e,
        };
        black = err.downcast_ref::<BlackWorld>().is_some();
        // 停下了就不重来
        cancel::check()?;
        failures += 1;
        if failures >= 2 {
            return Err(err);
        }
        rec.host.event(Event::Retrying { reason: format!("{err:#}"), fail: fail_kind(&err) });
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
        // 原先还发 demo_pauseatservertick 让回放停在终点，但它要的是服务器 tick，和录像 tick 差几百
        // （2026-11-09 实测：终点算成"已经过了"，立刻暂停，偶尔压过随后的 demo_resume，整段录成静止画面）。
        // 段长靠数帧控制，到数就 endmovie、暂停，多出的帧丢掉，用不着它
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
            // 点了停止：这一段不要了
            cancel::check()?;
            let Some(frame) = movie.take()? else {
                if started.elapsed() > timeout {
                    return Err(anyhow!("等了 {timeout:?} 只录了 {} 帧（共 {expected} 帧），游戏卡住了？", movie.taken()).context(Fail::GameStuck));
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
                    enc.insert(rec.ff.encoder(rec.codec, f, &video, EncodeHow::default()).context(Fail::Encode)?)
                }
            };
            e.write(&frame.bgra).context(Fail::Encode)?;
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
        enc.context("一帧都没录到").context(Fail::GameStuck)?.finish().context(Fail::Encode)?;
        let part = rec.tmp.join(format!("{prefix}_av.mp4"));
        rec.ff.mux_audio(&video, &movie.wav(), skip as f32 / FPS as f32, &part).context(Fail::Encode)?;
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
