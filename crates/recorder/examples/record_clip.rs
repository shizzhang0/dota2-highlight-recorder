//! 端到端验证：给定比赛、玩家和一段游戏时间，自动录出该玩家视角（英雄追踪）的视频。
//!
//! cargo run --release -p recorder --example record_clip -- \
//!     --match <比赛ID> --player <序号0-9|账号ID> --from <分:秒> --to <分:秒> [--extra "-perfectworld"]
//!
//! 录像取自游戏客户端的 replays 目录。游戏没开时自动以 `-netconport 2121 -insecure` 启动。
//! GSI 用我们自己的配置（端口 2122，录完删除），用来确认时钟和追踪的玩家；配置刚写入时游戏要重启才会推送。

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use game::netcon::Netcon;
use obs::obws::requests::inputs::{Create, InputId};
use obs::obws::requests::scenes::SceneId;
use obs::obws::requests::sources::{SaveScreenshot, SourceId};
use serde_json::{Value, json};

const PORT: u16 = 2121;
const SCENE: &str = "d2hr-record";
const INPUT: &str = "d2hr-dota-capture";
const WINDOW: &str = "Dota 2:SDL_app:dota2.exe";

struct Args {
    match_id: u64,
    player: String,
    from: f32,
    to: f32,
    extra: Vec<String>,
    /// 录像已经加载好时不再重新加载：重新加载会把镜头模式重置成"自由视角"
    no_reload: bool,
}

fn parse_clock(s: &str) -> Result<f32> {
    let (m, sec) = s.split_once(':').ok_or_else(|| anyhow!("时间要写成 分:秒，例如 4:05"))?;
    Ok(m.parse::<f32>()? * 60.0 + sec.parse::<f32>()?)
}

fn mmss(t: f32) -> String {
    let s = t.round() as i32;
    format!("{:02}-{:02}", s / 60, s % 60)
}

fn args() -> Result<Args> {
    let mut it = std::env::args().skip(1);
    let (mut match_id, mut player, mut from, mut to, mut extra, mut no_reload) = (None, None, None, None, Vec::new(), false);
    while let Some(a) = it.next() {
        let mut val = || it.next().ok_or_else(|| anyhow!("{a} 后面缺值"));
        match a.as_str() {
            "--match" => match_id = Some(val()?.parse()?),
            "--player" => player = Some(val()?),
            "--from" => from = Some(parse_clock(&val()?)?),
            "--to" => to = Some(parse_clock(&val()?)?),
            "--extra" => extra = val()?.split_whitespace().map(str::to_owned).collect(),
            "--no-reload" => no_reload = true,
            _ => bail!("不认识的参数 {a}"),
        }
    }
    Ok(Args {
        match_id: match_id.context("缺 --match")?,
        player: player.context("缺 --player")?,
        from: from.context("缺 --from")?,
        to: to.context("缺 --to")?,
        extra,
        no_reload,
    })
}

/// 读控制台变量当前值（输出形如 `snd_mute_losefocus = true`）
fn convar(con: &mut Netcon, name: &str) -> Result<String> {
    con.send(name)?;
    let prefix = format!("{name} = ");
    con.wait_for(Duration::from_secs(3), |l| l.starts_with(&prefix))
        .map(|l| l[prefix.len()..].trim().to_owned())
        .ok_or_else(|| anyhow!("读不到 {name}"))
}

/// 等到满足条件的 GSI 包，返回它
fn wait_gsi(gsi: &Receiver<Value>, timeout: Duration, mut pred: impl FnMut(&Value) -> bool) -> Option<Value> {
    let started = Instant::now();
    while let Some(left) = timeout.checked_sub(started.elapsed()) {
        match gsi.recv_timeout(left) {
            Ok(v) if pred(&v) => return Some(v),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

fn selected_player(v: &Value) -> Option<usize> {
    for team in ["team2", "team3"] {
        for (k, h) in v["hero"][team].as_object()? {
            if h["selected_unit"] == true {
                return k.strip_prefix("player")?.parse().ok();
            }
        }
    }
    None
}

/// OBS 停止录制后文件还要再写一会儿，等大小稳定
fn wait_file_done(path: &Path) {
    let mut last = 0;
    let mut stable = 0;
    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(500));
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if size > 0 && size == last {
            stable += 1;
            if stable >= 3 {
                return;
            }
        } else {
            stable = 0;
        }
        last = size;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let a = args()?;
    if a.to <= a.from {
        bail!("--to 要晚于 --from");
    }

    // 1. 解析录像
    let dem = game::launch::replays_dir()?.join(format!("{}.dem", a.match_id));
    let r = replay::parse(&dem).with_context(|| format!("先在游戏里下载这场录像：{}", dem.display()))?;
    let player = match a.player.parse::<usize>() {
        Ok(i) if i < r.players.len() => i,
        Ok(acc) => r.players.iter().find(|p| p.account_id as usize == acc).context("本场没有这个账号")?.player_id,
        Err(_) => bail!("--player 要写玩家序号 0-9 或账号 ID"),
    };
    let hero = r.players[player].hero.trim_start_matches("npc_dota_hero_").to_owned();
    let from_tick = r.tick_at(a.from).context("--from 超出录像范围")?;
    let to_tick = r.tick_at(a.to).context("--to 超出录像范围")?;
    println!("玩家 #{player} {}（{hero}），{} → {}，tick {from_tick} → {to_tick}", r.players[player].name, mmss(a.from), mmss(a.to));

    // 2. 游戏
    if !game::launch::netcon_ready(PORT) {
        println!("游戏没在 netcon 模式下运行，启动中…");
        game::launch::launch(PORT, &a.extra)?;
        game::launch::wait_netcon(PORT, Duration::from_secs(180))?;
    }
    game::gsi::install(game::gsi::DEFAULT_PORT)?;
    let gsi = game::gsi::listen(game::gsi::DEFAULT_PORT)?;
    let mut con = Netcon::connect(PORT)?;
    con.wait_responsive(Duration::from_secs(120))?;
    con.drain(Duration::from_millis(300), Duration::from_secs(2));

    // 3. 后台静音设置：记下原值，录完一定恢复
    let mute = convar(&mut con, "snd_mute_losefocus")?;
    con.send("snd_mute_losefocus 0")?;
    println!("snd_mute_losefocus 原值 {mute}，录制期间设为 0");

    let result = record(&a, player, &hero, from_tick, to_tick, &mut con, &gsi).await;

    let restore = if mute == "true" { "1" } else { "0" };
    con.send(&format!("snd_mute_losefocus {restore}"))?;
    println!("snd_mute_losefocus 已恢复为 {}", convar(&mut con, "snd_mute_losefocus")?);
    game::gsi::remove()?;
    let out = result?;
    println!("完成：{}", out.display());
    Ok(())
}

async fn record(
    a: &Args,
    player: usize,
    hero: &str,
    from_tick: u32,
    to_tick: u32,
    con: &mut Netcon,
    gsi: &Receiver<Value>,
) -> Result<PathBuf> {
    // 4. 加载录像。以控制台的加载完成标志为准：如果游戏里本来就在放同一场，GSI 会立刻报出这场的
    //    比赛 ID（旧状态），在重新加载的过程中发跳转命令会被吞掉
    if !a.no_reload {
        con.send(&format!("playdemo replays/{}", a.match_id))?;
        con.wait_for(Duration::from_secs(120), |l| l.contains("OnSwitchLoopModeFinished( game : success )"))
            .context("录像没有加载起来")?;
    }
    while gsi.try_recv().is_ok() {}
    let id = a.match_id.to_string();
    wait_gsi(gsi, Duration::from_secs(30), |v| v["map"]["matchid"] == id.as_str() && v["map"]["clock_time"].is_number())
        .context("加载后没收到这场比赛的 GSI")?;
    println!("录像已加载");

    // 跳到起点并暂停（不再发 spec_autodirector：它不影响 Dota 的镜头模式）
    con.send(&format!("demo_gototick {from_tick}"))?;
    let target = format!("Demo Skipping finished at tick {from_tick}");
    con.wait_for(Duration::from_secs(60), |l| l.contains(&target)).context("跳转没有完成")?;
    con.send("demo_pause")?;
    println!("已跳到 tick {from_tick}");

    // 按键追踪该玩家，用 GSI 确认（先丢掉跳转前积压的包，免得读到旧状态）
    while gsi.try_recv().is_ok() {}
    let window = game::window::dota_window()?;
    game::window::post_key(window, game::window::focus_player_key(player))?;
    let v = wait_gsi(gsi, Duration::from_secs(5), |v| selected_player(v) == Some(player))
        .with_context(|| format!("按键后 GSI 显示追踪的不是玩家 #{player}"))?;
    println!("已追踪玩家 #{player}，GSI 时钟 {}", v["map"]["clock_time"]);

    // 5. OBS
    let client = obs::connect().await?;
    if client.recording().status().await?.active {
        bail!("OBS 正在录制，先停掉再试");
    }
    let original = client.scenes().current_program_scene().await?.id.name;
    if !client.scenes().list().await?.scenes.iter().any(|s| s.id.name == SCENE) {
        client.scenes().create(SCENE).await?;
    }
    let settings = json!({ "capture_mode": "window", "window": WINDOW, "priority": 2, "capture_cursor": false, "capture_audio": true });
    client
        .inputs()
        .create(Create { scene: SceneId::Name(SCENE), input: INPUT, kind: "game_capture", settings: Some(settings), enabled: Some(true) })
        .await?;
    client.scenes().set_current_program_scene(SceneId::Name(SCENE)).await?;
    tokio::time::sleep(Duration::from_secs(4)).await; // 等钩子挂上

    // 文件名一看就懂：比赛_玩家序号_英雄_起-止
    let name = format!("{}_{}_{}_{}_{}", a.match_id, player, hero, mmss(a.from), mmss(a.to));
    let recorded = async {
        client.recording().start().await?;
        while !client.recording().status().await?.active {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // 按 1 倍速播放计时：demo_pauseatservertick 实测不会在指定位置停下，不能依赖
        con.send("demo_resume")?;
        let started = Instant::now();
        let secs = (to_tick - from_tick) as f32 / 30.0;
        println!("开始录制，预计 {secs:.1} 秒");

        // 录到一半时截一张图留证
        tokio::time::sleep(Duration::from_secs_f32(secs / 2.0)).await;
        let dir = PathBuf::from(client.config().record_directory().await?);
        let shot = dir.join(format!("{name}.png"));
        let s = SaveScreenshot { source: SourceId::Name(INPUT), format: "png", width: Some(1280), height: None, compression_quality: None, file_path: &shot };
        if let Err(e) = client.sources().save_screenshot(s).await {
            println!("截图失败：{e}");
        }

        if let Some(left) = Duration::from_secs_f32(secs).checked_sub(started.elapsed()) {
            tokio::time::sleep(left).await;
        }
        let path = PathBuf::from(client.recording().stop().await?);
        con.send("demo_pause")?;
        // 通道里积压着之前的包，丢掉后等一个新包，看实际停在几秒
        while gsi.try_recv().is_ok() {}
        let clock = wait_gsi(gsi, Duration::from_secs(3), |_| true).map(|v| v["map"]["clock_time"].clone());
        println!("停止录制，用时 {:.1} 秒，停止后 GSI 时钟 {}", started.elapsed().as_secs_f32(), clock.unwrap_or_default());
        anyhow::Ok((path, shot))
    }
    .await;

    if recorded.is_err() && client.recording().status().await.is_ok_and(|s| s.active) {
        let _ = client.recording().stop().await;
    }
    client.scenes().set_current_program_scene(SceneId::Name(&original)).await?;
    client.inputs().remove(InputId::Name(INPUT)).await?;
    client.scenes().remove(SceneId::Name(SCENE)).await?;
    println!("OBS 已切回场景 {original}，专用场景已删除");

    // 6. 等文件写完，改名
    let (path, _shot) = recorded?;
    wait_file_done(&path);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    let out = path.with_file_name(format!("{name}.{ext}"));
    std::fs::rename(&path, &out)?;
    Ok(out)
}
