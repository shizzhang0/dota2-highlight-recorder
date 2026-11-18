//! 应用里播放视频用的本机 HTTP 服务（2026-11-18 起）。
//!
//! 原来经 Tauri 的 asset 协议读文件，用户反馈有时卡顿。asset 协议每次最多回 1000 KB，
//! 我们的视频约 25 Mbit/s（每秒 3 MB 多），播放器每秒要发三四次请求，每次都要经过主窗口的消息线程，
//! 界面一忙（生成缩略图、刷新页面）数据就跟不上。改成播放器自己的网络栈直接从本机读：
//! - 只听 127.0.0.1，端口随机；
//! - 只放行刚点开的那一个 mp4，地址里带随机令牌，换一个视频令牌就换；
//! - 支持 Range（拖进度条、播放器分段读）。

use std::fs::File;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use tiny_http::{Header, Method, Response, Server, StatusCode};

/// 现在放行的视频：(令牌, 文件)
type Current = Arc<Mutex<Option<(String, PathBuf)>>>;

struct VideoServer {
    port: u16,
    current: Current,
}

static SERVER: OnceLock<VideoServer> = OnceLock::new();

fn server() -> Result<&'static VideoServer, String> {
    if let Some(s) = SERVER.get() {
        return Ok(s);
    }
    let http = Server::http("127.0.0.1:0").map_err(|e| format!("开不了本机播放服务：{e}"))?;
    let port = http.server_addr().to_ip().map(|a| a.port()).ok_or("本机播放服务没有端口")?;
    let current: Current = Arc::new(Mutex::new(None));
    let shared = current.clone();
    std::thread::spawn(move || {
        for req in http.incoming_requests() {
            let current = shared.clone();
            // 播放器会同时发几个请求（开头、结尾的索引、拖动后的位置），一个请求一个线程，慢的不挡别的
            std::thread::spawn(move || handle(req, &current));
        }
    });
    Ok(SERVER.get_or_init(|| VideoServer { port, current }))
}

/// 随机令牌：系统给每个 RandomState 的随机种子，够防别的本机网页猜地址
fn token() -> String {
    let a = std::collections::hash_map::RandomState::new().build_hasher().finish();
    let b = std::collections::hash_map::RandomState::new().build_hasher().finish();
    format!("{a:016x}{b:016x}")
}

/// 播放前调用：放行这个 mp4，返回给 `<video>` 用的地址
#[tauri::command]
pub fn video_url(path: String) -> Result<String, String> {
    let p = Path::new(&path);
    if !p.is_file() {
        return Err(format!("文件不在了：{path}"));
    }
    if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")) {
        return Err(format!("不是 mp4 视频：{path}"));
    }
    let s = server()?;
    let t = token();
    *s.current.lock().unwrap() = Some((t.clone(), p.to_path_buf()));
    Ok(format!("http://127.0.0.1:{}/{t}/video.mp4", s.port))
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("合法的响应头")
}

fn status(req: tiny_http::Request, code: u16) {
    let _ = req.respond(Response::empty(StatusCode(code)));
}

/// 解析 `Range: bytes=起-止`（只认单段；`起-` 到结尾、`-N` 最后 N 字节也认）。不合法返回 None
fn parse_range(value: &str, len: u64) -> Option<(u64, u64)> {
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = match (a.trim(), b.trim()) {
        ("", n) => {
            let n: u64 = n.parse().ok()?;
            (len.saturating_sub(n), len.checked_sub(1)?)
        }
        (s, "") => (s.parse().ok()?, len.checked_sub(1)?),
        (s, e) => (s.parse().ok()?, e.parse::<u64>().ok()?.min(len.checked_sub(1)?)),
    };
    (start <= end && end < len).then_some((start, end))
}

fn handle(req: tiny_http::Request, current: &Current) {
    if !matches!(req.method(), Method::Get | Method::Head) {
        return status(req, 405);
    }
    let file = {
        let cur = current.lock().unwrap();
        match cur.as_ref() {
            Some((t, p)) if req.url() == format!("/{t}/video.mp4") => p.clone(),
            _ => return status(req, 404),
        }
    };
    let Ok(mut f) = File::open(&file) else { return status(req, 404) };
    let Ok(len) = f.metadata().map(|m| m.len()) else { return status(req, 500) };
    let range = req.headers().iter().find(|h| h.field.equiv("Range")).map(|h| h.value.as_str().to_owned());
    let (code, start, end) = match range {
        Some(r) => match parse_range(&r, len) {
            Some((s, e)) => (206, s, e),
            None => {
                let resp = Response::empty(StatusCode(416)).with_header(header("Content-Range", &format!("bytes */{len}")));
                let _ = req.respond(resp);
                return;
            }
        },
        None => (200, 0, len.saturating_sub(1)),
    };
    let body_len = if len == 0 { 0 } else { end - start + 1 };
    if f.seek(SeekFrom::Start(start)).is_err() {
        return status(req, 500);
    }
    let mut headers = vec![
        header("Content-Type", "video/mp4"),
        header("Accept-Ranges", "bytes"),
        header("Cache-Control", "no-store"),
    ];
    if code == 206 {
        headers.push(header("Content-Range", &format!("bytes {start}-{end}/{len}")));
    }
    let body: Box<dyn Read + Send> = if *req.method() == Method::Head { Box::new(std::io::empty()) } else { Box::new(BufReader::with_capacity(256 * 1024, f).take(body_len)) };
    // 长度已知，总是带 Content-Length（tiny_http 默认大于 32 KB 就改成分块传输、不给长度）。
    // 播放器关掉或拖走时会断开连接，写失败就算了
    let resp = Response::new(StatusCode(code), headers, body, Some(body_len as usize), None).with_chunked_threshold(usize::MAX);
    let _ = req.respond(resp);
}
