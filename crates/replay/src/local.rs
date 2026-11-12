//! 本地录像：列出目录里的录像文件，以及只读文件信息、不跑整场录像的快速查看。

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context as _, Result, bail};
use serde::Serialize;
use source2_demo::prelude::*;

use crate::{Player, players_of};

/// 目录里的一个录像文件
#[derive(Debug, Serialize)]
pub struct LocalReplay {
    pub path: PathBuf,
    /// 游戏客户端下载的录像按 `<比赛ID>.dem` 命名
    pub match_id: u64,
    pub size: u64,
    pub modified: SystemTime,
}

/// 录像文件信息里就有的内容，不用跑整场录像
#[derive(Debug, Serialize)]
pub struct Summary {
    pub match_id: u64,
    pub players: Vec<Player>,
    /// 录像时长（秒）
    pub playback_time: f32,
    /// 比赛结束时间（Unix 时间戳）
    pub end_time: u32,
    /// 获胜方：2 = 天辉，3 = 夜魇
    pub winner: i32,
}

/// 列出目录里的 `<比赛ID>.dem`，最新修改的在前。目录不存在时返回空列表
pub fn list(dir: impl AsRef<Path>) -> Result<Vec<LocalReplay>> {
    let dir = dir.as_ref();
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("dem")) {
            continue;
        }
        let Some(match_id) = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse().ok()) else {
            continue;
        };
        let meta = entry.metadata()?;
        out.push(LocalReplay { path, match_id, size: meta.len(), modified: meta.modified()? });
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.modified));
    Ok(out)
}

/// 只读录像的文件信息：比赛 ID、10 名玩家和英雄、时长
pub fn peek(path: impl AsRef<Path>) -> Result<Summary> {
    let path = path.as_ref();
    let file = File::open(path).with_context(|| format!("打不开录像 {}", path.display()))?;
    let parser = Parser::from_reader(BufReader::new(file)).with_context(|| format!("读不了录像 {}", path.display()))?;
    let info = parser.replay_info();
    let Some(dota) = info.game_info.as_ref().and_then(|g| g.dota.as_ref()) else {
        bail!("录像里没有 Dota 2 比赛信息：{}", path.display());
    };
    Ok(Summary {
        match_id: dota.match_id(),
        players: players_of(dota),
        playback_time: info.playback_time(),
        end_time: dota.end_time(),
        winner: dota.game_winner(),
    })
}

#[derive(Default)]
struct NeedEntities;

impl Observer for NeedEntities {
    fn interests(&self) -> Interests {
        Interests::ENTITY_STATE
    }
}

/// 一名玩家终场的计分板：击杀 / 死亡 / 助攻
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Score {
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
}

/// 终场计分板，按玩家序号 0-9 排。直接跳到录像最后读计分板，不逐 tick 跑整场，
/// 比 [`crate::parse`] 快得多（首页列表要给每场显示 K/D/A）
pub fn scoreboard(path: impl AsRef<Path>) -> Result<Vec<Score>> {
    let path = path.as_ref();
    let file = File::open(path).with_context(|| format!("打不开录像 {}", path.display()))?;
    let mut parser = Parser::from_reader(BufReader::new(file)).with_context(|| format!("读不了录像 {}", path.display()))?;
    let players = parser
        .replay_info()
        .game_info
        .as_ref()
        .and_then(|g| g.dota.as_ref())
        .map_or(0, |d| d.player_info.len());
    // 没有观察者要实体的话，解析器根本不建实体
    parser.register_observer::<NeedEntities>();
    parser.jump_to_tick(u32::MAX).with_context(|| format!("读不了录像 {}", path.display()))?;
    let entities = parser.context().entities();
    let pr = entities
        .get_by_class_name("CDOTA_PlayerResource")
        .map_err(|_| anyhow::anyhow!("录像里没有计分板：{}", path.display()))?;
    let get = |i: usize, field: &str| -> u32 {
        try_property!(pr, i32, "m_vecPlayerTeamData.{i:04}.{field}").unwrap_or(0).max(0) as u32
    };
    Ok((0..players)
        .map(|i| Score { kills: get(i, "m_iKills"), deaths: get(i, "m_iDeaths"), assists: get(i, "m_iAssists") })
        .collect())
}
