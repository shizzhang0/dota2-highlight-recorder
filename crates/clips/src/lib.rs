//! 高光片段规则（见 docs/phase1-plan.md 的"片段规则"）：
//!
//! 1. 每次击杀一段：从"这波战斗开打前 `pre` 秒"到"击杀后 `post` 秒"，开打时间从伤害记录里往回找；
//! 2. 相邻击杀间隔不超过 `merge_gap` 的合成一个片段；
//! 3. 片段内两段之间的空档不超过 `keep_gap` 就保留，超过才剪掉；
//! 4. 超过 `max_export` 的连续部分拆成多次导出，优先在保留下来的空档处拆。
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
    /// 单次导出的内容最长多少秒（剪辑工具单次超过约 60 秒会导出空文件；加上两头多留的部分不超过 58 秒）
    pub max_export: f32,
    /// 导出时选区开头多留多少秒，拼接时剪掉。跳转后游戏会补放天赋变化的通知，
    /// 在画面右侧挂约 2.5 秒（2026-10-23 实测，和开头留多少无关），留 4 秒就都剪掉了
    pub export_lead: f32,
    /// 导出时选区结尾多留多少秒，拼接时剪掉
    pub export_margin: f32,
    /// 剪辑工具渲染耗时 ≈ 视频时长 × 这个倍数
    pub render_factor: f32,
    /// 每次导出的固定开销（切前台、点界面、确认对话框、等文件），秒
    pub export_overhead: f32,
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
            render_factor: 1.8,
            export_overhead: 17.5,
        }
    }
}

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

/// 一个高光片段：一个或几个相邻的击杀
#[derive(Debug, Clone)]
pub struct Clip {
    pub player: usize,
    /// 在 `Replay::kills` 里的下标
    pub kills: Vec<usize>,
    /// 剪掉长空档后保留的内容，按时间排序
    pub spans: Vec<Span>,
    pub exports: Vec<Export>,
    /// 片段内游戏判定的最大多杀数（2 = 双杀 … 5 = 暴走），没有为 0
    pub multi_kill: u32,
    /// 片段内游戏判定的最大连杀数（3 起），没有为 0
    pub kill_streak: u32,
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

    /// 录这个片段预计要多少秒：每次导出的固定开销 + 渲染（含两头余量）
    pub fn estimated_seconds(&self, p: &Params) -> f32 {
        self.exports.iter().map(|e| p.export_overhead + (e.span.len() + p.export_lead + p.export_margin) * p.render_factor).sum()
    }
}

/// 往回找"这波战斗"的开打时间：从击杀那一刻往前，相关伤害之间的间隔不超过 `damage_gap` 就继续往前，
/// 最多找到击杀前 `window` 秒。找不到相关伤害时为 None
pub fn fight_start(r: &Replay, kill: &Kill, anchor: Anchor, damage_gap: f32, window: f32) -> Option<f32> {
    let killer = kill.killer?;
    let victim = kill.victim;
    let enemy = |a: usize, b: usize| r.players[a].team != r.players[b].team;
    let relevant = |d: &replay::Damage| match anchor {
        Anchor::Victim => (d.target == victim && enemy(d.attacker, victim)) || (d.attacker == victim && enemy(d.target, victim)),
        Anchor::KillerVictim => (d.attacker == killer && d.target == victim) || (d.attacker == victim && d.target == killer),
        Anchor::KillerAny => (d.attacker == killer && enemy(d.target, killer)) || (d.target == killer && enemy(d.attacker, killer)),
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

/// 某个玩家的全部高光片段，按时间排序
pub fn plan(r: &Replay, player: usize, p: &Params) -> Vec<Clip> {
    let first = r.clock.first().map_or(f32::MIN, |s| s.game_time);
    let last = r.clock.last().map_or(f32::MAX, |s| s.game_time);
    let mut kills: Vec<usize> = (0..r.kills.len()).filter(|&i| r.kills[i].killer == Some(player)).collect();
    kills.sort_by(|&a, &b| r.kills[a].game_time.total_cmp(&r.kills[b].game_time));

    // 每次击杀一段
    let segment = |i: usize| {
        let k = &r.kills[i];
        let lead = fight_start(r, k, p.anchor, p.damage_gap, p.max_lead)
            .map_or(p.min_lead, |s| (k.game_time - s + p.pre).clamp(p.min_lead, p.max_lead));
        Span { start: (k.game_time - lead).max(first), end: (k.game_time + p.post).min(last) }
    };

    // 相邻击杀间隔不超过 merge_gap 的合成一个片段
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for i in kills {
        match groups.last_mut() {
            Some(g) if r.kills[i].game_time - r.kills[*g.last().unwrap()].game_time <= p.merge_gap => g.push(i),
            _ => groups.push(vec![i]),
        }
    }

    groups
        .into_iter()
        .map(|g| {
            let mut segs: Vec<Span> = g.iter().map(|&i| segment(i)).collect();
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
            let (from, to) = (r.kills[g[0]].game_time - 1.0, r.kills[*g.last().unwrap()].game_time + 1.0);
            let max_count = |events: &[replay::KillEvent]| {
                events.iter().filter(|e| e.player == player && e.game_time >= from && e.game_time <= to).map(|e| e.count).max().unwrap_or(0)
            };
            Clip {
                player,
                multi_kill: max_count(&r.multi_kills),
                kill_streak: max_count(&r.kill_streaks),
                kills: g,
                spans,
                exports,
            }
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

/// 文件名（不含扩展名）：`比赛ID_玩家序号_英雄_起点_终点[_多杀]`
pub fn file_name(r: &Replay, clip: &Clip) -> String {
    let hero = r.players[clip.player].hero.trim_start_matches("npc_dota_hero_");
    let tag = match clip.multi_kill {
        2 => "_双杀",
        3 => "_三杀",
        4 => "_四杀",
        n if n >= 5 => "_暴走",
        _ => "",
    };
    format!("{}_{}_{}_{}_{}{}", r.match_id, clip.player, hero, mmss(clip.start()), mmss(clip.end()), tag)
}
