//! 视频文件的检查与处理。
//!
//! 导出后核对时长不用 ffprobe：mp4 的总时长就写在 `moov/mvhd` 里，直接读，少一个外部依赖。
//! 裁剪、拼接调用 ffmpeg（`ffmpeg` 模块）。

pub mod ffmpeg;

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result, bail};

fn read_u32(r: &mut impl Read) -> Result<u32> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

fn read_u64(r: &mut impl Read) -> Result<u64> {
    let mut b = [0; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_be_bytes(b))
}

/// 在 `[start, end)` 里逐个跳过 box，找到类型为 `kind` 的，返回它内容的起止位置
fn find_box(r: &mut (impl Read + Seek), start: u64, end: u64, kind: &[u8; 4]) -> Result<Option<(u64, u64)>> {
    let mut pos = start;
    while pos + 8 <= end {
        r.seek(SeekFrom::Start(pos))?;
        let size = read_u32(r)? as u64;
        let mut t = [0; 4];
        r.read_exact(&mut t)?;
        let (header, size) = match size {
            0 => (8, end - pos),
            1 => (16, read_u64(r)?),
            n => (8, n),
        };
        if size < header || pos + size > end {
            bail!("mp4 结构损坏（位置 {pos} 的 box 大小 {size}）");
        }
        if &t == kind {
            return Ok(Some((pos + header, pos + size)));
        }
        pos += size;
    }
    Ok(None)
}

/// mp4 的总时长（秒），读 `moov/mvhd`
pub fn mp4_duration(path: &Path) -> Result<f64> {
    let mut r = BufReader::new(File::open(path).with_context(|| format!("打不开 {}", path.display()))?);
    let len = r.seek(SeekFrom::End(0))?;
    let (moov, moov_end) = find_box(&mut r, 0, len, b"moov")?.context("不是完整的 mp4（没有 moov）")?;
    let (mvhd, _) = find_box(&mut r, moov, moov_end, b"mvhd")?.context("mp4 里没有 mvhd")?;
    r.seek(SeekFrom::Start(mvhd))?;
    let version = read_u32(&mut r)? >> 24;
    let (timescale, duration) = if version == 1 {
        r.seek(SeekFrom::Current(16))?;
        (read_u32(&mut r)?, read_u64(&mut r)?)
    } else {
        r.seek(SeekFrom::Current(8))?;
        (read_u32(&mut r)?, read_u32(&mut r)? as u64)
    };
    if timescale == 0 {
        bail!("mvhd 的 timescale 是 0");
    }
    Ok(duration as f64 / timescale as f64)
}
