//! 验证 OBS 控制：建一个专用场景录一段，中间暂停一次，录完恢复原场景并删掉专用场景。
//!
//! cargo run -p obs --example record_test -- [--window "<标题>:<窗口类>:<exe>"] [--shot <截图.png>] [--keep]
//!
//! 给了 --window 就在专用场景里加一个"游戏捕获"抓那个窗口（连同游戏声音），否则只录一个纯色画面。
//! 录制节奏：录 4 秒 → 暂停 3 秒 → 继续录 4 秒 → 停止。输出视频应约 8 秒。

use std::time::Duration;

use obs::obws::requests::inputs::{Create, InputId};
use obs::obws::requests::scenes::SceneId;
use obs::obws::requests::sources::{SaveScreenshot, SourceId};
use serde_json::json;
use tokio::time::sleep;

const SCENE: &str = "d2hr-test";
const INPUT: &str = "d2hr-test-capture";

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut window, mut shot, mut keep) = (None, None, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--window" => window = args.next(),
            "--shot" => shot = args.next(),
            "--keep" => keep = true,
            _ => anyhow::bail!("不认识的参数 {a}"),
        }
    }

    let client = obs::connect().await?;
    if client.recording().status().await?.active {
        let path = client.recording().stop().await?;
        println!("发现上次没停掉的录制，已停止：{path}");
    }
    let v = client.general().version().await?;
    println!("OBS {}，obs-websocket {}，{}", v.obs_studio_version, v.obs_web_socket_version, v.platform_description);
    println!("录制目录 {}", client.config().record_directory().await?);

    let scenes = client.scenes().list().await?;
    println!("现有场景：{:?}", scenes.scenes.iter().map(|s| s.id.name.as_str()).collect::<Vec<_>>());
    let original = client.scenes().current_program_scene().await?.id.name;
    println!("当前场景：{original}");

    if !scenes.scenes.iter().any(|s| s.id.name == SCENE) {
        client.scenes().create(SCENE).await?;
    }
    let (kind, settings) = match &window {
        // capture_audio：直接从游戏捕获源录游戏声音，不录系统里的其他声音
        Some(w) => (
            "game_capture",
            json!({ "capture_mode": "window", "window": w, "priority": 2, "capture_cursor": false, "capture_audio": true }),
        ),
        None => ("color_source_v3", json!({ "color": 0xFF2060C0u32, "width": 1280, "height": 720 })),
    };
    client
        .inputs()
        .create(Create { scene: SceneId::Name(SCENE), input: INPUT, kind, settings: Some(settings), enabled: Some(true) })
        .await?;
    client.scenes().set_current_program_scene(SceneId::Name(SCENE)).await?;
    sleep(Duration::from_secs(4)).await; // 等捕获源出画面

    let result = async {
        client.recording().start().await?;
        println!("开始录制");
        if let Some(p) = &shot {
            // 让 OBS 对捕获源截一张图，确认抓到的不是黑屏
            let p = std::path::Path::new(p);
            let s = SaveScreenshot {
                source: SourceId::Name(INPUT),
                format: "png",
                width: Some(1280),
                height: None,
                compression_quality: None,
                file_path: p,
            };
            match client.sources().save_screenshot(s).await {
                Ok(()) => println!("捕获源截图 {}", p.display()),
                Err(e) => println!("捕获源截图失败（捕获源可能还没有画面）：{e}"),
            }
        }
        sleep(Duration::from_secs(4)).await;
        client.recording().pause().await?;
        println!("暂停");
        sleep(Duration::from_secs(3)).await;
        client.recording().resume().await?;
        println!("继续");
        sleep(Duration::from_secs(4)).await;
        let path = client.recording().stop().await?;
        anyhow::Ok(path)
    }
    .await;

    // 不管录制成不成功，都停掉录制、恢复原场景、删掉我们加的东西
    if result.is_err() && client.recording().status().await.is_ok_and(|s| s.active) {
        let _ = client.recording().stop().await;
    }
    client.scenes().set_current_program_scene(SceneId::Name(&original)).await?;
    if !keep {
        client.inputs().remove(InputId::Name(INPUT)).await?;
        client.scenes().remove(SceneId::Name(SCENE)).await?;
    }
    let path = result?;
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!("已保存 {path}（{size} 字节），已切回场景 {original}");
    Ok(())
}
