//! 给一个已有的视频加上片头试效果（不用重新录）：取开头一段，画片头图层，用和成品一样的 `compile` 做出来。
//!
//! cargo run -p media --example intro -- <视频.mp4> <输出.mp4> <类型> <英雄名> <战绩> [立绘.webp] [玩家名]
//! 例如 `... 9096983180_斯温_击杀集锦.mp4 out.mp4 击杀集锦 斯温 "12 / 3 / 6" ui/img/heroes/renders/sven.webp 辣酒煮雞子`。
//! 要 ffmpeg：环境变量 D2HR_FFMPEG，或在 PATH 里

use std::path::Path;

use anyhow::{Context, Result};
use media::intro::Card;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [video, out, kind, hero, kda, rest @ ..] = a.as_slice() else {
        anyhow::bail!("用法：intro <视频.mp4> <输出.mp4> <类型> <英雄名> <战绩> [立绘.webp] [玩家名]");
    };
    let render = rest.first().filter(|s| !s.is_empty()).map(std::fs::read).transpose().context("读不了立绘")?;
    let player = rest.get(1).filter(|s| !s.is_empty()).cloned();
    let card = Card { kind: kind.clone(), hero: hero.clone(), kda: kda.clone(), player, render };
    let ff = media::ffmpeg::Ffmpeg::locate(None)?;
    let video = Path::new(video);
    let (w, h) = media::mp4_video_size(video)?;
    let dir = std::env::temp_dir().join("d2hr-intro-example");
    let layer = ff.intro_layer(&card, w, h, &dir)?;
    let len = media::mp4_duration(video)?.min(15.0) as f32;
    ff.compile(ff.pick_codec(), &[(video.to_path_buf(), Some((0.0, len)))], Some((&layer, h)), Path::new(out))?;
    let _ = std::fs::remove_dir_all(&dir);
    println!("{w}×{h}，写到 {out}");
    Ok(())
}
