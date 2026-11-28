//! 跳到录像里的某一刻。
//!
//! 原来和「用游戏自带的视频剪辑工具导出」放在一起（clip_builder，验证 6），2026-11-28 剪辑工具那条路删掉了，
//! 录制早就改用 `startmovie`（见 `movie`），跳转留下来单独成一个模块。

use std::sync::mpsc::Receiver;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use crate::netcon::Netcon;

/// 跳到 `tick`（游戏时钟 `clock` 秒）并暂停，用 GSI 确认真的到了。
/// 要在播放状态下跳：暂停时跳转，控制台照样报完成，但画面不更新（2026-10-22 实测）
pub fn goto(con: &mut Netcon, gsi: &Receiver<Value>, tick: u32, clock: f32) -> Result<()> {
    con.send("demo_resume")?;
    con.seek(tick, Duration::from_secs(60))?;
    crate::gsi::drain(gsi);
    con.send("demo_pause")?;
    let what = format!("GSI 时钟到 {clock:.0} 秒");
    crate::gsi::wait_next(gsi, Duration::from_secs(15), &what, |s| s.clock.is_some_and(|c| (c as f32 - clock).abs() <= 2.0))
        .map(drop)
}
