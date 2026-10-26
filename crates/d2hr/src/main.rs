//! d2hr：Dota 2 高光录制命令行。
//!
//! d2hr list                 列出游戏客户端已下载的录像
//! d2hr plan <比赛ID|文件>   打印某个玩家的高光片段和录制计划（默认是自己）
//! d2hr record <比赛ID|文件> 录下这些片段，每个片段一个视频，可选拼成合集

mod record;

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use clips::{Anchor, Params};

#[derive(Parser)]
#[command(name = "d2hr", version, about = "Dota 2 高光录制")]
struct Cli {
    /// 录像目录，默认是游戏客户端的 replays 目录
    #[arg(long, global = true)]
    dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 列出本地录像
    List,
    /// 打印高光片段和录制计划
    Plan {
        /// 比赛 ID（在录像目录里找），或者录像文件路径
        replay: String,
        /// 玩家：序号 0–9 或账号 ID，默认是自己（Steam 当前登录的账号）
        #[arg(long)]
        player: Option<u64>,
        #[command(flatten)]
        rules: Rules,
    },
    /// 录制高光片段（会启动或重启 Dota，录完之前不要动鼠标键盘）
    Record {
        /// 比赛 ID（在录像目录里找），或者录像文件路径
        replay: String,
        /// 玩家：序号 0–9 或账号 ID，默认是自己（Steam 当前登录的账号）
        #[arg(long)]
        player: Option<u64>,
        /// 只录这几个片段（plan 列出的序号），例如 1,3；默认全录
        #[arg(long, value_delimiter = ',')]
        clips: Vec<usize>,
        /// 再把所有片段拼成一个合集
        #[arg(long)]
        compile: bool,
        /// 录制分辨率，例如 1920x1080；默认跟游戏窗口一致。和游戏现在的窗口大小不同时要重启游戏，
        /// 录完游戏会退出（它退出时会把这个大小存进显示设置，程序等它退出后改回原来的大小）
        #[arg(long, value_parser = parse_resolution)]
        resolution: Option<(u32, u32)>,
        /// 输出目录，默认是「视频\d2hr」
        #[arg(long)]
        out: Option<PathBuf>,
        /// ffmpeg.exe 或它所在的目录；不给就看环境变量 D2HR_FFMPEG，再在 PATH 里找
        #[arg(long)]
        ffmpeg: Option<PathBuf>,
        /// 接收游戏状态（GSI）的本机端口
        #[arg(long, default_value_t = game::gsi::DEFAULT_PORT)]
        gsi_port: u16,
        /// 录完让游戏退出
        #[arg(long)]
        quit: bool,
        /// 录制期间不检测鼠标键盘操作（默认一动就停下，另有取消快捷键 Ctrl+Shift+F12）
        #[arg(long)]
        allow_input: bool,
        #[command(flatten)]
        rules: Rules,
    },
}

fn parse_resolution(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s.split_once(['x', 'X', '*']).ok_or("要写成 宽x高，例如 1920x1080")?;
    match (w.trim().parse(), h.trim().parse()) {
        (Ok(w), Ok(h)) if w >= 640 && h >= 480 => Ok((w, h)),
        _ => Err("要写成 宽x高，例如 1920x1080".into()),
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum AnchorArg {
    /// 被杀者和任意敌方英雄之间
    A,
    /// 击杀者和被杀者之间
    B,
    /// 击杀者和任意敌方英雄之间
    C,
}

/// 片段规则参数，不给就用默认值（见 docs/phase1-plan.md）
#[derive(clap::Args)]
struct Rules {
    /// "开打"看谁和谁之间的伤害
    #[arg(long, value_enum)]
    anchor: Option<AnchorArg>,
    /// 相邻两次伤害间隔不超过多少秒算同一波战斗
    #[arg(long)]
    damage_gap: Option<f32>,
    /// 起点最多在击杀前多少秒
    #[arg(long)]
    max_lead: Option<f32>,
    /// 击杀后留多少秒
    #[arg(long)]
    post: Option<f32>,
    /// 相邻击杀间隔不超过多少秒合成一个片段
    #[arg(long)]
    merge_gap: Option<f32>,
    /// 片段内空档不超过多少秒就保留
    #[arg(long)]
    keep_gap: Option<f32>,
}

impl Rules {
    fn params(&self) -> Params {
        let d = Params::default();
        Params {
            anchor: match self.anchor {
                Some(AnchorArg::A) => Anchor::Victim,
                Some(AnchorArg::B) => Anchor::KillerVictim,
                Some(AnchorArg::C) => Anchor::KillerAny,
                None => d.anchor,
            },
            damage_gap: self.damage_gap.unwrap_or(d.damage_gap),
            max_lead: self.max_lead.unwrap_or(d.max_lead),
            post: self.post.unwrap_or(d.post),
            merge_gap: self.merge_gap.unwrap_or(d.merge_gap),
            keep_gap: self.keep_gap.unwrap_or(d.keep_gap),
            ..d
        }
    }
}

fn main() -> Result<()> {
    // 解析器递归较深，放到大栈线程里跑
    std::thread::Builder::new().stack_size(64 << 20).spawn(run)?.join().unwrap()
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let dir = match cli.dir {
        Some(d) => d,
        None => game::launch::replays_dir().context("找不到 Dota 2 的录像目录，用 --dir 指定")?,
    };
    match cli.command {
        Command::List => list(&dir),
        Command::Plan { replay, player, rules } => plan(&dir, &replay, player, &rules.params()),
        Command::Record { replay, player, clips, compile, resolution, out, ffmpeg, gsi_port, quit, allow_input, rules } => {
            // 先找 ffmpeg，免得游戏都开起来了才发现缺东西
            let ff = media::ffmpeg::Ffmpeg::locate(ffmpeg.as_deref())?;
            let p = rules.params();
            let (r, player) = load(&dir, &replay, player)?;
            let plan = clips::plan(&r, player, &p);
            if plan.is_empty() {
                bail!("这名玩家整场没有击杀，没有可录的片段");
            }
            let wanted: Vec<usize> = if clips.is_empty() { (1..=plan.len()).collect() } else { clips };
            let mut selected = Vec::new();
            for i in wanted {
                let clip = plan.get(i.wrapping_sub(1)).with_context(|| format!("没有第 {i} 个片段（共 {} 个）", plan.len()))?;
                selected.push(pipeline::Selected { index: i, clip: clip.clone() });
            }
            let secs: f32 = selected.iter().map(|s| s.clip.estimated_seconds(&p)).sum();
            let hero_name = hero(&r.players[player].hero);
            println!("录 #{player} {hero_name} 的 {} 个片段，预计约 {:.1} 分钟（不含启动游戏），期间请勿操作鼠标键盘。", selected.len(), secs / 60.0);
            let out = match out {
                Some(d) => d,
                None => PathBuf::from(std::env::var("USERPROFILE").context("找不到用户目录")?).join(r"Videos\d2hr"),
            };
            let o = pipeline::Options { compile, resolution, out, gsi_port, quit, input_guard: !allow_input };
            record::run(&r, player, &selected, &p, &ff, &o)
        }
    }
}

fn hero(name: &str) -> &str {
    name.trim_start_matches("npc_dota_hero_")
}

/// 秒 → `mm:ss`（号角前为负）
fn clock(t: f32) -> String {
    let sign = if t < 0.0 { "-" } else { "" };
    let s = t.abs().round() as u32;
    format!("{sign}{:02}:{:02}", s / 60, s % 60)
}

/// 修改时间 → 几分钟 / 几小时 / 几天前，免得为了格式化日期再加依赖
fn ago(t: SystemTime) -> String {
    let secs = SystemTime::now().duration_since(t).unwrap_or(Duration::ZERO).as_secs();
    match secs {
        s if s < 3600 => format!("{} 分钟前", s / 60),
        s if s < 86400 => format!("{} 小时前", s / 3600),
        s => format!("{} 天前", s / 86400),
    }
}

fn list(dir: &std::path::Path) -> Result<()> {
    let files = replay::list(dir)?;
    if files.is_empty() {
        println!("{} 里没有录像。在游戏的「观战 → 比赛记录」里点「下载」后再来。", dir.display());
        return Ok(());
    }
    let me = game::steam::me();
    println!("录像目录：{}", dir.display());
    println!("{:<12} {:<10} {:>7} {:>6}  我的英雄", "比赛 ID", "下载于", "大小", "时长");
    for f in files {
        let size = format!("{:.0}MB", f.size as f64 / 1e6);
        match replay::peek(&f.path) {
            Ok(s) => {
                let mine = me
                    .and_then(|id| s.players.iter().find(|p| p.account_id == id))
                    .map(|p| {
                        let won = if p.team == s.winner { "胜" } else { "负" };
                        format!("{}（{won}）", hero(&p.hero))
                    })
                    .unwrap_or_else(|| "—".into());
                println!("{:<12} {:<10} {:>7} {:>6}  {mine}", s.match_id, ago(f.modified), size, clock(s.playback_time));
            }
            Err(e) => println!("{:<12} {:<10} {:>7}  读不了：{e:#}", f.match_id, ago(f.modified), size),
        }
    }
    Ok(())
}

/// 读录像并选玩家：给了序号或账号就用，否则找自己
fn load(dir: &std::path::Path, which: &str, player: Option<u64>) -> Result<(replay::Replay, usize)> {
    let path = match which.parse::<u64>() {
        Ok(id) => dir.join(format!("{id}.dem")),
        Err(_) => PathBuf::from(which),
    };
    if !path.is_file() {
        bail!("本地没有这场录像：{}\n请先在游戏的「观战 → 比赛记录」里找到这场比赛，点「下载」。", path.display());
    }
    let r = replay::parse(&path)?;
    let me = game::steam::me();
    let player = match player {
        Some(n) if n < r.players.len() as u64 => n as usize,
        Some(acc) => r.players.iter().find(|p| p.account_id as u64 == acc).context("这场比赛里没有这个账号")?.player_id,
        None => {
            let id = me.context("认不出你的账号（Steam 没登录？），用 --player 指定玩家")?;
            r.players.iter().find(|p| p.account_id == id).map(|p| p.player_id).with_context(|| {
                format!("你（账号 {id}）不在这场比赛里，用 --player 指定要录谁")
            })?
        }
    };
    Ok((r, player))
}

fn plan(dir: &std::path::Path, which: &str, player: Option<u64>, p: &Params) -> Result<()> {
    let (r, player) = load(dir, which, player)?;
    let me = game::steam::me();

    println!("比赛 {}  游戏版本 {}", r.match_id, r.game_build);
    for pl in &r.players {
        let side = if pl.team == 2 { "天辉" } else { "夜魇" };
        let kills = r.kills.iter().filter(|k| k.killer == Some(pl.player_id)).count();
        let mark = match (pl.player_id == player, Some(pl.account_id) == me) {
            (true, true) => "← 录这个（你）",
            (true, false) => "← 录这个",
            (false, true) => "（你）",
            _ => "",
        };
        println!("  #{} {side} {:<20} 击杀 {kills:>2}  {} {mark}", pl.player_id, hero(&pl.hero), pl.name);
    }

    let clips = clips::plan(&r, player, p);
    println!(
        "\n片段规则：{}，伤害间隔 {}s，起点上限 {}s，开打前 {}s，击杀后 {}s，{}s 内的击杀合并，片段内 ≤{}s 的空档保留",
        p.anchor.label(),
        p.damage_gap,
        p.max_lead,
        p.pre,
        p.post,
        p.merge_gap,
        p.keep_gap
    );
    if clips.is_empty() {
        println!("这名玩家整场没有击杀，没有可录的片段。");
        return Ok(());
    }
    println!("\n{:>3}  {:<13} {:>6}  {:<28} {:>4}  文件名", "#", "时间", "时长", "击杀", "导出");
    for (i, c) in clips.iter().enumerate() {
        let victims: Vec<&str> = c.kills.iter().map(|&k| hero(&r.players[r.kills[k].victim].hero)).collect();
        let range = format!("{}–{}", clock(c.start()), clock(c.end()));
        println!(
            "{:>3}  {range:<13} {:>5.1}s  {:<28} {:>4}  {}.mp4",
            i + 1,
            c.duration(),
            victims.join(", "),
            c.exports.len(),
            clips::file_name(&r, c)
        );
    }
    let total: f32 = clips.iter().map(|c| c.duration()).sum();
    let exports: usize = clips.iter().map(|c| c.exports.len()).sum();
    let secs: f32 = clips.iter().map(|c| c.estimated_seconds(p)).sum();
    println!(
        "\n共 {} 个片段，成片 {:.0} 秒，导出 {exports} 次。预计录制约 {:.1} 分钟（不含启动游戏），期间请勿操作鼠标键盘。",
        clips.len(),
        total,
        secs / 60.0
    );
    Ok(())
}
