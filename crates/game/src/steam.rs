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

/// Valve 的 KeyValues 文本里按路径取值，例如 `["apps", "570", "LaunchOptions"]`：
/// 路径是层层嵌套的块名，只要求按顺序出现在当前位置的上层（中间可以隔着别的层）
pub(crate) fn kv_path(text: &str, path: &[&str]) -> Option<String> {
    let (want, parents) = path.split_last()?;
    // 当前所在的块名，从外到里；`last` 是刚读到、还没确定是不是块名的键
    let mut stack: Vec<String> = Vec::new();
    let mut last: Option<String> = None;
    for line in text.lines() {
        let t = line.trim();
        if t == "{" {
            stack.push(last.take().unwrap_or_default());
            continue;
        }
        if t == "}" {
            stack.pop();
            continue;
        }
        let mut it = t.split('"');
        let (_, k, _, v) = (it.next(), it.next(), it.next(), it.next());
        let Some(k) = k else { continue };
        match v {
            Some(v) if k == *want && stack.len() >= parents.len() => {
                // 上层块名的末尾必须正好是 parents
                if stack[stack.len() - parents.len()..].iter().map(String::as_str).eq(parents.iter().copied()) {
                    return Some(v.to_string());
                }
            }
            Some(_) => {}
            None => last = Some(k.to_string()),
        }
    }
    None
}

/// 用户在 Steam 里给 Dota 设的启动选项（`userdata\<id>\config\localconfig.vdf`），没设返回空
pub fn launch_options(account: u32) -> Result<Vec<String>> {
    let path = steam_dir()?.join("userdata").join(account.to_string()).join(r"config\localconfig.vdf");
    let text = fs::read_to_string(&path).with_context(|| format!("读不到 {}", path.display()))?;
    Ok(kv_path(&text, &["apps", "570", "LaunchOptions"]).map(|s| s.split_whitespace().map(str::to_owned).collect()).unwrap_or_default())
}

/// 游戏的显示设置（`local\cfg\video.txt`，只在本机）。读不到的项为 None
#[derive(Debug, Default)]
pub struct Video {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fullscreen: Option<bool>,
    pub borderless: Option<bool>,
}

/// 把 `video.txt` 里的窗口大小改回 `w`×`h`，其他设置不动。
/// 带 `-w -h` 启动的游戏退出时会把这个大小写进去（2026-10-22 实测），录完要改回来；要在游戏退出之后调用
pub fn set_video_size(account: u32, w: u32, h: u32) -> Result<()> {
    let path = dota_user_dir(account)?.join(r"local\cfg\video.txt");
    let text = fs::read_to_string(&path).with_context(|| format!("读不到 {}", path.display()))?;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let key = line.split('"').nth(1);
        let value = match key {
            Some("setting.defaultres") => Some(w),
            Some("setting.defaultresheight") => Some(h),
            _ => None,
        };
        match value {
            // 保留原来的缩进、分隔符和换行，只换值
            Some(v) => {
                let (head, _) = line.rsplit_once('"').and_then(|(a, _)| a.rsplit_once('"')).unwrap_or((line, ""));
                let tail = &line[line.trim_end().len()..];
                out.push_str(&format!("{head}\"{v}\"{tail}"));
            }
            None => out.push_str(line),
        }
    }
    fs::write(&path, out).with_context(|| format!("写不了 {}", path.display()))
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
