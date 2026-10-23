//! Game State Integration（GSI）：写入我们自己的配置文件，接收游戏推送的状态。
//!
//! 游戏启动时读取 `game/dota/cfg/gamestate_integration/*.cfg`，之后按配置把状态以 JSON POST 到指定地址。
//! 配置只在启动时读，改了要重启游戏才生效，所以端口和 token 都是固定的。
//! 请求体上限、token 校验、写配置的写法移植自 dota2-game-helper2 的 `gsi.rs` / `gsicfg.rs`。

use std::io::Read;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

/// 默认端口：紧挨 netcon 的 2121，不在 Windows 临时端口范围内，避开 Steam 和 helper2 的 53000
pub const DEFAULT_PORT: u16 = 2122;
/// 固定 token：配置只在游戏启动时读，随机生成就意味着"游戏已开着"时永远得重启
const TOKEN: &str = "d2hr";
const CFG_NAME: &str = "gamestate_integration_d2hr.cfg";
/// 一包的上限。Dota 的包一般几 KB 到几十 KB，4MB 是"明显不是 Dota"
const MAX_BODY: usize = 4 * 1024 * 1024;

fn cfg_text(port: u16) -> String {
    // heartbeat 1 秒：回放暂停时状态不变，没有心跳就收不到包，等确认时会误判超时。
    // data 只要 map 和 hero：用到 map.clock_time / paused / matchid / game_state 和 hero.*.selected_unit
    format!(
        r#""d2hr"
{{
  "uri" "http://127.0.0.1:{port}/"
  "timeout" "5.0"
  "buffer" "0.1"
  "throttle" "0.1"
  "heartbeat" "1.0"
  "auth"
  {{
    "token" "{TOKEN}"
  }}
  "data"
  {{
    "map" "1"
    "hero" "1"
  }}
}}
"#
    )
}

fn cfg_path() -> Result<PathBuf> {
    Ok(crate::launch::game_dir()?.join(r"dota\cfg\gamestate_integration").join(CFG_NAME))
}

/// 写入我们的 GSI 配置（内容相同就不重写）。返回配置是否有变化：有变化而游戏已经开着，就得重启游戏才生效
pub fn install(port: u16) -> Result<bool> {
    let path = cfg_path()?;
    let text = cfg_text(port);
    if std::fs::read_to_string(&path).is_ok_and(|s| s == text) {
        return Ok(false);
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, text).with_context(|| format!("写不了 GSI 配置 {}", path.display()))?;
    Ok(true)
}

/// 删掉我们的 GSI 配置（只删自己那一个文件）。上次异常退出留下的也用它清
pub fn remove() -> Result<()> {
    let path = cfg_path()?;
    if path.is_file() {
        std::fs::remove_file(&path).with_context(|| format!("删不掉 GSI 配置 {}", path.display()))?;
    }
    Ok(())
}

/// 端口能不能绑定。被占用就报错提示换端口，不悄悄换（换了配置就得重启游戏）
pub fn check_port(port: u16) -> Result<()> {
    TcpListener::bind(("127.0.0.1", port))
        .map(drop)
        .map_err(|e| anyhow!("GSI 端口 {port} 被占用（{e}），用 --gsi-port 换一个"))
}

/// 监听 127.0.0.1:port，收到的每个包解析成 JSON 送进通道。收包先回 200，不让游戏等。
/// 本机 dota2-game-helper2 也在收 GSI，两份配置都会生效，所以要认 token：
/// 带了 auth 但 token 不对的丢弃；没带 auth 的放行（只警告一次）
pub fn listen(port: u16) -> Result<Receiver<Value>> {
    let server = tiny_http::Server::http(("127.0.0.1", port)).map_err(|e| anyhow!("无法监听 GSI 端口 {port}: {e}"))?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut warned = false;
        for mut req in server.incoming_requests() {
            if req.body_length().is_some_and(|n| n > MAX_BODY) {
                let _ = req.respond(tiny_http::Response::from_string("too large").with_status_code(413));
                continue;
            }
            let mut body = String::new();
            let _ = req.as_reader().take(MAX_BODY as u64 + 1).read_to_string(&mut body);
            let _ = req.respond(tiny_http::Response::from_string("ok"));
            let Ok(mut v) = serde_json::from_str::<Value>(&body) else { continue };
            match v.get("auth").and_then(|a| a.get("token")).and_then(Value::as_str) {
                Some(TOKEN) => {}
                Some(_) => continue,
                None => {
                    if !warned {
                        warned = true;
                        eprintln!("[gsi] 包里没有 auth 段，先照收（可能是旧配置）");
                    }
                }
            }
            if let Some(o) = v.as_object_mut() {
                o.remove("auth");
            }
            if tx.send(v).is_err() {
                break;
            }
        }
    });
    Ok(rx)
}

/// 我们关心的几个字段
#[derive(Debug, Clone, Default)]
pub struct State {
    pub match_id: Option<u64>,
    /// 游戏时钟（秒，号角为 0），只精确到秒
    pub clock: Option<i64>,
    pub paused: bool,
    pub game_state: String,
    /// 当前追踪（选中）的玩家序号 0–9
    pub selected: Option<usize>,
}

impl State {
    pub fn from_json(v: &Value) -> State {
        let map = &v["map"];
        let mut selected = None;
        for team in ["team2", "team3"] {
            if let Some(o) = v["hero"][team].as_object() {
                for (k, h) in o {
                    if h["selected_unit"] == true {
                        selected = k.strip_prefix("player").and_then(|s| s.parse().ok());
                    }
                }
            }
        }
        State {
            match_id: map["matchid"].as_str().and_then(|s| s.parse().ok()),
            clock: map["clock_time"].as_i64(),
            paused: map["paused"] == true,
            game_state: map["game_state"].as_str().unwrap_or_default().to_string(),
            selected,
        }
    }
}

/// 清掉通道里积压的旧包
pub fn drain(rx: &Receiver<Value>) {
    while rx.try_recv().is_ok() {}
}

/// 等到满足条件的状态；`timeout` 内没等到就报错。先清掉通道里积压的旧包
pub fn wait_for(rx: &Receiver<Value>, timeout: std::time::Duration, what: &str, pred: impl FnMut(&State) -> bool) -> Result<State> {
    drain(rx);
    wait_next(rx, timeout, what, pred)
}

/// 同 `wait_for`，但不清旧包。用于"先 drain、再操作、再等"：回放暂停时几秒才一个包，
/// 操作之后再清，可能正好把反映操作结果的那个包清掉（2026-10-22 测试时按键追踪就这样误判超时）
pub fn wait_next(rx: &Receiver<Value>, timeout: std::time::Duration, what: &str, mut pred: impl FnMut(&State) -> bool) -> Result<State> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            bail!("等了 {timeout:?} 还没等到：{what}（GSI 没收到包？游戏可能需要重启才能读到配置）");
        }
        crate::cancel::check()?;
        // 分小段等，好及时响应 Ctrl+C
        if let Ok(v) = rx.recv_timeout(left.min(std::time::Duration::from_millis(300))) {
            let s = State::from_json(&v);
            if pred(&s) {
                return Ok(s);
            }
        }
    }
}
