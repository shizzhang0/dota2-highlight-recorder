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

/// 按数字键追踪玩家 `player`（0–9），用 GSI 确认。按键用 PostMessage，不需要前台
pub fn focus(player: usize, gsi: &Receiver<Value>) -> Result<()> {
    let window = crate::window::dota_window()?;
    crate::gsi::drain(gsi);
    crate::window::post_key(window, crate::window::focus_player_key(player))?;
    // 暂停时 GSI 几秒才一个包，多等一会
    crate::gsi::wait_next(gsi, Duration::from_secs(10), &format!("追踪玩家 #{player}"), |s| s.selected == Some(player))
        .map(drop)
        .context("按键后追踪的不是这个玩家（用户改过观战热键？）")
}
