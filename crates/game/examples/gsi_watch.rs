//! 接收 GSI 推送：每包打印一行摘要，并把完整内容按行追加写入文件（JSONL）。
//!
//! cargo run -p game --example gsi_watch -- [--port 2122] [--out gsi.jsonl] [--secs 秒]
//!
//! 只收我们自己的 GSI 配置推来的包（token 不对的会被丢掉），配置要先装好、游戏重启后才有包。

use std::io::Write;
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut port, mut out, mut secs) = (game::gsi::DEFAULT_PORT, None, 600.0);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().expect("--port 后面要跟端口").parse()?,
            "--out" => out = args.next(),
            "--secs" => secs = args.next().expect("--secs 后面要跟秒数").parse()?,
            _ => anyhow::bail!("不认识的参数 {a}"),
        }
    }

    let rx = game::gsi::listen(port)?;
    let mut file = out.map(std::fs::File::create).transpose()?;
    eprintln!("监听 http://127.0.0.1:{port}/");
    let started = Instant::now();
    let deadline = Duration::from_secs_f64(secs);
    while let Some(left) = deadline.checked_sub(started.elapsed()) {
        let Ok(v) = rx.recv_timeout(left) else { break };
        if let Some(f) = file.as_mut() {
            writeln!(f, "{}", serde_json::to_string(&v)?)?;
        }
        let map = &v["map"];
        let keys: Vec<&str> = v.as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
        println!(
            "{:>7.2}s clock={} game_time={} state={} paused={} keys={keys:?}",
            started.elapsed().as_secs_f64(),
            map["clock_time"],
            map["game_time"],
            map["game_state"],
            map["paused"],
        );
    }
    Ok(())
}
