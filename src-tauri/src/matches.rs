//! 本地录像列表、按比赛 ID 下载、比赛详情、录制计划。解析结果缓存在内存里，改规则参数重算计划时不用重新解析。
//!
//! 下载时发 `download-progress` 事件（`DownloadProgress`）给界面，可以取消（`cancel_download`）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter};

use clips::{Anchor, Params};
use replay::Replay;
use serde::{Deserialize, Serialize};

/// 解析过的录像，按比赛 ID。最多留几场，免得一直涨
static PARSED: Mutex<Vec<(u64, Arc<Replay>)>> = Mutex::new(Vec::new());
const KEEP: usize = 4;

fn hero(name: &str) -> String {
    name.trim_start_matches("npc_dota_hero_").to_string()
}

fn replays_dir() -> Result<PathBuf, String> {
    game::launch::replays_dir().map_err(|e| format!("找不到 Dota 2 的录像目录：{e:#}"))
}

#[derive(Serialize)]
pub struct ReplayRow {
    match_id: u64,
    size: u64,
    /// 下载时间（Unix 秒）
    modified: u64,
    /// 比赛开始时间（Unix 秒，录像里的结束时间减去录像时长，差不多是进游戏的时间）；读不了时为 null
    start_time: Option<u64>,
    /// 录像时长（秒）；读不了时为 null
    duration: Option<f32>,
    /// 我（Steam 当前账号）在这场玩的英雄，不在这场为 null
    my_hero: Option<String>,
    my_won: Option<bool>,
    /// 我在这场的终场击杀 / 死亡 / 助攻；不在这场或读不到计分板时为 null
    my_score: Option<replay::FinalScore>,
    error: Option<String>,
}

/// 终场计分板按（比赛 ID, 录像修改时间）缓存：一场约 50ms，首页每次刷新不用重读
type Board = Vec<replay::FinalScore>;
static SCORES: Mutex<Vec<((u64, u64), Board)>> = Mutex::new(Vec::new());

fn scoreboard(path: &std::path::Path, match_id: u64, modified: u64) -> Option<Board> {
    let key = (match_id, modified);
    if let Some((_, s)) = SCORES.lock().unwrap().iter().find(|(k, _)| *k == key) {
        return Some(s.clone());
    }
    let s = replay::scoreboard(path).ok()?;
    let mut cache = SCORES.lock().unwrap();
    cache.retain(|((id, _), _)| *id != match_id);
    cache.push((key, s.clone()));
    Some(s)
}

// 要读每场的计分板，放到后台线程跑，别卡住界面
#[tauri::command(async)]
pub fn list_replays() -> Result<Vec<ReplayRow>, String> {
    let dir = replays_dir()?;
    let me = game::steam::me();
    let files = replay::list(&dir).map_err(|e| format!("{e:#}"))?;
    Ok(files
        .into_iter()
        .map(|f| {
            let modified = f.modified.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let mut row =
                ReplayRow {
                match_id: f.match_id,
                size: f.size,
                modified,
                start_time: None,
                duration: None,
                my_hero: None,
                my_won: None,
                my_score: None,
                error: None,
            };
            match replay::peek(&f.path) {
                Ok(s) => {
                    row.duration = Some(s.playback_time);
                    row.start_time = (s.end_time > 0).then(|| (s.end_time as u64).saturating_sub(s.playback_time as u64));
                    if let Some(p) = me.and_then(|id| s.players.iter().find(|p| p.account_id == id)) {
                        row.my_hero = Some(hero(&p.hero));
                        row.my_won = Some(p.team == s.winner);
                        row.my_score = scoreboard(&f.path, f.match_id, modified).and_then(|b| b.get(p.player_id).copied());
                    }
                }
                Err(e) => row.error = Some(format!("{e:#}")),
            }
            row
        })
        .collect())
}

/// 正在下载的比赛和它们的取消标记，同一场不同时下两次
static DOWNLOADING: Mutex<Vec<(u64, Arc<AtomicBool>)>> = Mutex::new(Vec::new());

/// 下载进度，发给界面（`download-progress` 事件）
#[derive(Serialize, Clone)]
#[serde(tag = "stage", rename_all = "snake_case")]
enum DownloadProgress {
    /// 向 OpenDota 要录像地址
    FindingUrl { match_id: u64 },
    /// OpenDota 还没有地址，已经请它去取
    WaitingForUrl { match_id: u64, waited_secs: u64, max_secs: u64 },
    /// 下载中（字节，压缩包）；total 为 null 表示服务器没报大小
    Downloading { match_id: u64, done: u64, total: Option<u64> },
    /// 下完了，正在保存
    Saving { match_id: u64 },
}

impl DownloadProgress {
    fn new(match_id: u64, p: replay::download::Progress) -> Self {
        use replay::download::Progress as P;
        match p {
            P::FindingUrl => Self::FindingUrl { match_id },
            P::WaitingForUrl { waited_secs, max_secs } => Self::WaitingForUrl { match_id, waited_secs, max_secs },
            P::Downloading { done, total } => Self::Downloading { match_id, done, total },
            P::Saving => Self::Saving { match_id },
        }
    }
}

/// 下载失败的原因：确定下不到的分三种，界面按种类提示；其余（网络等）带原文
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DownloadError {
    NoMatch,
    NoUrl,
    Expired,
    Busy,
    /// 用户点了取消
    Cancelled,
    Other { message: String },
}

impl From<anyhow::Error> for DownloadError {
    fn from(e: anyhow::Error) -> Self {
        use replay::download::{Cancelled, Unavailable};
        if e.downcast_ref::<Cancelled>().is_some() {
            return DownloadError::Cancelled;
        }
        match e.downcast_ref::<Unavailable>() {
            Some(Unavailable::NoMatch) => DownloadError::NoMatch,
            Some(Unavailable::NoUrl) => DownloadError::NoUrl,
            Some(Unavailable::Expired) => DownloadError::Expired,
            None => DownloadError::Other { message: format!("{e:#}") },
        }
    }
}

fn other(message: impl ToString) -> DownloadError {
    DownloadError::Other { message: message.to_string() }
}

/// 按比赛 ID 下载录像到游戏的 replays 目录（和游戏里点"下载"存的是同一个地方）。已经有了就直接返回。
/// 进度用 `download-progress` 事件发给界面
#[tauri::command]
pub async fn download_replay(app: AppHandle, match_id: u64) -> Result<(), DownloadError> {
    let dest = replays_dir().map_err(other)?.join(format!("{match_id}.dem"));
    if dest.is_file() {
        return Ok(());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut d = DOWNLOADING.lock().unwrap();
        if d.iter().any(|(id, _)| *id == match_id) {
            return Err(DownloadError::Busy);
        }
        d.push((match_id, cancel.clone()));
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut report = |p| {
            let _ = app.emit("download-progress", DownloadProgress::new(match_id, p));
        };
        replay::download::download_match(match_id, &dest, &mut report, &cancel)
    })
    .await
    .map_err(other)
    .and_then(|r| r.map_err(DownloadError::from));
    DOWNLOADING.lock().unwrap().retain(|(id, _)| *id != match_id);
    result
}

/// 取消正在下载的这一场（没在下就什么都不做）。下载线程看到标记就停下、删掉没下完的文件，`download_replay` 返回 Cancelled
#[tauri::command]
pub fn cancel_download(match_id: u64) {
    if let Some((_, c)) = DOWNLOADING.lock().unwrap().iter().find(|(id, _)| *id == match_id) {
        c.store(true, Ordering::SeqCst);
    }
}

/// 解析一场录像（有缓存）。解析器递归较深，放到大栈线程里跑
pub(crate) async fn parsed(match_id: u64) -> Result<Arc<Replay>, String> {
    if let Some((_, r)) = PARSED.lock().unwrap().iter().find(|(id, _)| *id == match_id) {
        return Ok(r.clone());
    }
    let path = replays_dir()?.join(format!("{match_id}.dem"));
    if !path.is_file() {
        return Err(format!("本地没有这场录像：{}", path.display()));
    }
    let handle = std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || replay::parse(&path))
        .map_err(|e| e.to_string())?;
    let r = tauri::async_runtime::spawn_blocking(move || handle.join())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|_| "解析录像时程序出错了".to_string())?
        .map_err(|e| format!("{e:#}"))?;
    let r = Arc::new(r);
    let mut cache = PARSED.lock().unwrap();
    cache.retain(|(id, _)| *id != match_id);
    cache.insert(0, (match_id, r.clone()));
    cache.truncate(KEEP);
    Ok(r)
}

/// 删除本地录像（一场或多场）：一起移到回收站，游戏里也就看不到了（国服客户端进某些录像会闪退，用户要能清掉）。
/// 录过的视频、录制记录不动。录制中不让删，录制要用这些文件。已经不在的跳过
#[tauri::command]
pub fn delete_replays(match_ids: Vec<u64>) -> Result<(), String> {
    if crate::record::is_recording() {
        return Err("正在录制，录完再删".into());
    }
    let dir = replays_dir()?;
    let paths: Vec<PathBuf> = match_ids.iter().map(|id| dir.join(format!("{id}.dem"))).filter(|p| p.is_file()).collect();
    crate::history::recycle(&paths)?;
    PARSED.lock().unwrap().retain(|(id, _)| !match_ids.contains(id));
    Ok(())
}

#[derive(Serialize)]
pub struct PlayerRow {
    id: usize,
    name: String,
    hero: String,
    /// 2 = 天辉，3 = 夜魇
    team: i32,
    kills: usize,
    deaths: usize,
    assists: usize,
    is_me: bool,
}

#[derive(Serialize)]
pub struct MatchInfo {
    match_id: u64,
    game_build: u32,
    /// 游戏时长（秒，号角起算）
    duration: f32,
    radiant_won: Option<bool>,
    players: Vec<PlayerRow>,
    /// 默认选中的玩家：我在这场就是我，否则 null
    me: Option<usize>,
}

#[tauri::command]
pub async fn open_match(match_id: u64) -> Result<MatchInfo, String> {
    let r = parsed(match_id).await?;
    let me = game::steam::me();
    let board = replay::scoreboard(replays_dir()?.join(format!("{match_id}.dem"))).ok();
    let players: Vec<PlayerRow> = r
        .players
        .iter()
        .map(|p| {
            // 终场计分板为准；读不到就从击杀事件里数（死亡里含被小兵、塔杀，和计分板一致）
            let (kills, deaths, assists) = match board.as_ref().and_then(|b| b.get(p.player_id)) {
                Some(s) => (s.kills as usize, s.deaths as usize, s.assists as usize),
                None => (
                    r.kills.iter().filter(|k| k.killer == Some(p.player_id)).count(),
                    r.kills.iter().filter(|k| k.victim == p.player_id).count(),
                    r.kills.iter().filter(|k| k.assists.contains(&p.player_id)).count(),
                ),
            };
            PlayerRow {
                id: p.player_id,
                name: p.name.clone(),
                hero: hero(&p.hero),
                team: p.team,
                kills,
                deaths,
                assists,
                is_me: Some(p.account_id) == me,
            }
        })
        .collect();
    let winner = replay::peek(replays_dir()?.join(format!("{match_id}.dem"))).ok().map(|s| s.winner);
    Ok(MatchInfo {
        match_id,
        game_build: r.game_build,
        duration: r.clock.last().map_or(0.0, |c| c.game_time),
        radiant_won: winner.map(|w| w == 2),
        me: players.iter().find(|p| p.is_me).map(|p| p.id),
        players,
    })
}

/// 片段规则里能在界面上调的几项，和命令行的参数一一对应
#[derive(Serialize, Deserialize, Clone)]
pub struct Rules {
    /// "a" 被杀者 ↔ 任意敌方，"b" 击杀者 ↔ 被杀者，"c" 击杀者 ↔ 任意敌方
    pub anchor: String,
    pub damage_gap: f32,
    pub max_lead: f32,
    pub post: f32,
    pub merge_gap: f32,
    pub keep_gap: f32,
}

impl Default for Rules {
    fn default() -> Self {
        let d = Params::default();
        Rules {
            anchor: match d.anchor {
                Anchor::Victim => "a",
                Anchor::KillerVictim => "b",
                Anchor::KillerAny => "c",
            }
            .into(),
            damage_gap: d.damage_gap,
            max_lead: d.max_lead,
            post: d.post,
            merge_gap: d.merge_gap,
            keep_gap: d.keep_gap,
        }
    }
}

impl Rules {
    pub fn params(&self) -> Params {
        Params {
            anchor: match self.anchor.as_str() {
                "a" => Anchor::Victim,
                "b" => Anchor::KillerVictim,
                _ => Anchor::KillerAny,
            },
            damage_gap: self.damage_gap,
            max_lead: self.max_lead,
            post: self.post,
            merge_gap: self.merge_gap,
            keep_gap: self.keep_gap,
            ..Params::default()
        }
    }
}

#[tauri::command]
pub fn default_rules() -> Rules {
    Rules::default()
}

#[derive(Serialize)]
pub struct ClipRow {
    /// 计划里的序号，从 1 开始（和命令行的 --clips 一致）
    index: usize,
    start: f32,
    end: f32,
    /// 成片时长（剪掉的空档不算）
    duration: f32,
    victims: Vec<String>,
    multi_kill: u32,
    kill_streak: u32,
    file_name: String,
    estimated_secs: f32,
    /// 输出目录里已经有这个片段的文件（再录会覆盖）
    recorded: bool,
}

#[derive(Serialize)]
pub struct PlanInfo {
    clips: Vec<ClipRow>,
}

#[tauri::command]
pub async fn plan(match_id: u64, player: usize, rules: Rules) -> Result<PlanInfo, String> {
    let r = parsed(match_id).await?;
    if player >= r.players.len() {
        return Err(format!("没有 {player} 号玩家"));
    }
    let settings = crate::settings::get_settings();
    // 规则用界面上正在调的；录制速度用这台机器实测的
    let mut p = rules.params();
    p.speed = settings.params().speed;
    let out_dir = PathBuf::from(&settings.out_dir);
    let clips = clips::plan(&r, player, &p)
        .iter()
        .enumerate()
        .map(|(i, c)| ClipRow {
            index: i + 1,
            start: c.start(),
            end: c.end(),
            duration: c.duration(),
            victims: c.kills.iter().map(|&k| hero(&r.players[r.kills[k].victim].hero)).collect(),
            multi_kill: c.multi_kill,
            kill_streak: c.kill_streak,
            file_name: clips::file_name(&r, c),
            estimated_secs: c.estimated_seconds(&p),
            recorded: out_dir.join(format!("{}.mp4", clips::file_name(&r, c))).is_file(),
        })
        .collect();
    Ok(PlanInfo { clips })
}

/// 英雄显示名（读游戏的本地化文件）。`language` 是界面语言 "zh-CN" / "en"；读不到返回空表，前端退回内部名
#[tauri::command]
pub fn hero_names(language: String) -> HashMap<String, String> {
    let game_lang = if language.starts_with("zh") { "schinese" } else { "english" };
    game::heroes::names(game_lang).unwrap_or_default()
}

/// 现在的 Unix 秒（录制记录的时间）
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// 在资源管理器里打开文件夹或选中文件
#[tauri::command]
pub fn reveal(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    let mut cmd = std::process::Command::new("explorer");
    if p.is_file() {
        cmd.arg(format!("/select,{path}"));
    } else {
        cmd.arg(&path);
    }
    cmd.spawn().map(drop).map_err(|e| e.to_string())
}
