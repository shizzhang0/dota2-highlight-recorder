//! 本地录像：列出目录里的录像文件，以及只读文件信息、不跑整场录像的快速查看。

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context as _, Result, bail};
use serde::Serialize;
use source2_demo::prelude::*;
use source2_demo::proto::EDotaUserMessages;

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
    /// 游戏模式（DOTA_GameMode）：22 = 全英雄选择，23 = 加速模式……
    pub game_mode: i32,
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
        game_mode: dota.game_mode(),
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

/// 赛后评选：MVP 和两个荣誉提名，按玩家序号 0-9，第一个是 MVP（游戏结算界面上的那个）。
/// 在比赛结束时的比赛元数据（`DOTA_UM_MatchMetadata`）里：`CDOTAMatchMetadataFile.metadata(3)` →
/// `CDOTAMatchMetadata.mvp_data(9)` → 每人一条 `mvps(1)`，里面 `player_slot(1)`（天辉 0-4，夜魇 128-132）和获奖理由。
/// 解析库没带这几个消息的定义，按字段号手读（2026-11-16 用本地 14 场核对：每场 3 人，第一个都在胜方、带 3 条理由）。
/// 只读用户消息、不建实体，一场约 50ms；这条消息在结尾前约 2 分半，直接跳到结尾会跳过去，所以从头读。
/// 录像里没有（太老或比赛没正常结束）返回空表
pub fn awards(path: impl AsRef<Path>) -> Result<Vec<usize>> {
    let path = path.as_ref();
    let file = File::open(path).with_context(|| format!("打不开录像 {}", path.display()))?;
    let mut parser = Parser::from_reader(BufReader::new(file)).with_context(|| format!("读不了录像 {}", path.display()))?;
    // 玩家序号 → 位置号：同一方按出现的先后数
    let slots: Vec<u32> = match parser.replay_info().game_info.as_ref().and_then(|g| g.dota.as_ref()) {
        Some(dota) => {
            let mut seen = [0u32; 2];
            dota.player_info
                .iter()
                .map(|p| {
                    let dire = p.game_team() == 3;
                    let k = &mut seen[dire as usize];
                    *k += 1;
                    if dire { 128 + *k - 1 } else { *k - 1 }
                })
                .collect()
        }
        None => bail!("录像里没有 Dota 2 比赛信息：{}", path.display()),
    };
    let meta = parser.register_observer::<Metadata>();
    parser.run_to_end().with_context(|| format!("读不了录像 {}", path.display()))?;
    let raw = std::mem::take(&mut meta.borrow_mut().0);
    let Some(raw) = raw else { return Ok(Vec::new()) };
    let mvps = fields(&raw, 3).into_iter().flat_map(|m| fields(m, 9)).flat_map(|d| fields(d, 1));
    Ok(mvps
        .filter_map(|d| varint_field(d, 1))
        .filter_map(|slot| slots.iter().position(|&s| u64::from(s) == slot))
        .collect())
}

#[derive(Default)]
struct Metadata(Option<Vec<u8>>);

impl Observer for Metadata {
    fn interests(&self) -> Interests {
        Interests::DOTA_USER_MESSAGE
    }

    fn on_dota_user_message(&mut self, _ctx: &Context, msg_type: EDotaUserMessages, msg: &[u8]) -> ObserverResult {
        if msg_type == EDotaUserMessages::DotaUmMatchMetadata {
            self.0 = Some(msg.to_vec());
        }
        Ok(())
    }
}

/// protobuf 里读一个 varint，返回值和下一个位置
fn varint(b: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let c = *b.get(i)?;
        i += 1;
        v |= u64::from(c & 0x7f) << shift;
        if c < 0x80 {
            return Some((v, i));
        }
    }
    None
}

/// 逐个字段：（字段号，类型，varint 值或长度型的内容）。格式不对就停
fn each(b: &[u8]) -> impl Iterator<Item = (u64, u64, u64, &[u8])> {
    let mut i = 0;
    std::iter::from_fn(move || {
        let (key, j) = varint(b, i)?;
        let (num, kind) = (key >> 3, key & 7);
        let (value, bytes, next) = match kind {
            0 => {
                let (v, j) = varint(b, j)?;
                (v, &b[0..0], j)
            }
            1 => (0, b.get(j..j + 8)?, j + 8),
            2 => {
                let (n, j) = varint(b, j)?;
                let end = j.checked_add(usize::try_from(n).ok()?)?;
                (0, b.get(j..end)?, end)
            }
            5 => (0, b.get(j..j + 4)?, j + 4),
            _ => return None,
        };
        i = next;
        Some((num, kind, value, bytes))
    })
}

/// 长度型字段 `num` 的所有内容（重复字段每条一个）
fn fields(b: &[u8], num: u64) -> Vec<&[u8]> {
    each(b).filter(|&(n, kind, _, _)| n == num && kind == 2).map(|(_, _, _, v)| v).collect()
}

/// varint 字段 `num` 的值
fn varint_field(b: &[u8], num: u64) -> Option<u64> {
    each(b).find(|&(n, kind, _, _)| n == num && kind == 0).map(|(_, _, v, _)| v)
}

