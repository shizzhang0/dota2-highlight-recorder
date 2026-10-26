//! 录像列表、比赛详情、录制计划。解析结果缓存在内存里，改规则参数重算计划时不用重新解析。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

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
    /// 录像时长（秒）；读不了时为 null
    duration: Option<f32>,
    /// 我（Steam 当前账号）在这场玩的英雄，不在这场为 null
    my_hero: Option<String>,
    my_won: Option<bool>,
    error: Option<String>,
}

#[tauri::command]
pub fn list_replays() -> Result<Vec<ReplayRow>, String> {
    let dir = replays_dir()?;
    let me = game::steam::me();
    let files = replay::list(&dir).map_err(|e| format!("{e:#}"))?;
    Ok(files
        .into_iter()
        .map(|f| {
            let modified = f.modified.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let mut row =
                ReplayRow { match_id: f.match_id, size: f.size, modified, duration: None, my_hero: None, my_won: None, error: None };
            match replay::peek(&f.path) {
                Ok(s) => {
                    row.duration = Some(s.playback_time);
                    if let Some(p) = me.and_then(|id| s.players.iter().find(|p| p.account_id == id)) {
                        row.my_hero = Some(hero(&p.hero));
                        row.my_won = Some(p.team == s.winner);
                    }
                }
                Err(e) => row.error = Some(format!("{e:#}")),
            }
            row
        })
        .collect())
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

#[derive(Serialize)]
pub struct PlayerRow {
    id: usize,
    name: String,
    hero: String,
    /// 2 = 天辉，3 = 夜魇
    team: i32,
    kills: usize,
    deaths: usize,
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
    let players: Vec<PlayerRow> = r
        .players
        .iter()
        .map(|p| PlayerRow {
            id: p.player_id,
            name: p.name.clone(),
            hero: hero(&p.hero),
            team: p.team,
            kills: r.kills.iter().filter(|k| k.killer == Some(p.player_id)).count(),
            deaths: r.kills.iter().filter(|k| k.victim == p.player_id).count(),
            is_me: Some(p.account_id) == me,
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
    exports: usize,
    file_name: String,
    estimated_secs: f32,
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
    let p = rules.params();
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
            exports: c.exports.len(),
            file_name: clips::file_name(&r, c),
            estimated_secs: c.estimated_seconds(&p),
        })
        .collect();
    Ok(PlanInfo { clips })
}

/// 英雄显示名（读游戏的本地化文件）。`language` 是界面语言 "zh-CN" / "en"；读不到返回空表，前端退回内部名
#[tauri::command]
pub fn hero_names(language: String) -> std::collections::HashMap<String, String> {
    let game_lang = if language.starts_with("zh") { "schinese" } else { "english" };
    game::heroes::names(game_lang).unwrap_or_default()
}

/// 现在的 Unix 秒，前端用来算"几分钟前"
#[tauri::command]
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
