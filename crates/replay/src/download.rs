//! 按比赛 ID 下载录像。
//!
//! 录像地址形如 `http://replay{cluster}.valve.net/570/{match_id}_{salt}.dem.bz2`（国服是 dota2.com.cn），
//! 其中 salt 只有游戏协调服务器（GC）给得出。这里借 OpenDota：它取到 salt 后会在比赛详情里给出 replay_url，
//! 没有时可以请求它去取。录像在 Valve 服务器上只保留一段时间，过期后就下不到了
//! （2026-10-27 实测国服 08-19 的录像两个多月后仍能下载，保留多久说不准，只能真去下）。
//!
//! 只用 OpenDota 的"按比赛查"：它的"按玩家列比赛"会落后几个月（2026-10-27 实测），不可靠。
//!
//! 下载时报进度（`Progress`），可以取消（`cancel` 标记设成 true，错误里带 [`Cancelled`]）；边下边解压写盘，不把整个压缩包读进内存。

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// 用户取消了下载
#[derive(Debug, Clone, Copy)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("下载已取消")
    }
}

impl std::error::Error for Cancelled {}

/// 下载进行到哪一步了，报给界面
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// 向 OpenDota 要录像地址
    FindingUrl,
    /// OpenDota 还没有地址，已经请它去取，等了 `waited_secs` 秒（最多等 `max_secs` 秒）
    WaitingForUrl { waited_secs: u64, max_secs: u64 },
    /// 下载中：已经下了 `done` 字节（压缩包），共 `total` 字节（服务器没报大小就是 None）
    Downloading { done: u64, total: Option<u64> },
    /// 下完了，正在写完、改名
    Saving,
}

/// 进度最多这么久报一次
const REPORT_EVERY: Duration = Duration::from_millis(200);

fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) {
        return Err(Cancelled.into());
    }
    Ok(())
}

/// 404 / 410 当作"没有这个东西"
fn not_found(e: &ureq::Error) -> bool {
    matches!(e, ureq::Error::StatusCode(404 | 410))
}

/// 网络问题（连接被断、TLS 握手失败、超时）重试 2 次，每次隔 2 秒；服务器给了明确状态码（404 等）的不重试。
/// 2026-11-10 实测连 OpenDota 偶尔握手时被断（`native-tls: unexpected EOF during handshake`），马上再试就好了
fn with_retry<T>(mut call: impl FnMut() -> Result<T, ureq::Error>) -> Result<T, ureq::Error> {
    let mut tries = 0;
    loop {
        match call() {
            Err(e) if !matches!(e, ureq::Error::StatusCode(_)) && tries < 2 => {
                tries += 1;
                std::thread::sleep(Duration::from_secs(2));
            }
            r => return r,
        }
    }
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
    let mut resp = match with_retry(|| agent().get(format!("{OPENDOTA}/matches/{match_id}")).call()) {
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
/// 免得游戏或列表读到下了一半的文件。确定下不到时，错误里带 [`Unavailable`]；取消了带 [`Cancelled`]（都用 `downcast_ref` 取）
pub fn download_match(match_id: u64, dest: &Path, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<()> {
    let url = request_replay_url(match_id, Duration::from_secs(120), progress, cancel)?;
    let part = dest.with_extension("dem.part");
    let result = download(&url, &part, progress, cancel).and_then(|()| std::fs::rename(&part, dest).context("下载完了却改不了名"));
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// 请 OpenDota 去取这场比赛的录像信息，然后等它给出地址（每 10 秒问一次，免得请求太多；每秒报一次等了多久）
pub fn request_replay_url(match_id: u64, wait: Duration, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<String> {
    progress(Progress::FindingUrl);
    if let Some(url) = replay_url(match_id)? {
        return Ok(url);
    }
    check(cancel)?;
    with_retry(|| agent().post(format!("{OPENDOTA}/request/{match_id}")).send_empty()).context("请求 OpenDota 解析失败")?;
    let started = Instant::now();
    let mut asked = Instant::now();
    while started.elapsed() < wait {
        progress(Progress::WaitingForUrl { waited_secs: started.elapsed().as_secs(), max_secs: wait.as_secs() });
        for _ in 0..4 {
            check(cancel)?;
            std::thread::sleep(Duration::from_millis(250));
        }
        if asked.elapsed() >= Duration::from_secs(10) {
            asked = Instant::now();
            if let Some(url) = replay_url(match_id)? {
                return Ok(url);
            }
        }
    }
    Err(Unavailable::NoUrl.into())
}

/// 读压缩包时顺便数字节、报进度、看有没有取消
struct Counting<'a, R> {
    inner: R,
    done: u64,
    total: Option<u64>,
    reported: Instant,
    progress: &'a mut dyn FnMut(Progress),
    cancel: &'a AtomicBool,
}

impl<R: Read> Read for Counting<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // 取消了就让读出错（不能用 Interrupted：io::copy 会重试），调用方看标记换成 Cancelled
        if self.cancel.load(Ordering::SeqCst) {
            return Err(std::io::Error::other(Cancelled));
        }
        let n = self.inner.read(buf)?;
        self.done += n as u64;
        if self.reported.elapsed() >= REPORT_EVERY || n == 0 {
            self.reported = Instant::now();
            (self.progress)(Progress::Downloading { done: self.done, total: self.total });
        }
        Ok(n)
    }
}

/// 下载并解压到 dest（.dem），边下边解压。服务器上的文件虽然叫 .dem.bz2，实际可能是 zstd 压缩，按文件头识别
pub fn download(url: &str, dest: &Path, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<()> {
    check(cancel)?;
    let mut resp = match with_retry(|| agent().get(url).call()) {
        Err(e) if not_found(&e) => return Err(Unavailable::Expired.into()),
        r => r.with_context(|| format!("下载失败 {url}"))?,
    };
    let total = resp.body().content_length();
    progress(Progress::Downloading { done: 0, total });
    let body = resp.body_mut().with_config().limit(u64::MAX).reader();
    let counting = Counting { inner: body, done: 0, total, reported: Instant::now(), progress: &mut *progress, cancel };
    let mut src = BufReader::with_capacity(1 << 16, counting);
    let head: Vec<u8> = src.fill_buf()?.iter().take(4).copied().collect();

    let mut out = BufWriter::new(File::create(dest).with_context(|| format!("无法写入 {}", dest.display()))?);
    let copied = match head.as_slice() {
        [0x28, 0xB5, 0x2F, 0xFD] => {
            let mut d = ruzstd::decoding::StreamingDecoder::new(&mut src).context("zstd 解压失败")?;
            std::io::copy(&mut d, &mut out)
        }
        [b'B', b'Z', b'h', _] => std::io::copy(&mut bzip2::read::BzDecoder::new(&mut src), &mut out),
        [b'P', b'B', b'D', b'E'] => std::io::copy(&mut src, &mut out),
        _ => bail!("无法识别的录像文件格式"),
    };
    if copied.is_err() {
        check(cancel)?;
    }
    copied.context("下载或解压失败")?;
    drop(src);
    progress(Progress::Saving);
    out.flush()?;
    Ok(())
}
