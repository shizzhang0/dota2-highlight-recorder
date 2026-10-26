//! 英雄的显示名，读游戏自带的本地化文件，跟着游戏更新，新英雄也有。
//!
//! 在 `pak01_dir.vpk` 的 `resource/localization/abilities_<语言>.txt` 里，键是 `npc_dota_hero_<名字>:n`
//! （2026-10-26 查的；同一文件里还有 `__en:n` 结尾的英文名和 `persona` 变体，不要）。

use std::collections::HashMap;

use anyhow::Result;

/// 英雄内部名（不带 `npc_dota_hero_` 前缀，例如 `phantom_assassin`）→ 显示名。
/// `language` 是游戏的语言名：`schinese`、`english` 等
pub fn names(language: &str) -> Result<HashMap<String, String>> {
    let dir = crate::launch::game_dir()?.join("dota");
    let raw = crate::vpk::read(&dir, &format!("resource/localization/abilities_{language}.txt"))?;
    let text = String::from_utf8_lossy(&raw);
    let mut out = HashMap::new();
    for line in text.lines() {
        // 一行 `\t\t"npc_dota_hero_axe:n"\t\t"斧王"` 按引号切开是 ["\t\t", 键, "\t\t", 值, ""]
        let mut it = line.split('"');
        let (Some(key), Some(value)) = (it.nth(1), it.nth(1)) else { continue };
        let Some(hero) = key.strip_prefix("npc_dota_hero_").and_then(|k| k.strip_suffix(":n")) else { continue };
        if hero.contains("__") || hero.contains("persona") || value.is_empty() {
            continue;
        }
        out.insert(hero.to_owned(), value.to_owned());
    }
    Ok(out)
}
