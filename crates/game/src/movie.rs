//! 用控制台命令 `startmovie` / `endmovie` 录回放：游戏逐帧渲染，每帧写一张图片，另写一个 WAV。
//!
//! 文件写到 `game\dota\movie\<时间>\<前缀>00000000.tga`……和 `<前缀>.wav`，目录名由游戏定，从控制台的
//! `Started recording movie: movie/<时间>/<前缀>.*: tga framerate 60.000000` 这一行读出来。
//! 每写完一帧才推进 1/帧率 秒的游戏时间，所以不掉帧，速度取决于渲染和写图片；游戏在后台也照样出帧。
//! 2026-11-04 / 11-06 实测（国服客户端）：TGA 在 2560×1600 是实时的 0.52 倍，1080p 是 0.84 倍；JPG 慢得多。

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::netcon::Netcon;

/// 一帧 TGA 图片
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// 从下往上存的（TGA 默认）
    pub bottom_up: bool,
    /// BGRA 像素
    pub bgra: Vec<u8>,
}

impl Frame {
    /// 画面中间（宽高各 30%–70%，避开 HUD）的平均亮度，0–255。
    /// 世界画面没画出来时（只有 HUD）约 30，正常约 70–80（2026-11-08 实测）
    pub fn center_brightness(&self) -> f32 {
        let (w, h) = (self.width as usize, self.height as usize);
        let (mut sum, mut n) = (0u64, 0u64);
        // 隔 4 个像素取一个，够用了
        for y in (h * 3 / 10..h * 7 / 10).step_by(4) {
            for x in (w * 3 / 10..w * 7 / 10).step_by(4) {
                let i = (y * w + x) * 4;
                sum += (self.bgra[i] as u64 + self.bgra[i + 1] as u64 + self.bgra[i + 2] as u64) / 3;
                n += 1;
            }
        }
        sum as f32 / n.max(1) as f32
    }
}

/// 读一张不压缩的 32 位 TGA（游戏 `startmovie ... tga` 写出的格式）
pub fn read_tga(path: &Path) -> Result<Frame> {
    let mut data = Vec::new();
    File::open(path).and_then(|mut f| f.read_to_end(&mut data)).with_context(|| format!("读不了 {}", path.display()))?;
    if data.len() < 18 {
        bail!("{} 不是完整的 TGA", path.display());
    }
    let id_len = data[0] as usize;
    let (kind, bpp, desc) = (data[2], data[16], data[17]);
    if kind != 2 || bpp != 32 {
        bail!("{} 是类型 {kind}、{bpp} 位的 TGA，只支持不压缩的 32 位", path.display());
    }
    let width = u16::from_le_bytes([data[12], data[13]]) as u32;
    let height = u16::from_le_bytes([data[14], data[15]]) as u32;
    let start = 18 + id_len;
    let len = width as usize * height as usize * 4;
    let bgra = data.get(start..start + len).with_context(|| format!("{} 没写完", path.display()))?.to_vec();
    // 图像描述字节的第 5 位为 1 表示从上往下存
    Ok(Frame { width, height, bottom_up: desc & 0x20 == 0, bgra })
}

/// 一次 `startmovie` 录制
pub struct Movie {
    dir: PathBuf,
    prefix: String,
    /// 下一帧的序号
    next: u32,
    /// 游戏说的总帧数（`endmovie` 之后才知道）
    total: Option<u32>,
}

impl Movie {
    /// 发 `startmovie <prefix> tga wav framerate <fps>`，从控制台输出读出写到哪个目录。
    /// 发完游戏不会马上出帧：回放暂停着时不推进，要调用方再 `demo_resume`
    pub fn start(con: &mut Netcon, prefix: &str, fps: u32) -> Result<Movie> {
        // 游戏会把文件名转成小写（2026-11-08 实测），统一用小写，好认出控制台那一行、删得掉自己的文件
        let prefix = prefix.to_lowercase();
        let prefix = prefix.as_str();
        con.drain(Duration::from_millis(100), Duration::from_millis(500));
        con.send(&format!("startmovie {prefix} tga wav framerate {fps}"))?;
        let line = con
            .wait_for(Duration::from_secs(10), |l| l.contains("Started recording movie:") && l.contains(prefix))
            .context("游戏没开始录（没看到 Started recording movie）")?;
        // movie/2026_11_06_11_57_22/d2hrspeed_1080_tga.*: tga framerate 60.000000
        let rel = line.split("Started recording movie:").nth(1).and_then(|s| s.split(".*").next()).map(str::trim).context("认不出录制输出的路径")?;
        let rel_dir = rel.rsplit_once('/').map(|(d, _)| d).context("认不出录制输出的目录")?;
        let dir = crate::launch::game_dir()?.join("dota").join(rel_dir.replace('/', "\\"));
        Ok(Movie { dir, prefix: prefix.to_owned(), next: 0, total: None })
    }

    fn frame_path(&self, n: u32) -> PathBuf {
        self.dir.join(format!("{}{n:08}.tga", self.prefix))
    }

    pub fn wav(&self) -> PathBuf {
        self.dir.join(format!("{}.wav", self.prefix))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 已经拿走的帧数
    pub fn taken(&self) -> u32 {
        self.next
    }

    /// 游戏已经开始写的最大帧序号 + 1（含正在写的那帧），用来看积压了多少
    pub fn written(&self) -> u32 {
        let mut n = self.next;
        while self.frame_path(n).is_file() {
            n += 1;
        }
        n
    }

    /// 下一帧写完了就读出来并删掉文件，没写完返回 None。
    /// 写完的判断：后一帧的文件已经出现（游戏按顺序写），或者已经 `endmovie`、总帧数已知
    pub fn take(&mut self) -> Result<Option<Frame>> {
        let path = self.frame_path(self.next);
        let done = match self.total {
            Some(total) => self.next < total,
            None => self.frame_path(self.next + 1).is_file(),
        };
        if !done || !path.is_file() {
            return Ok(None);
        }
        let frame = read_tga(&path)?;
        let _ = std::fs::remove_file(&path);
        self.next += 1;
        Ok(Some(frame))
    }

    /// 发 `endmovie`，等游戏报出总帧数（`Finishing movie: ... [231 frames @ 60.000000 == 3.850000 seconds]`）
    pub fn end(&mut self, con: &mut Netcon) -> Result<u32> {
        con.drain(Duration::from_millis(50), Duration::from_millis(200));
        con.send("endmovie")?;
        let line = con.wait_for(Duration::from_secs(10), |l| l.contains("Finishing movie:")).context("游戏没报录制结束（没看到 Finishing movie）")?;
        let total = line
            .split('[')
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .context("认不出录制的总帧数")?;
        self.total = Some(total);
        Ok(total)
    }

    /// 结束了没有（发过 `endmovie`、拿到了总帧数）
    pub fn ended(&self) -> bool {
        self.total.is_some()
    }

    /// 删掉这次录制剩下的文件（WAV 用完之后调用），目录空了也删。
    /// 只删自己前缀的文件：同一秒里开始的两次录制会写进同一个目录
    pub fn remove(&self) {
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                if e.file_name().to_str().is_some_and(|n| n.starts_with(&self.prefix)) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        // 不空（别的录制的文件）时删不掉，正好
        let _ = std::fs::remove_dir(&self.dir);
    }
}

/// 删掉 `game\dota\movie` 下名字以 `prefix` 开头的文件（上一轮程序崩溃或被强关时留下的帧），目录空了也删
pub fn clean_leftovers(prefix: &str) {
    let Ok(root) = crate::launch::game_dir().map(|d| d.join(r"dota\movie")) else { return };
    let Ok(dirs) = std::fs::read_dir(&root) else { return };
    for dir in dirs.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        if let Ok(files) = std::fs::read_dir(&dir) {
            for f in files.flatten() {
                if f.file_name().to_str().is_some_and(|n| n.starts_with(prefix)) {
                    let _ = std::fs::remove_file(f.path());
                }
            }
        }
        let _ = std::fs::remove_dir(&dir);
    }
}
