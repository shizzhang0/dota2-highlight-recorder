//! 高光片段规则（见 docs/phase1-plan.md 的"片段规则"）：
//!
//! 1. 每次击杀一段：从"这波战斗开打前 `pre` 秒"到"击杀后 `post` 秒"，开打时间从伤害记录里往回找；
//!    有效助攻、抢盾也各成一段（2026-11-16 加，见 [`plan`]）；
//! 2. 时间上挨着（间隔不超过 `merge_gap`）的合成一个片段；
//! 3. 片段内两段之间的空档不超过 `keep_gap` 就保留，超过才剪掉；
//! 4. 超过 `max_export` 的连续部分拆成多次导出，优先在保留下来的空档处拆（只有剪辑工具录制用；现在按 `spans` 整段录）。
//!
//! 时间都是游戏时钟（秒，开局号角为 0）。

use replay::{Kill, Replay};

/// "开打"看谁和谁之间的伤害
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// 被杀者和任意敌方英雄之间（初步统计用的口径）
    Victim,
    /// 只看击杀者和被杀者之间
    KillerVictim,
    /// 击杀者和任意敌方英雄之间（镜头跟着击杀者，推荐）
    KillerAny,
}

impl Anchor {
    pub const ALL: [Anchor; 3] = [Anchor::Victim, Anchor::KillerVictim, Anchor::KillerAny];

    pub fn label(self) -> &'static str {
        match self {
            Anchor::Victim => "(a) 被杀者↔任意敌方",
            Anchor::KillerVictim => "(b) 击杀者↔被杀者",
            Anchor::KillerAny => "(c) 击杀者↔任意敌方",
        }
    }
}

/// 片段规则的全部参数。默认值是 2026-10-22 用户根据 `examples/stats.rs` 的统计结果选定的：
/// 口径 (c)、伤害间隔 3 秒、起点上限 19 秒（该口径 90% 分位 16.6 秒 + 开打前 2 秒，取整）、空档保留 8 秒
#[derive(Debug, Clone)]
pub struct Params {
    pub anchor: Anchor,
    /// 相邻两次伤害间隔不超过这么多秒，算同一波战斗
    pub damage_gap: f32,
    /// 起点最多在击杀前多少秒（也是往回找伤害的范围）
    pub max_lead: f32,
    /// 起点最少在击杀前多少秒；找不到相关伤害时就用它
    pub min_lead: f32,
    /// 开打前再多留多少秒
    pub pre: f32,
    /// 击杀后留多少秒
    pub post: f32,
    /// 相邻击杀间隔不超过这么多秒，合成一个片段（Dota 判定多杀的窗口是 18 秒）
    pub merge_gap: f32,
    /// 片段内空档不超过这么多秒就保留
    pub keep_gap: f32,
    /// 单次导出的内容最长多少秒（剪辑工具单次超过约 60 秒会导出空文件；加上两头多留的部分不超过 58 秒）。
    /// 以下三项只有剪辑工具录制用（`exports`，技术验证的例子程序）；现在用 `startmovie` 录，按 `spans` 整段录，不用拆
    pub max_export: f32,
    /// 导出时选区开头多留多少秒，拼接时剪掉。跳转后游戏会补放天赋变化的通知，
    /// 在画面右侧挂约 2.5 秒（2026-10-23 实测，和开头留多少无关），留 4 秒就都剪掉了
    pub export_lead: f32,
    /// 导出时选区结尾多留多少秒，拼接时剪掉
    pub export_margin: f32,
    /// 录制速度：每秒墙钟能录多少秒画面。`startmovie` 逐帧渲染，1080p 团战约 0.7（2026-11-07 实测）；
    /// 桌面应用用这台机器上次实测的速度
    pub speed: f32,
    /// 每段的固定开销（跳转、补按追踪键、合进声音），秒
    pub segment_overhead: f32,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            anchor: Anchor::KillerAny,
            damage_gap: 3.0,
            max_lead: 19.0,
            min_lead: 3.0,
            pre: 2.0,
            post: 3.0,
            merge_gap: 18.0,
            keep_gap: 8.0,
            max_export: 53.0,
            export_lead: 4.0,
            export_margin: 1.0,
            speed: 0.65,
            segment_overhead: 6.0,
        }
    }
}

/// 每段开头多录、再丢掉的秒数：跳转后第一帧上有"回放已暂停"字样（2026-11-07 实测）
pub const LEAD: f32 = 0.5;

/// 一段连续的游戏时间
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub start: f32,
    pub end: f32,
}

impl Span {
    pub fn len(&self) -> f32 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// 一次导出（剪辑工具里一次选区）
#[derive(Debug, Clone, Copy)]
pub struct Export {
    pub span: Span,
    /// 是不是在连续画面中间拆出来的（拆分点会有 ±1 秒误差）
    pub mid_action: bool,
}

/// 片段的主类：有他的人头就是击杀；没人头、有他的助攻是助攻；只有抢盾是抢盾
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Kill,
    Assist,
    Aegis,
}

impl Kind {
    /// 界面、录制记录里用的名字
    pub fn key(self) -> &'static str {
        match self {
            Kind::Kill => "kill",
            Kind::Assist => "assist",
            Kind::Aegis => "aegis",
        }
    }
}

/// 一个高光片段：时间上挨着的几次击杀、助攻、抢盾
#[derive(Debug, Clone)]
pub struct Clip {
    pub player: usize,
    pub kind: Kind,
    /// 他的击杀，在 `Replay::kills` 里的下标
    pub kills: Vec<usize>,
    /// 他的有效助攻（人头是别人的），在 `Replay::kills` 里的下标
    pub assists: Vec<usize>,
    /// 剪掉长空档后保留的内容，按时间排序
    pub spans: Vec<Span>,
    pub exports: Vec<Export>,
    /// 片段内游戏判定的最大多杀数（2 = 双杀 … 5 = 暴走），没有为 0
    pub multi_kill: u32,
    /// 片段内游戏判定的最大连杀数（3 起），没有为 0
    pub kill_streak: u32,
    /// 片段里有残血反杀（见 [`low_hp_counter`]）
    pub low_hp: bool,
    /// 片段里有默契时刻（见 [`teamwork`]）
    pub teamwork: bool,
    /// 片段里他抢了盾
    pub aegis_steal: bool,
}

impl Clip {
    pub fn start(&self) -> f32 {
        self.spans[0].start
    }

    pub fn end(&self) -> f32 {
        self.spans[self.spans.len() - 1].end
    }

    /// 成片时长（剪掉的空档不算）
    pub fn duration(&self) -> f32 {
        self.spans.iter().map(Span::len).sum()
    }

    /// 录这个片段预计要多少秒：每段的固定开销 + 按录制速度录（含开头丢掉的 `LEAD`）
    pub fn estimated_seconds(&self, p: &Params) -> f32 {
        self.spans.iter().map(|s| p.segment_overhead + (s.len() + LEAD) / p.speed.max(0.05)).sum()
    }

    /// 片段里被杀的英雄（他杀的和他助攻的），按时间排序，是玩家序号
    pub fn victims(&self, r: &Replay) -> Vec<usize> {
        let mut ks: Vec<usize> = self.kills.iter().chain(&self.assists).copied().collect();
        ks.sort_by(|&a, &b| r.kills[a].game_time.total_cmp(&r.kills[b].game_time));
        ks.into_iter().map(|k| r.kills[k].victim).collect()
    }
}

/// 残血反杀：他这波最低到过的血量不超过最大血量的这个比例（2026-11-16 用户定 10%，20%、15% 都嫌松）
pub const LOW_HP_RATIO: f32 = 0.10;
/// 残血反杀看击杀前多少秒内两人的交手
const COUNTER_WINDOW: f32 = 20.0;
/// 残血反杀要求被杀者在击杀前这么多秒内打过他
const COUNTER_RECENT: f32 = 10.0;
/// 有效助攻看击杀前多少秒
pub const ASSIST_WINDOW: f32 = 10.0;
/// 有效助攻：他打出被杀者这段时间受到的英雄伤害的至少这么多（或者控过被杀者）
pub const ASSIST_SHARE: f32 = 0.10;
/// 抢盾片段从肉山被杀前多少秒开始
const AEGIS_LEAD: f32 = 8.0;
/// 抢盾前这么多秒内找不到肉山被杀，就从抢盾前 `AEGIS_FALLBACK` 秒开始
const ROSHAN_LOOKBACK: f32 = 60.0;
const AEGIS_FALLBACK: f32 = 15.0;
/// 默契时刻看被杀者死前多少秒内的控制（估的：连控一般几秒内连上，没拿数据校过）
const TEAMWORK_WINDOW: f32 = 5.0;
/// 默契时刻要求至少这么多个不同的队友控过被杀者
const TEAMWORK_DISABLERS: usize = 2;

/// 伤害记录里游戏时钟在 [from, to] 的那一截（伤害按时间排好序）
fn damage_between(r: &Replay, from: f32, to: f32) -> &[replay::Damage] {
    let lo = r.damage.partition_point(|d| d.game_time < from);
    let hi = r.damage.partition_point(|d| d.game_time <= to + 0.05);
    &r.damage[lo..hi.max(lo)]
}

/// 击杀前 `ASSIST_WINDOW` 秒里 `player` 对被杀者的贡献：打出了它受到的英雄伤害的多少（0–1）、控没控过它
fn assist_detail(r: &Replay, player: usize, k: &Kill) -> (f32, bool) {
    let (from, to) = (k.game_time - ASSIST_WINDOW, k.game_time);
    let victim_team = r.players[k.victim].team;
    let (mut mine, mut all) = (0.0f32, 0.0f32);
    for d in damage_between(r, from, to).iter().filter(|d| d.target == k.victim && r.players[d.attacker].team != victim_team) {
        all += d.value as f32;
        if d.attacker == player {
            mine += d.value as f32;
        }
    }
    let lo = r.disables.partition_point(|d| d.game_time < from);
    let disabled = r.disables[lo..].iter().take_while(|d| d.game_time <= to + 0.05).any(|d| d.attacker == player && d.target == k.victim);
    (if all > 0.0 { mine / all } else { 0.0 }, disabled)
}

/// 有效助攻：击杀前 `ASSIST_WINDOW` 秒里，`player` 打出了被杀者受到的英雄伤害的至少 `ASSIST_SHARE`，或者控过被杀者。
/// 光环、给队友加状态、早先蹭的伤害这类助攻，录出来他多半在旁边没做什么（2026-11-16 用户定）
pub fn effective_assist(r: &Replay, player: usize, k: &Kill) -> bool {
    let (share, disabled) = assist_detail(r, player, k);
    share >= ASSIST_SHARE || disabled
}

/// 残血反杀（2026-11-16 用户定）：击杀者
/// 1. 击杀时还活着（同归于尽不算）；
/// 2. 这波是对方先动手：击杀前 `COUNTER_WINDOW` 秒里两人之间的第一下伤害是被杀者打的；
/// 3. 被杀者在击杀前 `COUNTER_RECENT` 秒内打过他；
/// 4. 从两人交手起，他最低到过的血量不超过最大血量的 `LOW_HP_RATIO`（看最低点，不看击杀那一刻：吸血回上来的也算）
pub fn low_hp_counter(r: &Replay, k: &Kill) -> bool {
    let Some(killer) = k.killer else { return false };
    let (t, victim) = (k.game_time, k.victim);
    if r.kills.iter().any(|o| o.victim == killer && (o.game_time - t).abs() <= 1.0) {
        return false;
    }
    let w = damage_between(r, t - COUNTER_WINDOW, t);
    let Some(first_v) = w.iter().find(|d| d.attacker == victim && d.target == killer).map(|d| d.game_time) else {
        return false;
    };
    if w.iter().any(|d| d.attacker == killer && d.target == victim && d.game_time <= first_v) {
        return false;
    }
    if !w.iter().any(|d| d.attacker == victim && d.target == killer && d.game_time >= t - COUNTER_RECENT) {
        return false;
    }
    let Some(low) = w.iter().filter(|d| d.target == killer && d.game_time >= first_v && d.target_health > 0).min_by_key(|d| d.target_health) else {
        return false;
    };
    r.max_health_at(killer, low.tick).is_some_and(|max| low.target_health as f32 <= max as f32 * LOW_HP_RATIO)
}

/// 默契时刻（2026-11-16 用户定，是给已有片段贴的标签，和双杀一样，不另外切片段）：这次击杀
/// 1. 全队都参与了（击杀者加助攻覆盖这一方的所有人）；
/// 2. 被杀者死前 `TEAMWORK_WINDOW` 秒内被至少 `TEAMWORK_DISABLERS` 个不同的队友控过（连控配合；被动击晕不算，见 `replay::Disable`）；
/// 3. `player` 自己是控人的之一，或者人头是他的（只是在旁边蹭伤害的不标）。
///
/// 一开始只有第 1 条，几乎每波团战都算、和助攻大量重合，删过一次；收紧后 15 场里 112 段（共 1538 段）
pub fn teamwork(r: &Replay, player: usize, k: &Kill) -> bool {
    let Some(killer) = k.killer else { return false };
    let team = r.players[player].team;
    if r.players[killer].team != team {
        return false;
    }
    let mut who: Vec<usize> = std::iter::once(killer).chain(k.assists.iter().copied()).filter(|&p| r.players[p].team == team).collect();
    who.sort_unstable();
    who.dedup();
    let size = r.players.iter().filter(|p| p.team == team).count();
    if size < 5 || who.len() != size {
        return false;
    }
    let (from, to) = (k.game_time - TEAMWORK_WINDOW, k.game_time);
    let lo = r.disables.partition_point(|d| d.game_time < from);
    let mut disablers: Vec<usize> = r.disables[lo..]
        .iter()
        .take_while(|d| d.game_time <= to + 0.05)
        .filter(|d| d.target == k.victim && r.players[d.attacker].team == team)
        .map(|d| d.attacker)
        .collect();
    disablers.sort_unstable();
    disablers.dedup();
    disablers.len() >= TEAMWORK_DISABLERS && (killer == player || disablers.contains(&player))
}

/// 往回找"这波战斗"的开打时间：从击杀那一刻往前，相关伤害之间的间隔不超过 `damage_gap` 就继续往前，
/// 最多找到击杀前 `window` 秒。找不到相关伤害时为 None。以击杀者为主角
pub fn fight_start(r: &Replay, kill: &Kill, anchor: Anchor, damage_gap: f32, window: f32) -> Option<f32> {
    fight_start_of(r, kill.killer?, kill, anchor, damage_gap, window)
}

/// 同 [`fight_start`]，以 `actor` 为主角（助攻片段里是助攻的人）：口径里的"击杀者"换成他
pub fn fight_start_of(r: &Replay, actor: usize, kill: &Kill, anchor: Anchor, damage_gap: f32, window: f32) -> Option<f32> {
    let victim = kill.victim;
    let enemy = |a: usize, b: usize| r.players[a].team != r.players[b].team;
    let relevant = |d: &replay::Damage| match anchor {
        Anchor::Victim => (d.target == victim && enemy(d.attacker, victim)) || (d.attacker == victim && enemy(d.target, victim)),
        Anchor::KillerVictim => (d.attacker == actor && d.target == victim) || (d.attacker == victim && d.target == actor),
        Anchor::KillerAny => (d.attacker == actor && enemy(d.target, actor)) || (d.target == actor && enemy(d.attacker, actor)),
    };
    // 伤害按时间排好序，先二分找到击杀那一刻，再往前走
    let t = kill.game_time;
    let end = r.damage.partition_point(|d| d.game_time <= t + 0.05);
    let mut cursor = t;
    let mut found = None;
    for d in r.damage[..end].iter().rev() {
        if d.game_time < t - window || cursor - d.game_time > damage_gap {
            break;
        }
        if relevant(d) {
            cursor = d.game_time;
            found = Some(cursor);
        }
    }
    found
}

/// 片段里的一件事
#[derive(Clone, Copy)]
enum Event {
    /// 他的击杀（`Replay::kills` 下标）
    Kill(usize),
    /// 他的有效助攻
    Assist(usize),
    /// 他抢盾（`Replay::aegis_steals` 下标）
    Aegis(usize),
}

/// 某个玩家的全部高光片段，按时间排序：他的击杀、有效助攻、抢盾，
/// 时间上挨着（间隔不超过 `merge_gap`）的合成一个片段
pub fn plan(r: &Replay, player: usize, p: &Params) -> Vec<Clip> {
    let first = r.clock.first().map_or(f32::MIN, |s| s.game_time);
    let last = r.clock.last().map_or(f32::MAX, |s| s.game_time);
    let mut events: Vec<(f32, Event)> = Vec::new();
    for (i, k) in r.kills.iter().enumerate() {
        if k.killer == Some(player) {
            events.push((k.game_time, Event::Kill(i)));
        } else if k.assists.contains(&player) && effective_assist(r, player, k) {
            events.push((k.game_time, Event::Assist(i)));
        }
    }
    for (i, a) in r.aegis_steals.iter().enumerate().filter(|(_, a)| a.player == player) {
        events.push((a.game_time, Event::Aegis(i)));
    }
    events.sort_by(|a, b| a.0.total_cmp(&b.0));

    // 每件事一段：击杀、助攻从这波开打到事后 `post` 秒（镜头跟着他）；抢盾从肉山被杀前一点到抢到盾后
    let segment = |e: Event| {
        let (start, at) = match e {
            Event::Kill(i) | Event::Assist(i) => {
                let k = &r.kills[i];
                let lead = fight_start_of(r, player, k, p.anchor, p.damage_gap, p.max_lead)
                    .map_or(p.min_lead, |s| (k.game_time - s + p.pre).clamp(p.min_lead, p.max_lead));
                (k.game_time - lead, k.game_time)
            }
            Event::Aegis(i) => {
                let t = r.aegis_steals[i].game_time;
                let roshan = r.roshan_kills.iter().rev().find(|&&rt| rt <= t && t - rt <= ROSHAN_LOOKBACK);
                (roshan.map_or(t - AEGIS_FALLBACK, |rt| rt - AEGIS_LEAD), t)
            }
        };
        Span { start: start.max(first), end: (at + p.post).min(last) }
    };

    // 时间上挨着的合成一个片段
    let mut groups: Vec<Vec<(f32, Event)>> = Vec::new();
    for e in events {
        match groups.last_mut() {
            Some(g) if e.0 - g.last().unwrap().0 <= p.merge_gap => g.push(e),
            _ => groups.push(vec![e]),
        }
    }

    groups
        .into_iter()
        .map(|g| {
            let mut segs: Vec<Span> = g.iter().map(|&(_, e)| segment(e)).collect();
            segs.sort_by(|a, b| a.start.total_cmp(&b.start));
            // 空档不超过 keep_gap 的连起来；连起来的部分记下内部的空档，超长时优先在那里拆
            let mut runs: Vec<(Span, Vec<Span>)> = Vec::new();
            for s in segs {
                match runs.last_mut() {
                    Some((run, gaps)) if s.start - run.end <= p.keep_gap => {
                        if s.start > run.end {
                            gaps.push(Span { start: run.end, end: s.start });
                        }
                        run.end = run.end.max(s.end);
                    }
                    _ => runs.push((s, Vec::new())),
                }
            }
            let spans: Vec<Span> = runs.iter().map(|(s, _)| *s).collect();
            let exports = runs.iter().flat_map(|(s, gaps)| split(*s, gaps, p.max_export)).collect();
            let kills: Vec<usize> = g.iter().filter_map(|&(_, e)| if let Event::Kill(i) = e { Some(i) } else { None }).collect();
            let assists: Vec<usize> = g.iter().filter_map(|&(_, e)| if let Event::Assist(i) = e { Some(i) } else { None }).collect();
            let aegis_steal = g.iter().any(|&(_, e)| matches!(e, Event::Aegis(_)));
            let (multi_kill, kill_streak) = match (kills.first(), kills.last()) {
                (Some(&a), Some(&b)) => {
                    let (from, to) = (r.kills[a].game_time - 1.0, r.kills[b].game_time + 1.0);
                    let max_count = |events: &[replay::KillEvent]| {
                        events.iter().filter(|e| e.player == player && e.game_time >= from && e.game_time <= to).map(|e| e.count).max().unwrap_or(0)
                    };
                    (max_count(&r.multi_kills), max_count(&r.kill_streaks))
                }
                _ => (0, 0),
            };
            Clip {
                player,
                kind: if !kills.is_empty() {
                    Kind::Kill
                } else if !assists.is_empty() {
                    Kind::Assist
                } else {
                    Kind::Aegis
                },
                low_hp: kills.iter().any(|&i| low_hp_counter(r, &r.kills[i])),
                teamwork: kills.iter().chain(&assists).any(|&i| teamwork(r, player, &r.kills[i])),
                aegis_steal,
                multi_kill,
                kill_streak,
                kills,
                assists,
                spans,
                exports,
            }
        })
        .collect()
}

/// 集锦（自动合集）叫什么，按选中片段的内容定（2026-11-16 用户定）：
/// 全是抢盾叫抢盾集锦、全是残血反杀叫残血反杀集锦；否则全是击杀片段叫击杀集锦（三杀、四杀、暴走也都算在这里，
/// 一度细分成三杀集锦、暴走集锦等，用户说多杀就叫击杀集锦），全是助攻叫助攻集锦，混着的叫高光集锦
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Collection {
    Aegis,
    LowHp,
    Kills,
    Assists,
    Mixed,
}

impl Collection {
    pub fn of<'a>(clips: impl IntoIterator<Item = &'a Clip> + Clone) -> Collection {
        let all = |f: fn(&Clip) -> bool| clips.clone().into_iter().all(f);
        if all(|c| c.aegis_steal) {
            Collection::Aegis
        } else if all(|c| c.low_hp) {
            Collection::LowHp
        } else if all(|c| c.kind == Kind::Kill) {
            Collection::Kills
        } else if all(|c| c.kind == Kind::Assist) {
            Collection::Assists
        } else {
            Collection::Mixed
        }
    }

    /// 界面、录制记录里用的名字
    pub fn key(self) -> &'static str {
        match self {
            Collection::Aegis => "aegis",
            Collection::LowHp => "low_hp",
            Collection::Kills => "kills",
            Collection::Assists => "assists",
            Collection::Mixed => "mixed",
        }
    }

    /// 文件名里用的叫法（文件名本来就是中文的，比如"_三杀"）
    pub fn file_label(self) -> &'static str {
        match self {
            Collection::Aegis => "抢盾集锦",
            Collection::LowHp => "残血反杀集锦",
            Collection::Kills => "击杀集锦",
            Collection::Assists => "助攻集锦",
            Collection::Mixed => "高光集锦",
        }
    }
}

/// 集锦最长多少秒（2026-11-18 用户定 2 分钟：90 秒嫌短，完美时刻算上它自己的片头约 2 分 15 秒）
pub const COLLECTION_CAP: f32 = 120.0;
/// 助攻打分里"打了一大块血"：占被杀者死前 `ASSIST_WINDOW` 秒受到伤害的这么多
const ASSIST_BIG_SHARE: f32 = 0.30;

/// 比赛页「推荐」里的成品（2026-11-18 用户定，参考完美时刻：一局直接告诉你能生成什么，不用一个个勾片段）。
/// 击杀集锦、助攻集锦；高光时刻同一种的也合成一个（三杀、默契时刻……，用户：同一个 tag 应该放一起做成集锦）。
/// 每个都精选到 `COLLECTION_CAP` 以内。双杀不单独成品，只进击杀集锦
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductKind {
    Kills,
    Assists,
    Rampage,
    Ultra,
    Triple,
    Aegis,
    LowHp,
    Teamwork,
}

impl ProductKind {
    /// 界面、录制记录里用的名字
    pub fn key(self) -> &'static str {
        match self {
            ProductKind::Kills => "kills",
            ProductKind::Assists => "assists",
            ProductKind::Rampage => "rampage",
            ProductKind::Ultra => "ultra",
            ProductKind::Triple => "triple",
            ProductKind::Aegis => "aegis",
            ProductKind::LowHp => "low_hp",
            ProductKind::Teamwork => "teamwork",
        }
    }

    /// 文件名里用的叫法（和完美时刻一样：三杀、四杀、暴走、残血反杀、抢盾时刻、默契时刻）
    pub fn file_label(self) -> &'static str {
        match self {
            ProductKind::Kills => "击杀集锦",
            ProductKind::Assists => "助攻集锦",
            ProductKind::Rampage => "暴走",
            ProductKind::Ultra => "四杀",
            ProductKind::Triple => "三杀",
            ProductKind::Aegis => "抢盾时刻",
            ProductKind::LowHp => "残血反杀",
            ProductKind::Teamwork => "默契时刻",
        }
    }

    /// 击杀集锦、助攻集锦（按击杀、助攻次数数）；其余是某种高光时刻（按片段数数）
    pub fn is_collection(self) -> bool {
        matches!(self, ProductKind::Kills | ProductKind::Assists)
    }

    /// 高光时刻，按难得程度排（比赛页上也按这个顺序）
    pub const MOMENTS: [ProductKind; 6] =
        [ProductKind::Rampage, ProductKind::Ultra, ProductKind::Triple, ProductKind::Aegis, ProductKind::LowHp, ProductKind::Teamwork];

    /// 这个片段有没有这种高光时刻。一个片段可以有好几种（三杀又是默契），会进好几个成品，录的时候只录一次
    pub fn in_clip(self, c: &Clip) -> bool {
        match self {
            ProductKind::Kills => c.kind == Kind::Kill,
            ProductKind::Assists => c.kind == Kind::Assist,
            ProductKind::Rampage => c.multi_kill >= 5,
            ProductKind::Ultra => c.multi_kill == 4,
            ProductKind::Triple => c.multi_kill == 3,
            ProductKind::Aegis => c.aegis_steal,
            ProductKind::LowHp => c.low_hp,
            ProductKind::Teamwork => c.teamwork,
        }
    }
}

/// 一个成品：用到哪些片段，怎么拼
#[derive(Debug, Clone)]
pub struct Product {
    pub kind: ProductKind,
    /// 用到的片段在 [`plan`] 结果里的下标（从 0 开始），按时间排序
    pub clips: Vec<usize>,
    /// 选进来的、一共有多少：击杀集锦、助攻集锦按击杀（助攻）次数，高光时刻按片段数。没超过上限时两者相等
    pub picked: usize,
    pub total: usize,
}

impl Product {
    /// 界面和录制命令用的标识：一种一个，就是种类名
    pub fn id(&self) -> String {
        self.kind.key().to_owned()
    }

    /// 成片时长
    pub fn duration(&self, clips: &[Clip]) -> f32 {
        self.clips.iter().map(|&i| clips[i].duration()).sum()
    }

    /// 文件名（不含扩展名）：`比赛ID_玩家序号_英雄_击杀集锦`、`…_默契时刻`（一种一个，不带时间）
    pub fn file_name(&self, r: &Replay, clips: &[Clip]) -> String {
        let player = clips[self.clips[0]].player;
        let hero = r.players[player].hero.trim_start_matches("npc_dota_hero_");
        format!("{}_{}_{}_{}", r.match_id, player, hero, self.kind.file_label())
    }
}

/// 「自定义」里勾的片段拼成的集锦叫什么（不含扩展名）：`比赛ID_玩家序号_英雄_自定义集锦`
pub fn custom_file_name(r: &Replay, player: usize) -> String {
    let hero = r.players[player].hero.trim_start_matches("npc_dota_hero_");
    format!("{}_{}_{}_自定义集锦", r.match_id, player, hero)
}

/// 多杀的分（2026-11-18 用户定）：双杀 3、三杀 6、四杀 10、暴走 15
fn multi_weight(n: u32) -> f32 {
    match n {
        0 | 1 => 0.0,
        2 => 3.0,
        3 => 6.0,
        4 => 10.0,
        _ => 15.0,
    }
}

/// 击杀片段的分：一杀 1 分、多杀按 [`multi_weight`]（取大的）；抢盾 +5，残血反杀 +4，默契 +3；
/// 同一波里还有他的助攻（团战）每个 +0.5
pub fn kill_score(c: &Clip) -> f32 {
    let base = (c.kills.len() as f32).max(multi_weight(c.multi_kill));
    let bonus = [(c.aegis_steal, 5.0), (c.low_hp, 4.0), (c.teamwork, 3.0)].iter().filter(|(on, _)| *on).map(|(_, v)| v).sum::<f32>();
    base + bonus + 0.5 * c.assists.len() as f32
}

/// 助攻片段的分：每个有效助攻 1 分，控住过它 +1，打了它三成以上的伤害（`ASSIST_BIG_SHARE`）+1；默契 +3
pub fn assist_score(r: &Replay, c: &Clip) -> f32 {
    let each: f32 = c
        .assists
        .iter()
        .map(|&k| {
            let (share, disabled) = assist_detail(r, c.player, &r.kills[k]);
            1.0 + if disabled { 1.0 } else { 0.0 } + if share >= ASSIST_BIG_SHARE { 1.0 } else { 0.0 }
        })
        .sum();
    each + if c.teamwork { 3.0 } else { 0.0 }
}

/// 片段有没有高光时刻（三杀以上、抢盾、残血反杀、默契）
fn has_moment(c: &Clip) -> bool {
    ProductKind::MOMENTS.iter().any(|k| k.in_clip(c))
}

/// 片段的分：助攻片段按 [`assist_score`]，其余按 [`kill_score`]
fn score(r: &Replay, c: &Clip) -> f32 {
    if c.kind == Kind::Assist { assist_score(r, c) } else { kill_score(c) }
}

/// 从候选片段里挑进集锦（2026-11-18 用户定）：有高光时刻的先保证进；然后按分从高到低，放得下（总长不超过 `cap`）就放；
/// 一段都放不下时至少放分最高的那段。挑完按时间排。片段本身不剪
fn pick(clips: &[Clip], candidates: &[(usize, f32)], cap: f32) -> Vec<usize> {
    let mut order: Vec<(usize, f32)> = candidates.to_vec();
    let special = |i: usize| has_moment(&clips[i]);
    order.sort_by(|a, b| special(b.0).cmp(&special(a.0)).then(b.1.total_cmp(&a.1)).then(a.0.cmp(&b.0)));
    let (mut chosen, mut total) = (Vec::new(), 0.0f32);
    for (i, _) in order {
        let d = clips[i].duration();
        if chosen.is_empty() || total + d <= cap {
            chosen.push(i);
            total += d;
        }
    }
    chosen.sort_unstable();
    chosen
}

/// 这个玩家这局能生成的成品（`clips` 是 [`plan`] 的结果）：击杀集锦、助攻集锦，再是每种高光时刻（按 `MOMENTS` 的顺序）。
/// 每个都精选到 `COLLECTION_CAP` 以内；这局没有的不列
pub fn products(r: &Replay, clips: &[Clip]) -> Vec<Product> {
    let kinds = [ProductKind::Kills, ProductKind::Assists].into_iter().chain(ProductKind::MOMENTS);
    kinds
        .filter_map(|kind| {
            let cands: Vec<(usize, f32)> = clips.iter().enumerate().filter(|(_, c)| kind.in_clip(c)).map(|(i, c)| (i, score(r, c))).collect();
            if cands.is_empty() {
                return None;
            }
            let chosen = pick(clips, &cands, COLLECTION_CAP);
            // 击杀集锦按击杀次数数，助攻集锦按助攻次数数，高光时刻按片段数
            let count = |i: usize| match kind {
                ProductKind::Kills => clips[i].kills.len(),
                ProductKind::Assists => clips[i].assists.len(),
                _ => 1,
            };
            Some(Product {
                kind,
                picked: chosen.iter().map(|&i| count(i)).sum(),
                total: cands.iter().map(|c| count(c.0)).sum(),
                clips: chosen,
            })
        })
        .collect()
}

/// 把一段连续内容拆成不超过 `max` 秒的导出：先在内部空档的中点拆，拆完还超长的部分再均分
fn split(run: Span, gaps: &[Span], max: f32) -> Vec<Export> {
    if run.len() <= max {
        return vec![Export { span: run, mid_action: false }];
    }
    // 贪心：从头开始，尽量往后找一个空档中点作为拆分点
    let mut out = Vec::new();
    let mut start = run.start;
    let cuts: Vec<f32> = gaps.iter().map(|g| (g.start + g.end) / 2.0).collect();
    while run.end - start > max {
        match cuts.iter().rev().find(|&&c| c > start && c - start <= max) {
            Some(&c) => {
                out.push(Export { span: Span { start, end: c }, mid_action: false });
                start = c;
            }
            None => {
                // 下一个空档太远（或没有空档）：这一截连续画面只能在中间拆
                let next_cut = cuts.iter().find(|&&c| c > start).copied().unwrap_or(run.end);
                let piece = Span { start, end: next_cut };
                let n = (piece.len() / max).ceil() as usize;
                let step = piece.len() / n as f32;
                for k in 0..n {
                    let s = start + step * k as f32;
                    out.push(Export { span: Span { start: s, end: s + step }, mid_action: k > 0 });
                }
                start = next_cut;
            }
        }
    }
    if run.end > start {
        out.push(Export { span: Span { start, end: run.end }, mid_action: false });
    }
    out
}

/// 游戏时钟格式化成文件名里用的 `mm-ss`（号角前为 `-mm-ss`）
pub fn mmss(t: f32) -> String {
    let sign = if t < 0.0 { "-" } else { "" };
    let s = t.abs().round() as u32;
    format!("{sign}{:02}-{:02}", s / 60, s % 60)
}

/// 文件名（不含扩展名）：`比赛ID_玩家序号_英雄_起点_终点[_多杀|_助攻|_抢盾]`
pub fn file_name(r: &Replay, clip: &Clip) -> String {
    let hero = r.players[clip.player].hero.trim_start_matches("npc_dota_hero_");
    // 击杀片段的名字和以前一样；助攻、抢盾片段加上类别（2026-11-16）
    let tag = match (clip.kind, clip.multi_kill) {
        (Kind::Assist, _) => "_助攻",
        (Kind::Aegis, _) => "_抢盾",
        (_, 2) => "_双杀",
        (_, 3) => "_三杀",
        (_, 4) => "_四杀",
        (_, n) if n >= 5 => "_暴走",
        _ => "",
    };
    format!("{}_{}_{}_{}_{}{}", r.match_id, clip.player, hero, mmss(clip.start()), mmss(clip.end()), tag)
}
