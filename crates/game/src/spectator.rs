//! 观战设置：英雄追踪镜头、阵营迷雾、追踪指定玩家。
//!
//! 两种做法：
//! - 点右上角的观战菜单（`setup`），剪辑工具录制用；
//! - 控制台命令（`console_setup`），`startmovie` 录制用，不点界面、游戏不用在前台。
//!   `dota_spectator_mode` 等几个命令 `help` 和 `cvarlist` 里都查不到，但能用（2026-11-04 实测；`spec_mode` 对 Dota 无效）。
//!   迷雾没有命令，用热键（PostMessage 发，不需要前台）。
//!
//! 重新加载录像（playdemo）会把镜头重置成自由视角、把面板收起，所以每次加载后都要重新设。

use std::sync::mpsc::Receiver;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Team {
    Radiant,
    Dire,
}

impl Team {
    /// 录像里的阵营编号：2 = 天辉，3 = 夜魇
    pub fn from_id(team: i32) -> Option<Team> {
        match team {
            2 => Some(Team::Radiant),
            3 => Some(Team::Dire),
            _ => None,
        }
    }
}

/// 按追踪热键（读自用户的热键设置）追踪玩家 `player`（0–9），用 GSI 确认。按键用 PostMessage，不需要前台
/// 第一次按键偶尔不生效（2026-10-26：游戏已开着、刚重新加载录像时连续两次复现，第二次按就好了），所以最多按 3 次
pub fn focus(player: usize, gsi: &Receiver<Value>) -> Result<()> {
    let window = crate::window::dota_window()?;
    let key = crate::keys::focus_player(player)?;
    let mut seen = None;
    for _ in 0..3 {
        crate::gsi::drain(gsi);
        crate::window::post_key(window, key)?;
        // 暂停时 GSI 几秒才一个包，每次等 5 秒
        let got = crate::gsi::wait_next(gsi, Duration::from_secs(5), &format!("追踪玩家 #{player}"), |s| {
            seen = s.selected;
            s.selected == Some(player)
        });
        if got.is_ok() {
            return Ok(());
        }
        crate::cancel::check()?;
    }
    Err(anyhow::anyhow!("GSI 最后看到追踪的是 {seen:?}")).context(format!("按了 3 次追踪热键，GSI 还是没显示在追踪玩家 #{player}"))
}

/// 控制台能切的镜头
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Camera {
    /// 英雄一直在画面中间（`dota_spectator_mode 2`）
    HeroChase,
    /// 还原这名玩家当时自己的镜头，画面跟着他当时看的地方走（`dota_spectator_mode 3`）
    PlayerView,
}

/// 用控制台命令设好观战：关掉鼠标靠边接管镜头，追踪 `player`（0–9）、镜头模式、`team` 一方的迷雾，收起右上角回放控制面板，隐藏鼠标指针。
/// 改的设置都记进 `s`，结束时恢复。不点界面，游戏不用在前台
/// 录出来的画面显示玩家名：头顶血条上是玩家名，关掉匿名模式（2026-11-25 用户改定，录自己和录别人一样）。
/// 原来（2026-11-17 起）为了隐私不露名字：头顶「无」+ 匿名模式；可匿名模式不藏自己和好友，
/// 录自己时击杀提示里有自己的 ID、录别人时是英雄名，两种录法不统一。要在 `playdemo` 之前设，加载时才读。
/// 2026-11-17 实测（录像里）：
/// - 头顶名字是「血条上显示名称」`dota_hero_overhead_names`：0 英雄名称、1 玩家名称、2 无。
///   英雄名称在录像和观战里不生效、什么都不显示（Valve 已知问题 ValveSoftware/Dota2-Gameplay#1688，不修）；
/// - 匿名模式 `dota_anonymous_mode` 把顶部英雄栏、击杀提示、聊天里别人的名字换成英雄名（自己和好友不换）；
/// - `dota_hide_spectator_player_names` 设了没效果。
///
/// 这两项都是用户自己的设置（存档），`Session` 结束时恢复
pub fn show_player_names(s: &mut crate::session::Session) -> Result<()> {
    s.set("dota_hero_overhead_names", "1")?;
    s.set("dota_anonymous_mode", "0")
}

/// 录出来的画面不要右上角的 FPS / PING 和「输入/输出丢失」（2026-11-24 用户定，完美时刻的没有）。
/// 是「显示网络信息」`dota_hud_netgraph`：游戏默认关，用户自己开过的才有；设了马上生效（2026-11-24 录像里实测）。
/// 也是用户自己的设置（存档），`Session` 结束时恢复。
/// 左上角「比赛数据」那块（击/死/助、正/反、下拉框）同一天试了 `dota_spectator_stats_panel`、`statstoggle`、
/// `use_broadcaster_stats_panel`、`hudhide`、`broadcaster_mode`，都藏不掉，没有加
pub fn hide_net_info(s: &mut crate::session::Session) -> Result<()> {
    s.set("dota_hud_netgraph", "0")
}

pub fn console_setup(s: &mut crate::session::Session, player: usize, camera: Camera, team: Team) -> Result<()> {
    let mode = match camera {
        Camera::HeroChase => "2",
        Camera::PlayerView => "3",
    };
    s.con().send(&format!("dota_spectator_hero_index {player}"))?;
    // 鼠标碰到画面边缘时镜头会被临时接管、变成自由视角，录制时 Dota 在最前面，用户一动鼠标镜头就被推走
    // （2026-11-10 应用实测）。要在设镜头模式之前关：已经进了自由视角再关也切不回来（2026-10 端到端实测）
    s.set("dota_spectator_enable_edgepan_temporary_free_camera", "0")?;
    s.set("dota_spectator_mode", mode)?;
    s.con().send(&format!("dota_camera_focus_player {player}"))?;
    s.set("dota_spectator_options_enabled", "0")?;
    // 单独 dota_hide_cursor 1 没用，要先开 sv_cheats（只在这次回放里有效；2026-11-06 实测）
    s.set("sv_cheats", "1")?;
    s.set("dota_hide_cursor", "1")?;
    crate::window::post_key(crate::window::dota_window()?, crate::keys::fog(team)?)
}
