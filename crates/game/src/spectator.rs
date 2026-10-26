//! 观战设置：英雄追踪镜头、阵营迷雾、追踪指定玩家。
//!
//! 镜头模式和迷雾没有控制台命令（`spec_mode` 等对 Dota 的镜头无效），只能点右上角的观战菜单。
//! 重新加载录像（playdemo）会把镜头重置成自由视角、把面板收起，所以每次加载后都要重新设。

use std::sync::mpsc::Receiver;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::layout;
use crate::ui::Ui;

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

/// 右上角面板现在是不是展开的。点小三角是切换，所以要自己记着状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Collapsed,
    Expanded,
}

/// 设成英雄追踪 + `team` 一方的迷雾，结束时面板是展开的（剪刀按钮在展开的面板里）。
/// 调用前游戏要在前台
pub fn setup(ui: &mut Ui, panel: Panel, team: Team) -> Result<()> {
    if panel == Panel::Collapsed {
        ui.click(layout::PANEL_TOGGLE, "展开面板")?;
    }
    ui.click(layout::CAMERA_DROPDOWN, "镜头下拉")?;
    ui.click(layout::CAMERA_HERO_CHASE, "英雄追踪")?;
    ui.click(layout::FOG_DROPDOWN, "迷雾下拉")?;
    let fog = match team {
        Team::Radiant => layout::FOG_RADIANT,
        Team::Dire => layout::FOG_DIRE,
    };
    ui.click(fog, "阵营迷雾")
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
