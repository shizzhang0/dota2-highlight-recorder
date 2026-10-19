//! 验证游戏捕获能否录到声音：建专用场景、加打开 capture_audio 的游戏捕获，
//! 每秒打印这个源的峰值音量；加 --record 则同时录一段。结束后恢复原场景并删掉专用场景。
//!
//! cargo run -p obs --example audio_meter -- --window "<标题>:<窗口类>:<exe>" [--secs 10] [--record]

use std::time::{Duration, Instant};

use obs::obws::events::Event;
use obs::obws::requests::EventSubscription;
use obs::obws::requests::inputs::{Create, InputId};
use obs::obws::requests::scenes::SceneId;
use serde_json::json;
use tokio_stream::StreamExt;

const SCENE: &str = "d2hr-test";
const INPUT: &str = "d2hr-test-capture";

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut window, mut secs, mut record) = (None, 10u64, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--window" => window = args.next(),
            "--secs" => secs = args.next().expect("--secs 后面要跟秒数").parse()?,
            "--record" => record = true,
            _ => anyhow::bail!("不认识的参数 {a}"),
        }
    }
    let window = window.ok_or_else(|| anyhow::anyhow!("要用 --window 指定窗口"))?;

    let client = obs::connect_with(Some(EventSubscription::INPUT_VOLUME_METERS)).await?;
    let original = client.scenes().current_program_scene().await?.id.name;
    if !client.scenes().list().await?.scenes.iter().any(|s| s.id.name == SCENE) {
        client.scenes().create(SCENE).await?;
    }
    let settings = json!({ "capture_mode": "window", "window": window, "priority": 2, "capture_cursor": false, "capture_audio": true });
    client
        .inputs()
        .create(Create { scene: SceneId::Name(SCENE), input: INPUT, kind: "game_capture", settings: Some(settings), enabled: Some(true) })
        .await?;
    client.scenes().set_current_program_scene(SceneId::Name(SCENE)).await?;
    tokio::time::sleep(Duration::from_secs(4)).await; // 等钩子挂上

    let result = async {
        let mut events = client.events()?;
        if record {
            client.recording().start().await?;
        }
        let started = Instant::now();
        let mut second = 0;
        let mut peak = 0f32;
        let mut seen = false;
        while started.elapsed() < Duration::from_secs(secs) {
            let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(500), events.next()).await else { continue };
            if let Event::InputVolumeMeters { inputs } = ev
                && let Some(m) = inputs.iter().find(|m| m.name == INPUT)
            {
                seen = true;
                peak = m.levels.iter().map(|l| l[1]).fold(peak, f32::max);
            }
            let now = started.elapsed().as_secs();
            if now > second {
                let db = if peak > 0.0 { format!("{:.1} dB", 20.0 * peak.log10()) } else { "静音".into() };
                println!("第 {now:>2} 秒  峰值 {peak:.4}（{db}）{}", if seen { "" } else { "  ← 音量表里没有这个源" });
                second = now;
                peak = 0.0;
                seen = false;
            }
        }
        let path = if record { Some(client.recording().stop().await?) } else { None };
        anyhow::Ok(path)
    }
    .await;

    if result.is_err() && client.recording().status().await.is_ok_and(|s| s.active) {
        let _ = client.recording().stop().await;
    }
    client.scenes().set_current_program_scene(SceneId::Name(&original)).await?;
    client.inputs().remove(InputId::Name(INPUT)).await?;
    client.scenes().remove(SceneId::Name(SCENE)).await?;
    if let Some(path) = result? {
        println!("已录制 {path}");
    }
    println!("已切回场景 {original}");
    Ok(())
}
