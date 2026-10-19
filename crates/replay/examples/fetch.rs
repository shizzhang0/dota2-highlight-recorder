//! 按比赛 ID 下载录像并解析。
//!
//! cargo run --release -p replay --example fetch -- <比赛ID> <保存目录>

use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (Some(id), Some(dir)) = (args.get(1), args.get(2)) else {
        eprintln!("用法: fetch <比赛ID> <保存目录>");
        std::process::exit(2);
    };
    let match_id: u64 = id.parse()?;

    let url = replay::download::request_replay_url(match_id, Duration::from_secs(120))?;
    println!("录像地址 {url}");

    let dest = PathBuf::from(dir).join(format!("{match_id}.dem"));
    let started = Instant::now();
    replay::download::download(&url, &dest)?;
    println!("已保存 {}（{} 字节，用时 {:.1?}）", dest.display(), std::fs::metadata(&dest)?.len(), started.elapsed());

    let r = replay::parse(&dest)?;
    println!("解析成功：比赛 {}，{} 名玩家，{} 次击杀", r.match_id, r.players.len(), r.kills.len());
    Ok(())
}
