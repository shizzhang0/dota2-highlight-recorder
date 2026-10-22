//! Steam 安装目录、当前登录的账号，以及该账号下 Dota 自己的设置文件。
//!
//! "取最近写过的账号目录"和读 KeyValues 扁平项的写法移植自 dota2-game-helper2 的 `dotacfg.rs`。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

pub fn steam_dir() -> Result<PathBuf> {
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Valve\Steam").context("注册表里没找到 Steam")?;
    let p: String = key.get_value("SteamPath")?;
    Ok(PathBuf::from(p.replace('/', "\\")))
}

/// Steam 当前登录的账号（32 位账号 ID，和录像、OpenDota 里的一致）。Steam 没开或没登录时为 None
pub fn active_account() -> Option<u32> {
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Valve\Steam\ActiveProcess").ok()?;
    let id: u32 = key.get_value("ActiveUser").ok()?;
    (id != 0).then_some(id)
}

fn mtime(p: &Path) -> Option<SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// 最近玩过 Dota 的账号：`user_convars.vcfg` 最近被写过的那个（Dota 每次退出都会写它）。
/// Steam 没开时用它兜底认"自己"
pub fn recent_account() -> Option<u32> {
    let users = steam_dir().ok()?.join("userdata");
    fs::read_dir(users)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let id: u32 = e.file_name().to_str()?.parse().ok()?;
            Some((mtime(&e.path().join(r"570\remote\user_convars.vcfg"))?, id))
        })
        .max_by_key(|(t, _)| *t)
        .map(|(_, id)| id)
}

/// "自己"：优先取 Steam 当前登录的账号，没有就取最近玩过 Dota 的账号
pub fn me() -> Option<u32> {
    active_account().or_else(recent_account)
}

/// 某个账号的 `userdata\<id>\570` 目录（Dota 的设置文件都在这下面）
pub fn dota_user_dir(account: u32) -> Result<PathBuf> {
    Ok(steam_dir()?.join("userdata").join(account.to_string()).join("570"))
}

/// Valve 的 KeyValues 文本里取 `"键"  "值"` 这种一行两个引号串的扁平项，同名键取第一个
pub fn kv(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let mut it = line.split('"');
        // 一行 `\t\t"key"\t\t"value"` 按引号切开是 ["\t\t", key, "\t\t", value, ""]
        let (_, k, _, v) = (it.next(), it.next(), it.next(), it.next());
        if k == Some(key) {
            return v.map(str::to_string);
        }
    }
    None
}

/// 游戏的显示设置（`local\cfg\video.txt`，只在本机）。读不到的项为 None
#[derive(Debug, Default)]
pub struct Video {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fullscreen: Option<bool>,
    pub borderless: Option<bool>,
}

pub fn video(account: u32) -> Result<Video> {
    let path = dota_user_dir(account)?.join(r"local\cfg\video.txt");
    let text = fs::read_to_string(&path).with_context(|| format!("读不到 {}", path.display()))?;
    let num = |k| kv(&text, k).and_then(|v| v.trim().parse::<f64>().ok()).map(|f| f as u32);
    let flag = |k| kv(&text, k).map(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    Ok(Video {
        width: num("setting.defaultres"),
        height: num("setting.defaultresheight"),
        fullscreen: flag("setting.fullscreen"),
        borderless: flag("setting.nowindowborder"),
    })
}
