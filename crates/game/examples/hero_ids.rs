//! 打印游戏里全部英雄的内部名（不带 `npc_dota_hero_` 前缀），一行一个，按字母排。
//! 读本机游戏的本地化文件（`game::heroes`），要装了 Dota 2。tools/hero_images.py 用它拿英雄列表。
//!
//! cargo run -q -p game --example hero_ids

fn main() -> anyhow::Result<()> {
    let mut ids: Vec<String> = game::heroes::names("english")?.into_keys().collect();
    ids.sort();
    for id in ids {
        println!("{id}");
    }
    Ok(())
}
