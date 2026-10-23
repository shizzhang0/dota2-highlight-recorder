//! 里程碑 2 的端到端验证：用游戏自带的剪辑工具，导出某个玩家一个片段的所有导出段。
//!
//! cargo run --release -p recorder --example export_one -- --match <比赛ID> --player <序号0-9|账号ID> \
//!     [--clip <第几个片段，从 1 开始> | --from <分:秒> --to <分:秒>] [--window 120] [--out <目录>] \
//!     [--no-reload] [--background-render] [--unmute-bg] [--extra "-perfectworld"] [--gsi-port 2122] \
//!     [--no-setup] [--focus key|portrait|none] [--set <设置项>=<值>]... [--tag <文件名后缀>]
//!
//! 不给 --clip 也不给 --from/--to 时，只列出该玩家的片段。
//! 后面一行是查"导出视频里没有装备栏"的对比实验用的：`--no-setup` 不设镜头和迷雾，`--focus` 选人方式
//! （数字键 / 点顶部头像 / 不选），`--set` 临时改设置（结束时恢复），`--tag` 给文件名加后缀区分。
//! 导出的文件放在 --out（默认 `视频\d2hr`），每次点击的截图放在它下面的 diag 目录。
//! 开始后用户不能动鼠标键盘，直到程序把前台还回来。

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use game::clip_builder::{self, Job};
use game::gsi;
use game::layout;
use game::netcon::Netcon;
use game::session::Session;
use game::spectator::{self, Panel, Team};
use game::ui::Ui;
use replay::Replay;

const NETCON_PORT: u16 = 2121;
/// 导出文件实际时长和计划长度允许差多少秒
const DURATION_TOLERANCE: f64 = 2.0;

struct Args {
    match_id: u64,
    player: String,
    clip: Option<usize>,
    from: Option<f32>,
    to: Option<f32>,
    window: f32,
    out: Option<PathBuf>,
    no_reload: bool,
    background_render: bool,
    unmute_bg: bool,
    extra: Vec<String>,
    gsi_port: u16,
    no_setup: bool,
    focus: Focus,
    sets: Vec<(String, String)>,
    tag: Option<String>,
}

/// 怎么选中要追踪的玩家
#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Key,
    Portrait,
    None,
}

fn parse_clock(s: &str) -> Result<f32> {
    let (m, sec) = s.split_once(':').ok_or_else(|| anyhow!("时间要写成 分:秒，例如 4:05"))?;
    Ok(m.parse::<f32>()? * 60.0 + sec.parse::<f32>()?)
}

fn args() -> Result<Args> {
    let mut a = Args {
        match_id: 0,
        player: String::new(),
        clip: None,
        from: None,
        to: None,
        window: 120.0,
        out: None,
        no_reload: false,
        background_render: false,
        unmute_bg: false,
        extra: Vec::new(),
        gsi_port: gsi::DEFAULT_PORT,
        no_setup: false,
        focus: Focus::Key,
        sets: Vec::new(),
        tag: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut val = || it.next().ok_or_else(|| anyhow!("{flag} 后面缺值"));
        match flag.as_str() {
            "--match" => a.match_id = val()?.parse()?,
            "--player" => a.player = val()?,
            "--clip" => a.clip = Some(val()?.parse()?),
            "--from" => a.from = Some(parse_clock(&val()?)?),
            "--to" => a.to = Some(parse_clock(&val()?)?),
            "--window" => a.window = val()?.parse()?,
            "--out" => a.out = Some(val()?.into()),
            "--no-reload" => a.no_reload = true,
            "--background-render" => a.background_render = true,
            "--unmute-bg" => a.unmute_bg = true,
            "--extra" => a.extra = val()?.split_whitespace().map(str::to_owned).collect(),
            "--gsi-port" => a.gsi_port = val()?.parse()?,
            "--no-setup" => a.no_setup = true,
            "--focus" => {
                a.focus = match val()?.as_str() {
                    "key" => Focus::Key,
                    "portrait" => Focus::Portrait,
                    "none" => Focus::None,
                    v => bail!("--focus 只能是 key、portrait、none，不认识 {v}"),
                }
            }
            "--set" => {
                let v = val()?;
                let (name, value) = v.split_once('=').ok_or_else(|| anyhow!("--set 要写成 设置项=值"))?;
                a.sets.push((name.to_owned(), value.to_owned()));
            }
            "--tag" => a.tag = Some(val()?),
            _ => bail!("不认识的参数 {flag}"),
        }
    }
    if a.match_id == 0 || a.player.is_empty() {
        bail!("要给 --match 和 --player");
    }
    Ok(a)
}

/// 一个要导出的范围和导出后的文件名
struct Part {
    from: f32,
    to: f32,
    name: String,
}

fn main() -> Result<()> {
    // 解析器递归较深，放到大栈线程里跑
    std::thread::Builder::new().stack_size(64 << 20).spawn(run)?.join().unwrap()
}

fn run() -> Result<()> {
    let a = args()?;
    let dem = game::launch::replays_dir()?.join(format!("{}.dem", a.match_id));
    let r = replay::parse(&dem).with_context(|| format!("先在游戏里下载这场录像：{}", dem.display()))?;
    let player = match a.player.parse::<usize>() {
        Ok(i) if i < r.players.len() => i,
        Ok(acc) => r.players.iter().find(|p| p.account_id as usize == acc).context("本场没有这个账号")?.player_id,
        Err(_) => bail!("--player 要写玩家序号 0-9 或账号 ID"),
    };
    let team = Team::from_id(r.players[player].team).context("玩家阵营不是天辉也不是夜魇")?;
    let Some(parts) = parts(&a, &r, player)? else { return Ok(()) };

    let out = match &a.out {
        Some(d) => d.clone(),
        None => PathBuf::from(std::env::var("USERPROFILE")?).join(r"Videos\d2hr"),
    };
    // 游戏先写到 tmp，核对通过再改名移到 out
    let tmp = out.join("tmp");
    std::fs::create_dir_all(&tmp)?;

    // 游戏和 GSI
    gsi::check_port(a.gsi_port)?;
    let changed = gsi::install(a.gsi_port)?;
    let running = game::launch::netcon_ready(NETCON_PORT);
    let rx = gsi::listen(a.gsi_port)?;
    // 配置刚写入而游戏已经开着：游戏只在启动时读配置，除非它启动时就有同样的配置
    // （上一轮运行写过、结束时删了），这时包照样会来
    // 暂停、尤其是游戏在后台时几秒才一个包，等久一点
    if running && changed && gsi::wait_for(&rx, Duration::from_secs(15), "GSI 包", |_| true).is_err() {
        gsi::remove()?;
        bail!("GSI 配置刚写入，游戏要重启才能读到。请退出游戏后重新运行");
    }
    if !running {
        println!("游戏没在 netcon 模式下运行，启动中…");
        game::launch::launch(NETCON_PORT, &a.extra)?;
        game::launch::wait_netcon(NETCON_PORT, Duration::from_secs(180))?;
    }
    let diag = out.join("diag");
    std::fs::create_dir_all(&diag)?;
    let mut con = Netcon::connect_logged(NETCON_PORT, &diag.join("console.log"))?;
    con.wait_responsive(Duration::from_secs(120))?;
    con.drain(Duration::from_millis(300), Duration::from_secs(2));

    // 设置守护：离开作用域（包括出错）时恢复，并删掉 GSI 配置
    let mut s = Session::new(con);
    s.remove_gsi_on_exit();
    s.set("dota_spectator_enable_edgepan_temporary_free_camera", "0")?;
    s.set("dota_clip_builder_max_duration", &a.window.to_string())?;
    s.set("dota_clip_builder_output_folder", &tmp.to_string_lossy().replace('\\', "/"))?;
    if a.unmute_bg {
        s.set("snd_mute_losefocus", "0")?;
    }
    for (name, value) in &a.sets {
        s.set(name, value)?;
    }
    let mut shown = vec!["dota_clip_builder_max_duration", "dota_clip_builder_output_folder", "snd_mute_losefocus"];
    shown.extend(a.sets.iter().map(|(n, _)| n.as_str()));
    for name in shown {
        println!("{name} = {}", s.con().get(name)?);
    }

    let result = export_all(&a, &r, player, team, &parts, &mut s, &rx, &out, &tmp);
    match s.restore() {
        Ok(()) => println!("游戏设置已恢复，GSI 配置已删除"),
        Err(e) => println!("恢复时出错：{e:#}"),
    }
    result
}

/// 要导出的范围。只列片段时返回 None
fn parts(a: &Args, r: &Replay, player: usize) -> Result<Option<Vec<Part>>> {
    let hero = r.players[player].hero.trim_start_matches("npc_dota_hero_");
    if let (Some(from), Some(to)) = (a.from, a.to) {
        let name = format!("{}_{player}_{hero}_{}_{}", r.match_id, clips::mmss(from), clips::mmss(to));
        return Ok(Some(vec![Part { from, to, name }]));
    }
    let p = clips::Params::default();
    let plan = clips::plan(r, player, &p);
    let Some(k) = a.clip else {
        for (i, c) in plan.iter().enumerate() {
            let exports: Vec<String> =
                c.exports.iter().map(|e| format!("{}–{}", clips::mmss(e.span.start), clips::mmss(e.span.end))).collect();
            println!("{:>2}. {}  导出段 {}", i + 1, clips::file_name(r, c), exports.join("、"));
        }
        return Ok(None);
    };
    let clip = plan.get(k.wrapping_sub(1)).with_context(|| format!("只有 {} 个片段", plan.len()))?;
    let base = clips::file_name(r, clip);
    let last = r.clock.last().map_or(f32::MAX, |c| c.game_time);
    let parts = clip
        .exports
        .iter()
        .enumerate()
        .map(|(i, e)| Part {
            from: e.span.start - p.export_lead,
            to: (e.span.end + p.export_margin).min(last),
            name: if clip.exports.len() == 1 { base.clone() } else { format!("{base}_part{}", i + 1) },
        })
        .collect();
    Ok(Some(parts))
}

#[allow(clippy::too_many_arguments)]
fn export_all(
    a: &Args,
    r: &Replay,
    player: usize,
    team: Team,
    parts: &[Part],
    s: &mut Session,
    rx: &std::sync::mpsc::Receiver<serde_json::Value>,
    out: &Path,
    tmp: &Path,
) -> Result<()> {
    let id = a.match_id;
    if !a.no_reload {
        s.con().send(&format!("playdemo replays/{id}"))?;
        s.con()
            .wait_for(Duration::from_secs(120), |l| l.contains("OnSwitchLoopModeFinished( game : success )"))
            .context("录像没有加载起来")?;
    }
    // 跳转太早会让游戏崩溃（CopyNewEntity），等到这场的 GSI 时钟再动
    gsi::wait_for(rx, Duration::from_secs(60), "这场比赛的 GSI 时钟", |st| st.match_id == Some(id) && st.clock.is_some())?;
    println!("录像已加载");

    for i in (1..=5).rev() {
        println!("{i} 秒后开始操作游戏界面，请不要动鼠标键盘…");
        std::thread::sleep(Duration::from_secs(1));
    }
    let mut ui = Ui::attach()?;
    ui.set_diag_dir(out.join("diag"))?;
    println!("游戏窗口客户区 {}×{}", ui.width, ui.height);

    // 先跳到第一段起点前 5 秒设镜头。不能直接跳到导出段中点：导出时还要再跳过去，
    // 暂停在同一 tick 时再跳，游戏不输出完成标志（第一次测试卡在这里）
    let prep = parts[0].from - 5.0;
    clip_builder::goto(s.con(), rx, r.tick_at(prep).context("超出录像范围")?, prep)?;
    if !a.no_reload {
        let _front = ui.front()?;
        if a.no_setup {
            // 剪刀在展开的面板里，镜头和迷雾不动
            ui.click(layout::PANEL_TOGGLE, "展开面板")?;
        } else {
            spectator::setup(&mut ui, Panel::Collapsed, team)?;
        }
    }

    for (i, part) in parts.iter().enumerate() {
        println!("[{}/{}] {} → {}", i + 1, parts.len(), clips::mmss(part.from), clips::mmss(part.to));
        match a.focus {
            Focus::Key => spectator::focus(player, rx)?,
            Focus::Portrait => {
                gsi::drain(rx);
                {
                    let _front = ui.front()?;
                    ui.click(layout::top_bar_hero(player), "点顶部头像")?;
                }
                match gsi::wait_next(rx, Duration::from_secs(10), "追踪玩家", |st| st.selected == Some(player)) {
                    Ok(_) => println!("  点头像后 GSI 显示追踪的是 #{player}"),
                    Err(_) => println!("  点头像后 GSI 没显示追踪 #{player}（照样导出，看截图）"),
                }
            }
            Focus::None => {}
        }
        let job = Job {
            from: part.from,
            to: part.to,
            center_tick: r.tick_at((part.from + part.to) / 2.0).context("超出录像范围")?,
            window: a.window,
            out_dir: tmp,
            background_render: a.background_render,
            player,
        };
        let done = clip_builder::export(&mut ui, s.con(), rx, &job)?;
        let secs = media::mp4_duration(&done.path)?;
        let planned = (part.to - part.from) as f64;
        println!(
            "  {}：{:.1} MB，时长 {secs:.2} 秒（计划 {planned:.1}），渲染 {:.1} 秒，GSI 时钟 {:?} → {:?}",
            done.path.display(),
            done.bytes as f64 / 1e6,
            done.render.as_secs_f32(),
            done.clock_first,
            done.clock_last,
        );
        if (secs - planned).abs() > DURATION_TOLERANCE {
            bail!("时长偏差超过 {DURATION_TOLERANCE} 秒，文件留在 {}", done.path.display());
        }
        let tag = a.tag.as_deref().map(|t| format!("_{t}")).unwrap_or_default();
        let dest = out.join(format!("{}{tag}.mp4", part.name));
        std::fs::rename(&done.path, &dest)?;
        println!("  → {}", dest.display());
    }
    Ok(())
}
