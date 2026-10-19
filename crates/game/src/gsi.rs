//! 接收 Game State Integration（GSI）推送。
//!
//! 游戏启动时读取 `game/dota/cfg/gamestate_integration/*.cfg`，之后按配置把状态以 JSON POST 到指定地址。
//! 配置只在启动时读，改了要重启游戏才生效。

use std::io::Read;
use std::sync::mpsc::{self, Receiver};

use anyhow::{Result, anyhow};

/// 一包的上限。Dota 的包一般几 KB 到几十 KB
const MAX_BODY: usize = 4 * 1024 * 1024;

/// 在 addr（如 `127.0.0.1:53000`）上监听，收到的每个包解析成 JSON 送进通道。收包先回 200，不让游戏等
pub fn listen(addr: &str) -> Result<Receiver<serde_json::Value>> {
    let server = tiny_http::Server::http(addr).map_err(|e| anyhow!("无法监听 {addr}: {e}"))?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for mut req in server.incoming_requests() {
            if req.body_length().is_some_and(|n| n > MAX_BODY) {
                let _ = req.respond(tiny_http::Response::from_string("too large").with_status_code(413));
                continue;
            }
            let mut body = String::new();
            let _ = req.as_reader().take(MAX_BODY as u64 + 1).read_to_string(&mut body);
            let _ = req.respond(tiny_http::Response::from_string("ok"));
            if body.len() > MAX_BODY {
                continue;
            }
            if let Ok(v) = serde_json::from_str(&body)
                && tx.send(v).is_err()
            {
                break;
            }
        }
    });
    Ok(rx)
}
