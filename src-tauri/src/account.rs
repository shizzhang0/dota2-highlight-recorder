//! 认"我"是哪个 Steam 账号：本地录像列表里"我的英雄 / 胜负 / K/D/A"、比赛页的「（你）」、录制时记的"是不是自己"都按它算。
//!
//! 默认跟着 Steam 当前登录的账号；它在这台电脑上没玩过 Dota（或者 Steam 没开）就用最近玩过 Dota 的那个。
//! 本机玩过 Dota 的账号有多个时，用户可以在本地录像页切换（2026-11-15 用户定）：只在这次打开有效，不存设置，
//! 下次启动又跟着 Steam 走——有小号的人在 Steam 换号登录后打开，默认看到的就是那个号。
//!
//! 启动游戏用的按键、启动选项、分辨率还是跟着真正登录的账号（`game::steam::me`），不归这里管。

use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use serde::Serialize;

/// 用户在本地录像页选的账号；None = 跟着默认
static CHOSEN: Mutex<Option<u32>> = Mutex::new(None);

/// 现在算"我"的账号
pub fn me() -> Option<u32> {
    CHOSEN.lock().unwrap().or_else(default_me)
}

fn default_me() -> Option<u32> {
    let list = game::steam::dota_accounts();
    match game::steam::active_account() {
        Some(a) if list.iter().any(|(id, _)| *id == a) => Some(a),
        active => game::steam::recent_account().or(active),
    }
}

#[derive(Serialize)]
pub struct Account {
    id: u32,
    /// Steam 昵称，读不到为 null（界面显示账号 ID）
    name: Option<String>,
    /// 头像（data: URL，Steam 本机缓存的），没有为 null
    avatar: Option<String>,
    /// 最近一次退出 Dota（Unix 秒）
    last_played: Option<u64>,
    /// 是不是 Steam 现在登录的账号
    steam_active: bool,
}

#[derive(Serialize)]
pub struct Accounts {
    /// 本机玩过 Dota 的账号：Steam 当前登录的在前，其余按最近玩过排
    accounts: Vec<Account>,
    current: Option<u32>,
}

#[tauri::command(async)]
pub fn accounts() -> Accounts {
    let active = game::steam::active_account();
    let mut accounts: Vec<Account> = game::steam::dota_accounts()
        .into_iter()
        .map(|(id, played)| Account {
            id,
            name: game::steam::persona_name(id),
            avatar: game::steam::avatar_path(id).and_then(|p| std::fs::read(p).ok()).map(|b| format!("data:image/png;base64,{}", base64(&b))),
            last_played: played.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()),
            steam_active: active == Some(id),
        })
        .collect();
    accounts.sort_by_key(|a| (!a.steam_active, std::cmp::Reverse(a.last_played)));
    Accounts { accounts, current: me() }
}

/// 切换"我"。选的正好是默认那个就回到跟着默认
#[tauri::command]
pub fn set_account(id: u32) -> Result<(), String> {
    if !game::steam::dota_accounts().iter().any(|(a, _)| *a == id) {
        return Err(format!("这台电脑上没有账号 {id} 玩过 Dota"));
    }
    *CHOSEN.lock().unwrap() = (default_me() != Some(id)).then_some(id);
    Ok(())
}

/// 头像只有几 KB，自己编一下，不为它加依赖
fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}
