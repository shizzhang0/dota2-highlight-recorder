//! `d2hr watch`：在游戏里把几场录像从头到尾放完（不录视频），全部放完让游戏退出。
//! 给别的程序收这几场回放的游戏状态（GSI）用，例如 dota2-game-helper2 的「记录对局数据」：
//! 它要的是"从头放、不拖进度条"的整场回放，GSI 里有十个人的数据和官方净资产。
//!
//! 不跳转、不切镜头、不拦输入。每场开始播放（`demo_resume`）时 Dota 会把自己提到最前面、抢走前台
//! （游戏自己的行为，见 `game::window`），那一刻在别的窗口打的字会进游戏；之后可以切走。
//!
//! 一场出错（加载不起来、卡住）就记下、回主菜单接着放下一场，最后汇总；游戏没了才整个停下。

use std::path::Path;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use game::gsi::{self, State};
use game::session::Session;

use crate::clock;
use crate::record::Console;

/// 比赛结束（POST_GAME）后再放多久才换下一场。dota2-game-helper2 结束后 10 秒收尾，多留 5 秒
const POST_GAME_SECS: u64 = 15;
/// 开局号角之后，没暂停、游戏时钟却这么久不走，先补发一次 `demo_resume`：回放可能被人按暂停了
/// （GSI 的 `paused` 只管比赛里的暂停，管不到回放暂停）
const NUDGE_SECS: u64 = 30;
/// 补发过 `demo_resume` 还是不走，到这么久就当录像放完了（停在最后一帧）
const STALL_SECS: u64 = 90;
/// 这么久一个这场的 GSI 包都没有，就当游戏关了（我们的配置心跳 1 秒）
const NO_PACKET_SECS: u64 = 60;
/// 进度多久打一行
const PROGRESS_EVERY: Duration = Duration::from_secs(60);
/// 实际倍速低于设定的这个比例就提醒（`demo_timescale` 没生效的话会按 1 倍放，不报错）
const SLOW_RATIO: f32 = 0.7;
/// 从游戏时钟到这里起算平均倍速：号角前选人阶段时钟走不走说不准，策略时间和准备时间（-90 起）一定在走
const RATE_FROM_CLOCK: i64 = -80;
const POST_GAME: &str = "DOTA_GAMERULES_STATE_POST_GAME";
/// 发了 `quit` 最多等游戏窗口消失这么久
const QUIT_WAIT: Duration = Duration::from_secs(180);
/// 游戏没退就隔这么久再发一次 `quit`
const QUIT_RETRY: Duration = Duration::from_secs(5);

pub struct Options {
    pub speed: f32,
    pub gsi_port: u16,
    /// 放完不让游戏退出
    pub keep_open: bool,
}

pub fn run(dir: &Path, ids: &[u64], o: &Options) -> Result<()> {
    // 同一场给了两次只放一次
    let mut unique = Vec::new();
    for &id in ids {
        if !unique.contains(&id) {
            unique.push(id);
        }
    }
    if unique.len() < ids.len() {
        println!("有重复的比赛 ID，每场只放一次。");
    }
    let ids = unique;
    // 先把录像都找齐，免得游戏开起来才发现少一场
    let mut lens = Vec::new();
    for &id in &ids {
        let path = dir.join(format!("{id}.dem"));
        if !path.is_file() {
            bail!("本地没有这场录像：{}\n请先在游戏的「观战 → 比赛记录」里找到这场比赛，点「下载」。", path.display());
        }
        lens.push(replay::peek(&path)?.playback_time);
    }
    let total: f32 = lens.iter().sum();
    println!(
        "放 {} 场录像，{} 倍速，预计约 {:.0} 分钟（不含启动游戏、加载录像）。按 Ctrl+C 停下。",
        ids.len(),
        o.speed,
        total / o.speed / 60.0
    );
    println!("每场开始播放时 Dota 会跳到最前面（游戏自己的行为），那一刻别在别的窗口打字；之后可以切走。");

    // 第一次 Ctrl+C 只做标记：手上这一步做完就停，照常恢复设置
    game::cancel::install()?;
    let diag = std::env::temp_dir().join("d2hr-watch");
    std::fs::create_dir_all(&diag)?;
    // 控制台日志是追加写的，只留这一次的
    let _ = std::fs::remove_file(diag.join("console.log"));
    let started = Instant::now();
    let (con, rx) = pipeline::prepare_plain(&Console { task: "放录像" }, o.gsi_port, &diag)?;
    let mut s = Session::new(con);
    s.remove_gsi_on_exit();

    let mut done = Vec::new();
    let mut failed = Vec::new();
    let mut fatal = None;
    for (k, &id) in ids.iter().enumerate() {
        println!("[{}/{}] {id}（录像 {}）", k + 1, ids.len(), clock(lens[k]));
        match play(&mut s, &rx, id, lens[k], o.speed) {
            Ok(()) => done.push(id),
            Err(_) if game::cancel::requested() => break,
            Err(e) => {
                println!("  这场没放完：{e:#}");
                failed.push((id, format!("{e:#}")));
                if game::window::dota_window().is_err() {
                    fatal = Some(e.context("游戏没了，后面的不放了"));
                    break;
                }
                // 回主菜单，接着放下一场
                let _ = s.con().send("disconnect");
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    }

    // 停下的原因要在 hold 之外读（hold 期间读不到，2026-11-22 试跑时停下了却报成"放完 0 场"）
    let cancelled = game::cancel::reason();
    let _hold = game::cancel::hold();
    // demo_timescale 不带 archive，不写进配置文件，不用 Session 记原值；还是改回 1，免得游戏留着时下次看回放还是快的
    let _ = s.con().send("demo_timescale 1");
    let restored = s.restore();
    // 停下了也退：-insecure 的游戏进不了匹配，留着没用，接着放就再跑一次。游戏没了就不用发
    let want_quit = fatal.is_none() && !o.keep_open;
    let quit = want_quit && quit_until_gone(&mut s);
    if let Err(e) = &restored {
        println!("恢复游戏设置时出错：{e:#}");
    }
    let minutes = started.elapsed().as_secs_f32() / 60.0;
    println!();
    if let Some(reason) = cancelled {
        println!("已停止（{}）。", reason.describe());
    }
    println!("放完 {} 场：{done:?}，用时 {minutes:.1} 分钟{}。", done.len(), if quit { "，游戏已退出" } else { "" });
    if want_quit && !quit {
        println!("游戏没有退出（等了 {} 秒），请手动关掉。", QUIT_WAIT.as_secs());
    }
    if !failed.is_empty() {
        println!("没放完 {} 场：", failed.len());
        for (id, e) in &failed {
            println!("  {id}：{e}");
        }
        println!("排查用的控制台日志：{}", diag.join("console.log").display());
    }
    let left: Vec<u64> = ids.iter().copied().filter(|id| !done.contains(id)).collect();
    if let Some(e) = fatal {
        return Err(e);
    }
    if cancelled.is_some() {
        bail!("已停止，没放的：{left:?}");
    }
    if !failed.is_empty() {
        bail!("有 {} 场没放完：{left:?}", failed.len());
    }
    Ok(())
}

/// 让游戏退出，等到窗口没了才返回 true。只发一次不够：2026-11-26 放完两场，回主菜单 3 秒后发的 `quit`
/// 游戏没理（刚回主菜单还在加载，和录制那边 2026-11-20 启动中途发 `quit` 被忽略一样），
/// 而且发完程序马上退出、连接一断，游戏也可能没读到。所以连接一直开着，没退就隔几秒再发
fn quit_until_gone(s: &mut Session) -> bool {
    let started = Instant::now();
    while game::window::dota_window().is_ok() {
        if started.elapsed() > QUIT_WAIT {
            return false;
        }
        // 游戏退出时连接会断，发不出去不要紧，看窗口
        let _ = s.con().send("quit");
        std::thread::sleep(QUIT_RETRY);
    }
    true
}

/// 从头放一场，放到比赛结束后 `POST_GAME_SECS` 秒，回主菜单
fn play(s: &mut Session, rx: &gsi::Listener, id: u64, len: f32, speed: f32) -> Result<()> {
    // 上一场放的时候积压的控制台输出里也有"加载好了"那一行，GSI 里也积压着上一场结束后的包，先都清掉
    s.con().drain(Duration::from_millis(300), Duration::from_secs(30));
    gsi::drain(rx);
    s.con().send(&format!("playdemo replays/{id}"))?;
    s.con()
        .wait_for(Duration::from_secs(180), |l| l.contains("OnSwitchLoopModeFinished( game : success )"))
        .context("录像没有加载起来（游戏里有这场录像吗？）")?;
    gsi::wait_for(rx, Duration::from_secs(60), "这场比赛的 GSI", |st| st.match_id == Some(id))?;
    s.con().send(&format!("demo_timescale {speed}"))?;
    s.con().send("demo_resume")?;

    let start = Instant::now();
    // 正常是 len / speed；暂停多、机器慢也不该超过两倍再加 10 分钟
    let limit = Duration::from_secs_f32(len / speed * 2.0 + 600.0);
    let mut last_packet = Instant::now();
    let mut last_clock: Option<i64> = None;
    let mut clock_moved = Instant::now();
    let mut nudged = false;
    // 起算平均倍速的点：(游戏时钟, 墙钟)；中间遇到比赛暂停就不提醒倍速
    let mut first: Option<(i64, Instant)> = None;
    let mut saw_pause = false;
    let mut warned_slow = false;
    let mut post: Option<Instant> = None;
    let mut reported = Instant::now();
    loop {
        game::cancel::check()?;
        let now = Instant::now();
        if post.is_some_and(|t| t.elapsed() >= Duration::from_secs(POST_GAME_SECS)) {
            break;
        }
        if start.elapsed() > limit {
            bail!("放了 {:.0} 分钟还没放完，游戏卡住了？", start.elapsed().as_secs_f32() / 60.0);
        }
        match rx.recv_timeout(Duration::from_millis(300)) {
            Ok(v) => {
                let st = State::from_json(&v);
                if st.match_id.is_some_and(|m| m != id) {
                    continue;
                }
                last_packet = now;
                if st.game_state == POST_GAME && post.is_none() {
                    println!("  比赛结束，再放 {POST_GAME_SECS} 秒");
                    post = Some(now);
                }
                if let Some(c) = st.clock {
                    if last_clock != Some(c) {
                        last_clock = Some(c);
                        clock_moved = now;
                        nudged = false;
                    }
                    if c >= RATE_FROM_CLOCK && first.is_none() {
                        first = Some((c, now));
                    }
                }
                // 比赛暂停时时钟本来就不走
                if st.paused {
                    clock_moved = now;
                    saw_pause = true;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => bail!("接收游戏状态的线程停了"),
        }
        if last_packet.elapsed() > Duration::from_secs(NO_PACKET_SECS) {
            bail!("{NO_PACKET_SECS} 秒没收到这场的游戏状态，游戏关了？");
        }
        // 号角前（选人、策略时间）时钟走法说不准，只在开局后判"不走了"
        if post.is_none() && last_clock.is_some_and(|c| c > 0) {
            let stalled = clock_moved.elapsed();
            if !nudged && stalled > Duration::from_secs(NUDGE_SECS) {
                println!("  游戏时钟 {NUDGE_SECS} 秒没走，补发一次继续播放（回放可能被暂停了）");
                s.con().send("demo_resume")?;
                nudged = true;
            }
            if stalled > Duration::from_secs(STALL_SECS) {
                println!("  游戏时钟 {STALL_SECS} 秒没走，当作录像放完了（没看到比赛结束）");
                break;
            }
        }
        if reported.elapsed() >= PROGRESS_EVERY {
            reported = Instant::now();
            if let (Some(c), Some((c0, t0))) = (last_clock, first) {
                let secs = t0.elapsed().as_secs_f32();
                let rate = (c - c0) as f32 / secs.max(1.0);
                println!("  {}（平均 {rate:.1} 倍速）", clock(c as f32));
                if !warned_slow && !saw_pause && secs >= 30.0 && rate < speed * SLOW_RATIO {
                    println!("  注意：设的是 {speed} 倍速，实际只有 {rate:.1} 倍，demo_timescale 可能没生效");
                    warned_slow = true;
                }
            }
        }
    }
    // 回主菜单：dota2-game-helper2 看到主菜单也会收尾
    s.con().send("disconnect")?;
    std::thread::sleep(Duration::from_secs(3));
    Ok(())
}
