//! 通过 netcon 依次执行控制台命令，打印游戏输出。
//!
//! cargo run -p game --example console -- [--port 2121] [--wait 秒] <命令> [<命令> ...]
//! 每条命令发出后，等输出安静下来（或到 --wait 秒）再发下一条。

use std::time::Duration;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1).peekable();
    let mut port = 2121;
    let mut wait = 3.0;
    let mut commands = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().expect("--port 后面要跟端口").parse()?,
            "--wait" => wait = args.next().expect("--wait 后面要跟秒数").parse()?,
            _ => commands.push(a),
        }
    }

    let mut con = game::netcon::Netcon::connect(port)?;
    // 连上时游戏可能先推一段积压的输出，丢掉
    con.drain(Duration::from_millis(300), Duration::from_secs(2));
    for c in &commands {
        println!(">>> {c}");
        con.send(c)?;
        for line in con.drain(Duration::from_millis(800), Duration::from_secs_f64(wait)) {
            println!("{line}");
        }
    }
    Ok(())
}
