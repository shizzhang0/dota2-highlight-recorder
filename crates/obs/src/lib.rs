//! 通过 obs-websocket 控制 OBS 录制。

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

pub use obws;

/// OBS 自己的 WebSocket 服务器设置（读 OBS 的配置文件，用户不用手动填端口和密码）
pub struct Settings {
    pub enabled: bool,
    pub port: u16,
    pub password: Option<String>,
}

fn config_path() -> Result<PathBuf> {
    let appdata = std::env::var_os("APPDATA").context("没有 APPDATA 环境变量")?;
    Ok(PathBuf::from(appdata).join(r"obs-studio\plugin_config\obs-websocket\config.json"))
}

/// 读取 OBS 的 WebSocket 设置。注意：OBS 在设置窗口点"确定"后才把改动写回这个文件
pub fn settings() -> Result<Settings> {
    let path = config_path()?;
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("读不到 {}，OBS 是否装过并打开过 WebSocket 设置？", path.display()))?;
    let v: serde_json::Value = serde_json::from_str(&text)?;
    let password = v["server_password"].as_str().filter(|p| !p.is_empty() && v["auth_required"] != false);
    Ok(Settings {
        enabled: v["server_enabled"].as_bool().unwrap_or(false),
        port: v["server_port"].as_u64().unwrap_or(4455) as u16,
        password: password.map(str::to_owned),
    })
}

/// 按 OBS 配置文件里的端口和密码连接本机 OBS
pub async fn connect() -> Result<obws::Client> {
    connect_with(None).await
}

/// 同 [`connect`]，可以额外订阅音量表这类高频事件（默认只收普通事件）
pub async fn connect_with(events: Option<obws::requests::EventSubscription>) -> Result<obws::Client> {
    let s = settings()?;
    if !s.enabled {
        bail!("OBS 的 WebSocket 服务器没开：工具 → WebSocket 服务器设置 → 开启，然后点确定");
    }
    let config = obws::client::ConnectConfig {
        host: "127.0.0.1",
        port: s.port,
        dangerous: None,
        password: s.password.as_deref(),
        event_subscriptions: events,
        broadcast_capacity: obws::client::DEFAULT_BROADCAST_CAPACITY,
        connect_timeout: obws::client::DEFAULT_CONNECT_TIMEOUT,
    };
    obws::Client::connect_with_config(config)
        .await
        .with_context(|| format!("连不上 OBS（127.0.0.1:{}）", s.port))
}
