//! 读游戏的 VPK 包（`game\dota\pak01_dir.vpk` + `pak01_NNN.vpk`），只读，不改任何文件。
//!
//! VPK v2 格式：28 字节文件头，然后是目录树——按"扩展名 → 路径 → 文件名"三层，每层以空字符串结束；
//! 每个文件一条 18 字节的记录（CRC、预载字节数、所在分卷、偏移、长度、结束标记）后跟预载数据。
//! 分卷号 0x7FFF 表示数据就在目录文件里（目录树之后）。

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const SIGNATURE: u32 = 0x55AA_1234;
const IN_DIR_FILE: u16 = 0x7FFF;

fn read_u16(r: &mut impl Read) -> Result<u16> {
    let mut b = [0; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}

fn read_u32(r: &mut impl Read) -> Result<u32> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_cstr(r: &mut impl BufRead) -> Result<String> {
    let mut buf = Vec::new();
    r.read_until(0, &mut buf)?;
    if buf.pop() != Some(0) {
        bail!("VPK 目录意外结束");
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// 从 `dir`（如 `...\game\dota`）的 pak01 包里读出 `path`（如 `resource/localization/abilities_schinese.txt`）
pub fn read(dir: &Path, path: &str) -> Result<Vec<u8>> {
    let (folder, file) = path.rsplit_once('/').unwrap_or(("", path));
    let (name, ext) = file.rsplit_once('.').unwrap_or((file, ""));
    let dir_path = dir.join("pak01_dir.vpk");
    let mut r = BufReader::new(File::open(&dir_path).with_context(|| format!("打不开 {}", dir_path.display()))?);
    if read_u32(&mut r)? != SIGNATURE {
        bail!("{} 不是 VPK 文件", dir_path.display());
    }
    let version = read_u32(&mut r)?;
    let tree_size = read_u32(&mut r)?;
    let header = match version {
        1 => 12,
        2 => 28,
        v => bail!("不认识的 VPK 版本 {v}"),
    };
    r.seek(SeekFrom::Start(header))?;
    loop {
        let e = read_cstr(&mut r)?;
        if e.is_empty() {
            break;
        }
        loop {
            let p = read_cstr(&mut r)?;
            if p.is_empty() {
                break;
            }
            loop {
                let n = read_cstr(&mut r)?;
                if n.is_empty() {
                    break;
                }
                let _crc = read_u32(&mut r)?;
                let preload = read_u16(&mut r)?;
                let archive = read_u16(&mut r)?;
                let offset = read_u32(&mut r)?;
                let length = read_u32(&mut r)?;
                let _end = read_u16(&mut r)?;
                if e == ext && p == folder && n == name {
                    let mut data = vec![0; preload as usize];
                    r.read_exact(&mut data)?;
                    let (file, start): (PathBuf, u64) = if archive == IN_DIR_FILE {
                        (dir_path.clone(), header + tree_size as u64 + offset as u64)
                    } else {
                        (dir.join(format!("pak01_{archive:03}.vpk")), offset as u64)
                    };
                    let mut f = File::open(&file).with_context(|| format!("打不开 {}", file.display()))?;
                    f.seek(SeekFrom::Start(start))?;
                    let at = data.len();
                    data.resize(at + length as usize, 0);
                    f.read_exact(&mut data[at..])?;
                    return Ok(data);
                }
                r.seek(SeekFrom::Current(preload as i64))?;
            }
        }
    }
    bail!("VPK 里没有 {path}")
}
