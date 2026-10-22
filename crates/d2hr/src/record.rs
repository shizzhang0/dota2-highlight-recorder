//! `d2hr record`：录一个玩家选定的高光片段，每个片段输出一个视频，可选拼成合集。
//! 流程见 docs/phase1-plan.md「录制流程」。

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clips::{Clip, Params};
use game::clip_builder::{self, Job};
use game::gsi;
use game::netcon::Netcon;
use game::session::Session;
use game::spectator::{self, Panel, Team};
use game::ui::Ui;
use media::ffmpeg::Ffmpeg;
use replay::Replay;
use serde_json::Value;

const NETCON_PORT: u16 = 2121;
/// 导出文件实际时长和计划长度允许差多少秒，超了就重新导出一次
const DURATION_TOLERANCE: f64 = 2.0;
/// 剪辑工具的时间轴窗口（秒）：选区落在中间，离两边远，把手不容易被夹住
const WINDOW: f32 = 120.0;

pub struct Options {
    pub compile: bool,
    pub resolution: Option<(u32, u32)>,
    pub out: PathBuf,
    pub gsi_port: u16,
    /// 录完让游戏退出。用 `resolution` 启动的游戏无论如何都会退出：
    /// 它退出时会把这个窗口大小写进显示设置，要等它退出后改回去
    pub quit: bool,
}

/// 选定的片段和它在 `plan` 里的序号（从 1 开始）
pub struct Selected {
    pub index: usize,
    pub clip: Clip,
}

fn ask(question: &str) -> bool {
    print!("{question} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).is_ok() && line.trim().eq_ignore_ascii_case("y")
}

fn dota_running() -> bool {
    game::window::dota_window().is_ok()
}

/// 等游戏窗口消失（用户自己退出，或者我们发了 quit）
fn wait_dota_gone(timeout: Duration) -> Result<()> {
    let started = Instant::now();
    while dota_running() {
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
fn restart_needed(reason: &str) -> Result<()> {
    println!("{reason}");
    if !ask("现在让游戏退出，再重新启动？") {
        bail!("已取消");
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
fn prepare(o: &Options, diag: &Path) -> Result<(Netcon, Receiver<Value>, bool)> {
    gsi::check_port(o.gsi_port)?;
    let changed = gsi::install(o.gsi_port)?;
    let rx = gsi::listen(o.gsi_port)?;

    if dota_running() && !game::launch::netcon_ready(NETCON_PORT) {
        println!("Dota 正以普通模式运行。录制要带控制台端口（-netconport -insecure）重新启动游戏。");
        println!("请先退出 Dota（主菜单 → 退出游戏），退出后会自动继续；按 Ctrl+C 取消。");
        wait_dota_gone(Duration::from_secs(600))?;
    }
    if dota_running() {
        // 配置刚写入：除非游戏启动时就有同样的配置（上一轮写过、结束时删了），否则收不到包。
        // 暂停、尤其是游戏在后台时几秒才一个包，等久一点
        if changed && gsi::wait_for(&rx, Duration::from_secs(15), "GSI 包", |_| true).is_err() {
            restart_needed("游戏开着，但没读到我们的 GSI 配置（配置只在游戏启动时读）。")?;
        } else if let (Some(want), Some(now)) = (o.resolution, client_size())
            && want != now
        {
            restart_needed(&format!("游戏窗口现在是 {}×{}，要录 {}×{} 得重新启动游戏。", now.0, now.1, want.0, want.1))?;
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
        println!("启动游戏：{}", extra.join(" "));
        game::launch::launch(NETCON_PORT, &extra)?;
        game::launch::wait_netcon(NETCON_PORT, Duration::from_secs(180))?;
    }
    let mut con = Netcon::connect_logged(NETCON_PORT, &diag.join("console.log"))?;
    con.wait_responsive(Duration::from_secs(120))?;
    con.drain(Duration::from_millis(300), Duration::from_secs(2));
    Ok((con, rx, sized))
}

pub fn run(r: &Replay, player: usize, selected: &[Selected], p: &Params, ff: &Ffmpeg, o: &Options) -> Result<()> {
    let team = Team::from_id(r.players[player].team).context("玩家阵营不是天辉也不是夜魇")?;
    let tmp = o.out.join("tmp");
    let diag = o.out.join("diag");
    std::fs::create_dir_all(&tmp)?;
    std::fs::create_dir_all(&diag)?;

    let started = Instant::now();
    // 带 -w -h 启动的游戏退出时会把这个窗口大小写进显示设置，先记下原来的
    let account = game::steam::me();
    let original_size = account.and_then(|a| game::steam::video(a).ok()).and_then(|v| v.width.zip(v.height));
    let (con, rx, sized) = prepare(o, &diag)?;
    // 设置守护：离开作用域（包括出错）时恢复，并删掉 GSI 配置
    let mut s = Session::new(con);
    s.remove_gsi_on_exit();
    s.set("dota_spectator_enable_edgepan_temporary_free_camera", "0")?;
    s.set("dota_clip_builder_max_duration", &WINDOW.to_string())?;
    s.set("dota_clip_builder_output_folder", &tmp.to_string_lossy().replace('\\', "/"))?;

    let result = record_all(r, player, team, selected, p, ff, o, &mut s, &rx, &tmp, &diag);
    let restored = s.restore();
    if o.quit || sized {
        let _ = s.con().send("quit");
    }
    if sized && let (Some(account), Some((w, h))) = (account, original_size) {
        println!("让游戏退出，把显示设置里的窗口大小改回 {w}×{h}…");
        match wait_dota_gone(Duration::from_secs(60)).and_then(|()| game::steam::set_video_size(account, w, h)) {
            Ok(()) => println!("窗口大小已改回 {w}×{h}"),
            Err(e) => println!("没能改回窗口大小（{e:#}），请在游戏的视频设置里自己改回 {w}×{h}"),
        }
    }
    match (&result, &restored) {
        (Ok(_), Ok(())) => {
            // 成功就不留排查用的截图和日志
            let _ = std::fs::remove_dir_all(&diag);
            let _ = std::fs::remove_dir(&tmp);
        }
        _ => println!("排查用的截图和控制台日志在 {}", diag.display()),
    }
    if let Err(e) = &restored {
        println!("恢复游戏设置时出错：{e:#}");
    }
    let files = result?;
    println!("\n完成，用时 {:.1} 分钟，输出 {} 个文件：", started.elapsed().as_secs_f32() / 60.0, files.len());
    for f in &files {
        println!("  {}", f.display());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn record_all(
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
) -> Result<Vec<PathBuf>> {
    // 只加载一次录像：重新加载会把镜头重置成自由视角
    s.con().send(&format!("playdemo replays/{}", r.match_id))?;
    s.con()
        .wait_for(Duration::from_secs(120), |l| l.contains("OnSwitchLoopModeFinished( game : success )"))
        .context("录像没有加载起来（游戏里有这场录像吗？）")?;
    // 跳转太早会让游戏崩溃（CopyNewEntity），等到这场的 GSI 时钟再动
    let id = r.match_id;
    gsi::wait_for(rx, Duration::from_secs(60), "这场比赛的 GSI 时钟", |st| st.match_id == Some(id) && st.clock.is_some())?;

    for i in (1..=5).rev() {
        println!("{i} 秒后开始操作游戏界面，录完之前请不要动鼠标键盘…");
        std::thread::sleep(Duration::from_secs(1));
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
        println!("[{}/{}] 片段 {}：{}", n + 1, selected.len(), sel.index, name);
        let mut cuts = Vec::new();
        for (k, e) in clip.exports.iter().enumerate() {
            let from = (e.span.start - p.export_margin).max(first);
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
            let part = export_checked(&mut ui, s, rx, &job, player)?;
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
        println!("  → {}（{:.1} 秒）", dest.display(), media::mp4_duration(&dest)?);
        outputs.push(dest);
    }

    if o.compile && outputs.len() > 1 {
        let hero = r.players[player].hero.trim_start_matches("npc_dota_hero_");
        let dest = o.out.join(format!("{}_{player}_{hero}_合集.mp4", r.match_id));
        ff.concat(&outputs, &dest)?;
        println!("合集 → {}（{:.1} 秒）", dest.display(), media::mp4_duration(&dest)?);
        outputs.push(dest);
    }
    Ok(outputs)
}

/// 导出一段并核对时长；导出失败、文件 0 字节或时长偏差超过容差时重来一次
fn export_checked(ui: &mut Ui, s: &mut Session, rx: &Receiver<Value>, job: &Job, player: usize) -> Result<PathBuf> {
    let planned = (job.to - job.from) as f64;
    let mut last_err = None;
    for attempt in 1..=2 {
        let tried = spectator::focus(player, rx).and_then(|()| clip_builder::export(ui, s.con(), rx, job));
        match tried {
            Ok(done) => {
                let secs = media::mp4_duration(&done.path)?;
                if (secs - planned).abs() <= DURATION_TOLERANCE {
                    println!("  导出 {}–{}：{secs:.1} 秒，渲染 {:.0} 秒", clips::mmss(job.from), clips::mmss(job.to), done.render.as_secs_f32());
                    let part = job.out_dir.join(format!("part_{}.mp4", job.center_tick));
                    std::fs::rename(&done.path, &part)?;
                    return Ok(part);
                }
                let _ = std::fs::remove_file(&done.path);
                last_err = Some(anyhow::anyhow!("导出的视频 {secs:.1} 秒，计划 {planned:.1} 秒"));
            }
            Err(e) => last_err = Some(e),
        }
        if attempt == 1 {
            println!("  第一次导出不对（{:#}），重来一次", last_err.as_ref().unwrap());
        }
    }
    Err(last_err.unwrap())
}
