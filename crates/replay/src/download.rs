//! 按比赛 ID 下载录像。
//!
//! 录像地址形如 `http://replay{cluster}.valve.net/570/{match_id}_{salt}.dem.bz2`（国服是 dota2.com.cn），
//! 其中 salt 只有游戏协调服务器（GC）给得出。这里借 OpenDota：它取到 salt 后会在比赛详情里给出 replay_url，
//! 没有时可以请求它去取。录像在 Valve 服务器上只保留一段时间，过期后就下不到了
//! （2026-10-27 实测国服 08-19 的录像两个多月后仍能下载，保留多久说不准，只能真去下）。
//!
//! 只用 OpenDota 的"按比赛查"：它的"按玩家列比赛"会落后几个月（2026-10-27 实测），不可靠。

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

const OPENDOTA: &str = "https://api.opendota.com/api";

/// 确定下不到的原因（不是网络之类的临时问题），界面按种类给出提示
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// OpenDota 没有这场比赛（比赛 ID 不对）
    NoMatch,
    /// 等了一阵 OpenDota 还是没取到录像地址
    NoUrl,
    /// Valve 服务器上已经没有这个录像文件
    Expired,
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(match self {
            Unavailable::NoMatch => "OpenDota 没有这场比赛，比赛 ID 不对？",
            Unavailable::NoUrl => "OpenDota 取不到这场的录像地址",
            Unavailable::Expired => "录像已过期，Valve 服务器上已经没有这个文件",
        })
    }
}

impl std::error::Error for Unavailable {}

/// 404 / 410 当作"没有这个东西"
fn not_found(e: &ureq::Error) -> bool {
    matches!(e, ureq::Error::StatusCode(404 | 410))
}

fn agent() -> Agent {
    Agent::config_builder()
        // 用 Windows 证书库校验，而不是 ureq 自带的根证书列表
        .tls_config(TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build())
        .timeout_global(Some(Duration::from_secs(300)))
        .build()
        .into()
}

fn opendota_match(match_id: u64) -> Result<serde_json::Value> {
    let mut resp = match agent().get(format!("{OPENDOTA}/matches/{match_id}")).call() {
        Err(e) if not_found(&e) => return Err(Unavailable::NoMatch.into()),
        r => r.context("查询 OpenDota 比赛详情失败")?,
    };
    resp.body_mut()
        .read_json()
        .context("OpenDota 返回的不是 JSON")
}

/// OpenDota 已知的录像地址；还没取到 salt 时为 None
pub fn replay_url(match_id: u64) -> Result<Option<String>> {
    let v = opendota_match(match_id)?;
    Ok(v["replay_url"].as_str().filter(|s| !s.is_empty()).map(str::to_owned))
}

/// OpenDota（即 Valve 官方统计）里每个玩家的击杀、死亡数，按录像里的玩家序号 0–9 排列，
/// 用来核对解析结果。OpenDota 的 player_slot 是 0–4 和 128–132
pub fn official_kills_deaths(match_id: u64) -> Result<Vec<(u32, u32)>> {
    let v = opendota_match(match_id)?;
    let players = v["players"].as_array().context("OpenDota 没有这场比赛的玩家数据")?;
    let mut out = vec![(0, 0); 10];
    for p in players {
        let slot = p["player_slot"].as_u64().context("缺 player_slot")? as usize;
        let i = if slot >= 128 { slot - 128 + 5 } else { slot };
        if i < 10 {
            out[i] = (p["kills"].as_u64().unwrap_or(0) as u32, p["deaths"].as_u64().unwrap_or(0) as u32);
        }
    }
    Ok(out)
}

/// 按比赛 ID 下载录像到 `dest`：先查（必要时请 OpenDota 去取）录像地址，下载到临时文件，完整解压后再改名，
/// 免得游戏或列表读到下了一半的文件。确定下不到时，错误里带 [`Unavailable`]（用 `downcast_ref` 取）
pub fn download_match(match_id: u64, dest: &Path) -> Result<()> {
    let url = request_replay_url(match_id, Duration::from_secs(120))?;
    let part = dest.with_extension("dem.part");
    let result = download(&url, &part).and_then(|()| std::fs::rename(&part, dest).context("下载完了却改不了名"));
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// 请 OpenDota 去取这场比赛的录像信息，然后等它给出地址（每 10 秒问一次，免得请求太多）
pub fn request_replay_url(match_id: u64, wait: Duration) -> Result<String> {
    if let Some(url) = replay_url(match_id)? {
        return Ok(url);
    }
    agent().post(format!("{OPENDOTA}/request/{match_id}")).send_empty().context("请求 OpenDota 解析失败")?;
    let started = Instant::now();
    while started.elapsed() < wait {
        std::thread::sleep(Duration::from_secs(10));
        if let Some(url) = replay_url(match_id)? {
            return Ok(url);
        }
    }
    Err(Unavailable::NoUrl.into())
}

/// 下载并解压到 dest（.dem）。服务器上的文件虽然叫 .dem.bz2，实际可能是 zstd 压缩，按文件头识别
pub fn download(url: &str, dest: &Path) -> Result<()> {
    let mut resp = match agent().get(url).call() {
        Err(e) if not_found(&e) => return Err(Unavailable::Expired.into()),
        r => r.with_context(|| format!("下载失败 {url}"))?,
    };
    let mut body = Vec::new();
    resp.body_mut().with_config().limit(u64::MAX).reader().read_to_end(&mut body)?;

    let mut out = BufWriter::new(File::create(dest).with_context(|| format!("无法写入 {}", dest.display()))?);
    match body.get(..4) {
        Some([0x28, 0xB5, 0x2F, 0xFD]) => {
            let mut d = ruzstd::decoding::StreamingDecoder::new(body.as_slice()).context("zstd 解压失败")?;
            std::io::copy(&mut d, &mut out)?;
        }
        Some([b'B', b'Z', b'h', _]) => {
            std::io::copy(&mut bzip2::read::BzDecoder::new(body.as_slice()), &mut out)?;
        }
        Some([b'P', b'B', b'D', b'E']) => out.write_all(&body)?,
        _ => bail!("无法识别的录像文件格式"),
    }
    out.flush()?;
    Ok(())
}
