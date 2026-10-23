//! 片段规则的统计和对账：用本地全部录像比较不同参数下的片段情况，给选默认值提供依据。
//! 直接调用正式的规则函数（`clips::plan` / `clips::fight_start`），调参数后重跑即可复现。
//!
//! cargo run --release -p clips --example stats -- <录像目录> [--fetch-opendota]
//!
//! 对账数据存在 `examples/data/official_kd.json`：每场比赛每个位置的官方击杀、死亡数（来自 OpenDota），
//! 只存比赛 ID 和数字，不存玩家昵称和账号。加 `--fetch-opendota` 时，为数据里还没有的比赛联网补上。

use std::collections::BTreeMap;
use std::path::PathBuf;

use clips::{Anchor, Params, plan};
use replay::Replay;

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/data/official_kd.json");

/// 比赛 ID → 每个位置的（击杀, 死亡）
type Official = BTreeMap<u64, Vec<(u32, u32)>>;

fn pct(v: &mut [f32], p: f32) -> f32 {
    if v.is_empty() {
        return f32::NAN;
    }
    v.sort_by(f32::total_cmp);
    v[((v.len() - 1) as f32 * p).round() as usize]
}

fn share(v: &[f32], f: impl Fn(f32) -> bool) -> f32 {
    v.iter().filter(|&&x| f(x)).count() as f32 / v.len().max(1) as f32 * 100.0
}

fn main() -> anyhow::Result<()> {
    // 解析器递归较深，放到大栈线程里跑
    std::thread::Builder::new().stack_size(64 << 20).spawn(run)?.join().unwrap()
}

fn run() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("用法: stats <录像目录> [--fetch-opendota]"));
    let fetch = args.any(|a| a == "--fetch-opendota");

    let mut replays: Vec<Replay> = Vec::new();
    for f in replay::list(&dir)? {
        match replay::parse(&f.path) {
            Ok(r) => replays.push(r),
            Err(e) => println!("跳过 {}：{e:#}", f.path.display()),
        }
    }
    replays.sort_by_key(|r| r.match_id);
    let kills: usize = replays.iter().map(|r| r.kills.iter().filter(|k| k.killer.is_some()).count()).sum();
    println!("录像 {} 场，有击杀者的击杀 {kills} 次\n", replays.len());

    reconcile(&replays, fetch)?;
    let leads = fight_durations(&replays);
    clip_plans(&replays, &leads);
    Ok(())
}

/// 和官方统计对账：每个位置的击杀、死亡数要完全一致
fn reconcile(replays: &[Replay], fetch: bool) -> anyhow::Result<()> {
    let mut data: Official = std::fs::read_to_string(DATA).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    if fetch {
        let mut added = 0;
        let missing: Vec<u64> = replays.iter().map(|r| r.match_id).filter(|id| !data.contains_key(id)).collect();
        for id in missing {
            match replay::download::official_kills_deaths(id) {
                Ok(kd) => {
                    data.insert(id, kd);
                    added += 1;
                }
                Err(e) => println!("取不到 {id} 的官方数据：{e:#}"),
            }
            std::thread::sleep(std::time::Duration::from_millis(1100)); // OpenDota 免费接口每分钟 60 次
        }
        std::fs::create_dir_all(std::path::Path::new(DATA).parent().unwrap())?;
        std::fs::write(DATA, serde_json::to_string_pretty(&data)?)?;
        println!("对账数据新增 {added} 场，共 {} 场：{DATA}", data.len());
    }

    println!("## 和官方统计对账（击杀、死亡）");
    let (mut checked, mut bad) = (0, 0);
    for r in replays {
        let Some(official) = data.get(&r.match_id) else { continue };
        checked += 1;
        for (i, &(k, d)) in official.iter().enumerate() {
            let pk = r.kills.iter().filter(|x| x.killer == Some(i)).count() as u32;
            let pd = r.kills.iter().filter(|x| x.victim == i).count() as u32;
            if (pk, pd) != (k, d) {
                bad += 1;
                println!("  {} #{i}：解析 {pk}/{pd}，官方 {k}/{d}", r.match_id);
            }
        }
    }
    let missing = replays.len() - checked;
    println!("  核对 {checked} 场，不一致 {bad} 处{}\n", if missing > 0 { format!("（{missing} 场没有对账数据）") } else { String::new() });
    Ok(())
}

/// 各口径、各伤害间隔下"开打 → 击杀"的分布；返回每个组合的 90% 分位，供后面定起点上限
fn fight_durations(replays: &[Replay]) -> BTreeMap<(usize, u32), f32> {
    println!("## 开打 → 击杀要多久（往回最多找 60 秒）");
    println!("| 口径 | 伤害间隔 | 中位数 | 75% | 90% | 95% | ≤3s | ≤5s | ≤8s | ≤12s | ≤20s | 找不到伤害 |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|");
    let mut p90s = BTreeMap::new();
    for (ai, anchor) in Anchor::ALL.into_iter().enumerate() {
        for gap in [3.0_f32, 5.0] {
            let mut v = Vec::new();
            let mut none = 0;
            for r in replays {
                for k in r.kills.iter().filter(|k| k.killer.is_some()) {
                    match clips::fight_start(r, k, anchor, gap, 60.0) {
                        Some(s) => v.push(k.game_time - s),
                        None => none += 1,
                    }
                }
            }
            let (p50, p75, p90, p95) = (pct(&mut v, 0.5), pct(&mut v, 0.75), pct(&mut v, 0.9), pct(&mut v, 0.95));
            p90s.insert((ai, gap as u32), p90);
            println!(
                "| {} | {gap}s | {p50:.1}s | {p75:.1}s | {p90:.1}s | {p95:.1}s | {:.0}% | {:.0}% | {:.0}% | {:.0}% | {:.0}% | {:.1}% |",
                anchor.label(),
                share(&v, |x| x <= 3.0),
                share(&v, |x| x <= 5.0),
                share(&v, |x| x <= 8.0),
                share(&v, |x| x <= 12.0),
                share(&v, |x| x <= 20.0),
                none as f32 / (v.len() + none).max(1) as f32 * 100.0,
            );
        }
    }
    println!();
    p90s
}

/// 各参数组合下切出来的片段：数量、时长、导出次数、超长和中间拆分、预计耗时
fn clip_plans(replays: &[Replay], p90s: &BTreeMap<(usize, u32), f32>) {
    println!("## 片段计划（起点上限 = 该口径 90% 分位 + 开打前 2 秒，取整）");
    println!("预计耗时按每次导出固定 17.5 秒 + 渲染 1.8 倍实时估算，是一名玩家录完一整场的时间，不含启动游戏。\n");
    println!("| 口径 | 伤害间隔 | 起点上限 | 空档保留 N | 片段数 | 2 杀以上 | 时长中位数 | 时长 90% | 最长 | 导出次数 | 超过导出上限 | 中间拆分 | 每人每场耗时中位数 | 90% | 最长 |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for (ai, anchor) in Anchor::ALL.into_iter().enumerate() {
        for gap in [3.0_f32, 5.0] {
            let p90 = p90s[&(ai, gap as u32)];
            for keep in [5.0_f32, 8.0, 12.0] {
                let p = Params { anchor, damage_gap: gap, max_lead: (p90 + 2.0).ceil(), keep_gap: keep, ..Params::default() };
                let (mut clips_n, mut multi, mut exports, mut long, mut mid) = (0, 0, 0, 0, 0);
                let (mut durations, mut per_player) = (Vec::new(), Vec::new());
                for r in replays {
                    for player in 0..r.players.len() {
                        let cs = plan(r, player, &p);
                        if cs.is_empty() {
                            continue;
                        }
                        per_player.push(cs.iter().map(|c| c.estimated_seconds(&p)).sum::<f32>());
                        for c in &cs {
                            clips_n += 1;
                            multi += usize::from(c.kills.len() >= 2);
                            exports += c.exports.len();
                            long += c.spans.iter().filter(|s| s.len() > p.max_export).count();
                            mid += c.exports.iter().filter(|e| e.mid_action).count();
                            durations.push(c.duration());
                        }
                    }
                }
                let (d50, d90, dmax) = (pct(&mut durations, 0.5), pct(&mut durations, 0.9), pct(&mut durations, 1.0));
                let (t50, t90, tmax) = (pct(&mut per_player, 0.5), pct(&mut per_player, 0.9), pct(&mut per_player, 1.0));
                println!(
                    "| {} | {gap}s | {}s | {keep}s | {clips_n} | {multi} | {d50:.1}s | {d90:.1}s | {dmax:.1}s | {exports} | {long} | {mid} | {:.1} 分钟 | {:.1} 分钟 | {:.1} 分钟 |",
                    anchor.label(),
                    p.max_lead,
                    t50 / 60.0,
                    t90 / 60.0,
                    tmax / 60.0,
                );
            }
        }
    }
}
