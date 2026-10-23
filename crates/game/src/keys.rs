//! 观战热键：从用户的 `dotakeys_personal.lst` 读"追踪第 N 号玩家"用哪个键。
//!
//! 默认是数字键 1–9、0（`SpectatorCameraFocusPlayer1..10`）；用户改过就按改过的来。
//! 键只能用 PostMessage 发单个键，带修饰键（Shift、Alt 等）的组合发不出去，碰到就报错让用户改回单键。

use std::sync::OnceLock;

use anyhow::{Result, anyhow};

/// 键名（热键文件里的写法）→ 虚拟键码
fn vk(name: &str) -> Option<u16> {
    let upper = name.to_ascii_uppercase();
    let c = upper.as_bytes();
    if c.len() == 1 && (c[0].is_ascii_uppercase() || c[0].is_ascii_digit()) {
        return Some(c[0] as u16);
    }
    if let Some(n) = upper.strip_prefix('F').and_then(|n| n.parse::<u16>().ok())
        && (1..=24).contains(&n)
    {
        return Some(0x70 + n - 1); // VK_F1..VK_F24
    }
    if let Some(n) = upper.strip_prefix("KP_").and_then(|n| n.parse::<u16>().ok())
        && n <= 9
    {
        return Some(0x60 + n); // VK_NUMPAD0..9
    }
    Some(match upper.as_str() {
        "SPACE" => 0x20,
        "TAB" => 0x09,
        "ENTER" => 0x0D,
        "UPARROW" => 0x26,
        "DOWNARROW" => 0x28,
        "LEFTARROW" => 0x25,
        "RIGHTARROW" => 0x27,
        "-" => 0xBD,
        "=" => 0xBB,
        "[" => 0xDB,
        "]" => 0xDD,
        "\\" => 0xDC,
        ";" => 0xBA,
        "'" => 0xDE,
        "," => 0xBC,
        "." => 0xBE,
        "/" => 0xBF,
        "`" => 0xC0,
        _ => return None,
    })
}

/// 默认的数字键
fn default_key(player: usize) -> u16 {
    b'0' as u16 + ((player + 1) % 10) as u16
}

fn load() -> Result<[u16; 10], String> {
    let Some(account) = crate::steam::me() else {
        return Ok(std::array::from_fn(default_key));
    };
    let path = crate::steam::dota_user_dir(account).map_err(|e| e.to_string())?.join(r"remote\cfg\dotakeys_personal.lst");
    // 没有这个文件说明用户没改过任何热键
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(std::array::from_fn(default_key));
    };
    let mut keys = [0; 10];
    for (player, key) in keys.iter_mut().enumerate() {
        let action = format!("SpectatorCameraFocusPlayer{}", player + 1);
        let name = crate::steam::kv_path(&text, &["Keys", &action, "Key"]);
        if let Some(m) = crate::steam::kv_path(&text, &["Keys", &action, "Modifier"]) {
            return Err(format!("观战热键「追踪第 {} 号玩家」带了修饰键 {m}，程序发不出组合键，请在游戏的热键设置里改成单个键", player + 1));
        }
        *key = match name {
            // 文件里没写这一项就是默认键
            None => default_key(player),
            Some(n) => vk(&n).ok_or(format!("认不出观战热键「追踪第 {} 号玩家」的键 {n}，请在游戏的热键设置里改成字母或数字键", player + 1))?,
        };
    }
    Ok(keys)
}

/// 追踪玩家 `player`（0–9）用的虚拟键码。热键文件只读一次（游戏运行期间改热键要重新运行程序）
pub fn focus_player(player: usize) -> Result<u16> {
    static KEYS: OnceLock<Result<[u16; 10], String>> = OnceLock::new();
    let keys = KEYS.get_or_init(load).as_ref().map_err(|e| anyhow!("{e}"))?;
    keys.get(player).copied().ok_or_else(|| anyhow!("玩家序号 {player} 超出 0–9"))
}
