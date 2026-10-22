//! d2hr：Dota 2 高光录制命令行。
//!
//! d2hr list                 列出游戏客户端已下载的录像
//! d2hr plan <比赛ID|文件>   打印某个玩家的高光片段和录制计划（默认是自己）

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

fn plan(dir: &std::path::Path, which: &str, player: Option<u64>, p: &Params) -> Result<()> {
    let path = match which.parse::<u64>() {
        Ok(id) => dir.join(format!("{id}.dem")),
        Err(_) => PathBuf::from(which),
    };
    if !path.is_file() {
        bail!("本地没有这场录像：{}\n请先在游戏的「观战 → 比赛记录」里找到这场比赛，点「下载」。", path.display());
    }
    let r = replay::parse(&path)?;

    // 选玩家：给了序号或账号就用，否则找自己
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
