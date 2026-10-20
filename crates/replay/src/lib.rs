//! 读取 Dota 2 录像（.dem）：本场玩家、英雄，以及英雄击杀、多杀、连杀的时间点。

pub mod download;

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use serde::Serialize;
use source2_demo::prelude::*;
use source2_demo::proto::DotaCombatlogTypes;

/// Steam64 ID 与 32 位账号 ID（OpenDota、比赛记录里用的那个）之间的差值
const STEAM_ID64_BASE: u64 = 76561197960265728;

#[derive(Debug, Serialize)]
pub struct Replay {
    pub match_id: u64,
    /// 录像总 tick 数
    pub playback_ticks: i32,
    /// 开局号角那一刻的引擎时间（秒），游戏时钟 = 引擎时间 - 此值
    pub game_start_time: f32,
    pub players: Vec<Player>,
    pub kills: Vec<Kill>,
    pub multi_kills: Vec<KillEvent>,
    pub kill_streaks: Vec<KillEvent>,
    /// 游戏时钟每走一秒记一个点，用来把时钟换算成 tick（比赛暂停时时钟不走，两者不成线性）
    #[serde(skip)]
    pub clock: Vec<ClockSample>,
}

#[derive(Debug, Clone, Copy)]
pub struct ClockSample {
    pub tick: u32,
    pub game_time: f32,
}

impl Replay {
    /// 游戏时钟 t 秒（号角为 0）对应的录像 tick，可直接用于 `demo_gototick`。超出录像范围时为 None
    pub fn tick_at(&self, t: f32) -> Option<u32> {
        let i = self.clock.partition_point(|s| s.game_time < t);
        let s = self.clock.get(i)?;
        // 采样点之间按每秒 30 tick 往回推
        let back = ((s.game_time - t) * TICKS_PER_SECOND).round() as u32;
        Some(s.tick.saturating_sub(back))
    }
}

const TICKS_PER_SECOND: f32 = 30.0;

#[derive(Debug, Serialize)]
pub struct Player {
    /// 本场玩家序号 0-9，录像里所有事件都用它指代玩家
    pub player_id: usize,
    pub name: String,
    pub account_id: u32,
    /// 2 = 天辉，3 = 夜魇
    pub team: i32,
    /// 例如 npc_dota_hero_axe
    pub hero: String,
}

#[derive(Debug, Serialize)]
pub struct Kill {
    pub tick: u32,
    /// 游戏时钟（秒），开局号角为 0，号角前为负
    pub game_time: f32,
    pub victim: usize,
    /// 计分板上记到这次击杀的玩家；被反补、野怪杀、塔杀且无人记功时为 None
    pub killer: Option<usize>,
    /// 战斗日志里的致死单位，塔、小兵、召唤物击杀时与击杀者英雄不同
    pub killer_unit: String,
    pub assists: Vec<usize>,
}

/// 游戏判定的多杀（count = 2 双杀 … 5 暴走）或连杀（count = 3 起）
#[derive(Debug, Serialize)]
pub struct KillEvent {
    pub tick: u32,
    pub game_time: f32,
    pub player: usize,
    pub count: u32,
}

pub fn parse(path: impl AsRef<Path>) -> Result<Replay> {
    let path = path.as_ref();
    let file = File::open(path).with_context(|| format!("打不开录像 {}", path.display()))?;
    let mut parser = Parser::from_reader(BufReader::new(file))?;

    let info = parser.replay_info().clone();
    let playback_ticks = info.playback_ticks();
    let Some(dota) = info.game_info.and_then(|g| g.dota) else {
        bail!("录像里没有 Dota 2 比赛信息");
    };
    let players: Vec<Player> = dota
        .player_info
        .iter()
        .enumerate()
        .map(|(i, p)| Player {
            player_id: i,
            name: String::from_utf8_lossy(p.player_name()).into_owned(),
            account_id: p.steamid().saturating_sub(STEAM_ID64_BASE) as u32,
            team: p.game_team(),
            hero: p.hero_name().to_string(),
        })
        .collect();

    let collector = parser.register_observer::<Collector>();
    collector.borrow_mut().score = vec![Score::default(); players.len()];
    parser.run_to_end()?;
    let mut c = collector.take();

    let Some(game_start_time) = c.game_start_time else {
        bail!("录像里找不到开局时间，可能不完整");
    };
    let by_hero: HashMap<&str, usize> = players.iter().map(|p| (p.hero.as_str(), p.player_id)).collect();
    let player_of = |unit: &str| by_hero.get(unit).copied();
    let clock = |t: f32| t - game_start_time;

    let mut kills = Vec::new();
    let mut multi_kills = Vec::new();
    let mut kill_streaks = Vec::new();
    for e in c.entries {
        match e.kind {
            RawKind::Death { victim, attacker } => {
                let Some(victim) = player_of(&victim) else { continue };
                let team = players[victim].team;
                // 战斗日志推断不出记功：塔杀可能记给附近的敌方英雄，播报里的人也未必记功，
                // 所以以同一 tick 计分板上击杀、助攻计数的增加为准
                let mut take = |pick: fn(&mut ScoreChange) -> &mut i32| -> Vec<usize> {
                    let mut got = Vec::new();
                    for ch in c.score_changes.iter_mut() {
                        if ch.tick != e.tick || players[ch.player].team == team {
                            continue;
                        }
                        let n = pick(ch);
                        if *n > 0 {
                            *n -= 1;
                            got.push(ch.player);
                        }
                    }
                    got
                };
                let killer = take(|ch| &mut ch.kills).first().copied();
                let assists = take(|ch| &mut ch.assists);
                kills.push(Kill {
                    tick: e.tick,
                    game_time: clock(e.timestamp),
                    victim,
                    killer,
                    killer_unit: attacker,
                    assists,
                });
            }
            RawKind::MultiKill { attacker, count } => {
                let Some(player) = player_of(&attacker) else { continue };
                multi_kills.push(KillEvent { tick: e.tick, game_time: clock(e.timestamp), player, count });
            }
            RawKind::KillStreak { attacker, count } => {
                let Some(player) = player_of(&attacker) else { continue };
                kill_streaks.push(KillEvent { tick: e.tick, game_time: clock(e.timestamp), player, count });
            }
        }
    }

    Ok(Replay {
        match_id: dota.match_id(),
        playback_ticks,
        game_start_time,
        players,
        kills,
        multi_kills,
        kill_streaks,
        clock: c.clock,
    })
}

#[derive(Default)]
struct Collector {
    game_start_time: Option<f32>,
    entries: Vec<RawEntry>,
    /// 每个玩家当前计分板上的击杀、助攻数
    score: Vec<Score>,
    score_changes: Vec<ScoreChange>,
    clock: Vec<ClockSample>,
}

#[derive(Clone, Copy, Default, PartialEq)]
struct Score {
    kills: i32,
    assists: i32,
}

struct ScoreChange {
    tick: u32,
    player: usize,
    kills: i32,
    assists: i32,
}

struct RawEntry {
    tick: u32,
    timestamp: f32,
    kind: RawKind,
}

enum RawKind {
    Death { victim: String, attacker: String },
    MultiKill { attacker: String, count: u32 },
    KillStreak { attacker: String, count: u32 },
}

#[observer]
#[uses_entities]
#[uses_combat_log]
impl Collector {
    #[on_tick_start]
    fn on_tick_start(&mut self, ctx: &Context) -> ObserverResult {
        let Ok(rules) = ctx.entities().get_by_class_name("CDOTAGamerulesProxy") else {
            return Ok(());
        };
        let Some(start) = self.game_start_time else {
            let start: f32 = property!(rules, "m_pGameRules.m_flGameStartTime");
            if start > 0.0 {
                self.game_start_time = Some(start);
            }
            return Ok(());
        };
        // 新录像没有 m_fGameTime，就用"网络 tick 减去累计暂停的 tick"推出引擎时间（与 opendota 的做法相同）
        let now = match try_property!(rules, f32, "m_pGameRules.m_fGameTime") {
            Some(t) => t,
            None => {
                let paused: bool = property!(rules, "m_pGameRules.m_bGamePaused");
                let tick: i32 = if paused {
                    property!(rules, "m_pGameRules.m_nPauseStartTick")
                } else {
                    ctx.net_tick() as i32
                };
                let paused_ticks: i32 = property!(rules, "m_pGameRules.m_nTotalPausedTicks");
                (tick - paused_ticks) as f32 / TICKS_PER_SECOND
            }
        };
        let game_time = now - start;
        if self.clock.last().is_none_or(|s| game_time.floor() > s.game_time.floor()) {
            self.clock.push(ClockSample { tick: ctx.tick(), game_time });
        }
        Ok(())
    }

    #[on_tick_end]
    fn on_tick_end(&mut self, ctx: &Context) -> ObserverResult {
        let Ok(pr) = ctx.entities().get_by_class_name("CDOTA_PlayerResource") else {
            return Ok(());
        };
        for (i, old) in self.score.iter_mut().enumerate() {
            let now = Score {
                kills: try_property!(pr, "m_vecPlayerTeamData.{i:04}.m_iKills").unwrap_or(old.kills),
                assists: try_property!(pr, "m_vecPlayerTeamData.{i:04}.m_iAssists").unwrap_or(old.assists),
            };
            if now != *old {
                self.score_changes.push(ScoreChange {
                    tick: ctx.tick(),
                    player: i,
                    kills: now.kills - old.kills,
                    assists: now.assists - old.assists,
                });
                *old = now;
            }
        }
        Ok(())
    }

    #[on_combat_log]
    fn on_combat_log(&mut self, ctx: &Context, cle: &CombatLogEntry) -> ObserverResult {
        let name = |r: Result<&str, _>| r.unwrap_or_default().to_string();
        let kind = match cle.r#type() {
            DotaCombatlogTypes::DotaCombatlogDeath => {
                // 只要真身英雄的死亡；幻象、复活中的（如骷髅王带盾）不算
                if !cle.is_target_hero().unwrap_or(false)
                    || cle.is_target_illusion().unwrap_or(false)
                    || cle.will_reincarnate().unwrap_or(false)
                {
                    return Ok(());
                }
                RawKind::Death { victim: name(cle.target_name()), attacker: name(cle.attacker_name()) }
            }
            DotaCombatlogTypes::DotaCombatlogMultikill => {
                RawKind::MultiKill { attacker: name(cle.attacker_name()), count: cle.value().unwrap_or_default() }
            }
            DotaCombatlogTypes::DotaCombatlogKillstreak => {
                RawKind::KillStreak { attacker: name(cle.attacker_name()), count: cle.value().unwrap_or_default() }
            }
            _ => return Ok(()),
        };
        self.entries.push(RawEntry { tick: ctx.tick(), timestamp: cle.timestamp().unwrap_or_default(), kind });
        Ok(())
    }
}
