//! 打印录像里的玩家、英雄和击杀时间。
//!
//! cargo run --release -p replay --example dump -- <录像.dem> [--json]

use std::time::Instant;

fn clock(t: f32) -> String {
    let sign = if t < 0.0 { "-" } else { "" };
    let s = t.abs().round() as u32;
    format!("{sign}{:02}:{:02}", s / 60, s % 60)
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("用法: dump <录像.dem> [--json]");
        std::process::exit(2);
    };

    let started = Instant::now();
    let r = replay::parse(path)?;
    let elapsed = started.elapsed();

    if args.iter().any(|a| a == "--json") {
        println!("{}", serde_json::to_string_pretty(&r)?);
        return Ok(());
    }

    let hero = |id: usize| r.players[id].hero.trim_start_matches("npc_dota_hero_");
    println!("比赛 {}  解析耗时 {:.2?}", r.match_id, elapsed);
    println!("\n玩家：");
    for p in &r.players {
        let kills = r.kills.iter().filter(|k| k.killer == Some(p.player_id)).count();
        let deaths = r.kills.iter().filter(|k| k.victim == p.player_id).count();
        let side = if p.team == 2 { "天辉" } else { "夜魇" };
        println!(
            "  #{} {side} {:<20} {:<12} 账号 {:<10} 击杀 {kills:>2} 死亡 {deaths:>2}",
            p.player_id,
            hero(p.player_id),
            p.name,
            p.account_id
        );
    }

    println!("\n击杀（共 {}）：", r.kills.len());
    for k in &r.kills {
        let killer = match k.killer {
            Some(id) => hero(id).to_string(),
            None => format!("({})", k.killer_unit),
        };
        println!("  {} tick {:>6}  {killer} → {}", clock(k.game_time), k.tick, hero(k.victim));
    }

    println!("\n多杀：");
    for e in &r.multi_kills {
        println!("  {} tick {:>6}  {} {} 连斩", clock(e.game_time), e.tick, hero(e.player), e.count);
    }
    println!("\n连杀：");
    for e in &r.kill_streaks {
        println!("  {} tick {:>6}  {} 连杀 {}", clock(e.game_time), e.tick, hero(e.player), e.count);
    }
    Ok(())
}
