//! 游戏界面上要点的位置。都是在 2560×1600 窗口（150% 系统缩放）下量的物理像素，
//! 按锚点换算到实际窗口：Panorama 界面按窗口高度等比缩放，右上角的观战菜单锚定右边和上边，
//! 剪辑工具锚定底部居中，对话框居中。其他分辨率下是否准确要在里程碑 4 实测。

/// 客户区坐标（物理像素）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pt {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy)]
pub enum Anchor {
    TopRight,
    TopCenter,
    Center,
    BottomCenter,
}

/// 一个界面位置：锚点 + 参考分辨率下的坐标
#[derive(Debug, Clone, Copy)]
pub struct Spot {
    pub anchor: Anchor,
    pub x: f32,
    pub y: f32,
}

const REF_W: f32 = 2560.0;
const REF_H: f32 = 1600.0;

const fn spot(anchor: Anchor, x: f32, y: f32) -> Spot {
    Spot { anchor, x, y }
}

impl Spot {
    /// 换算到宽 `w`、高 `h` 的客户区
    pub fn at(self, w: i32, h: i32) -> Pt {
        let (w, h) = (w as f32, h as f32);
        let s = h / REF_H;
        let (x, y) = match self.anchor {
            Anchor::TopRight => (w - (REF_W - self.x) * s, self.y * s),
            Anchor::TopCenter => (w / 2.0 + (self.x - REF_W / 2.0) * s, self.y * s),
            Anchor::Center => (w / 2.0 + (self.x - REF_W / 2.0) * s, h / 2.0 + (self.y - REF_H / 2.0) * s),
            Anchor::BottomCenter => (w / 2.0 + (self.x - REF_W / 2.0) * s, h - (REF_H - self.y) * s),
        };
        Pt { x: x.round() as i32, y: y.round() as i32 }
    }

    /// `at` 的反算：客户区横坐标 `px` 在这个锚点下对应的参考横坐标
    pub fn ref_x(self, px: i32, w: i32, h: i32) -> f32 {
        let (w, h) = (w as f32, h as f32);
        let s = h / REF_H;
        match self.anchor {
            Anchor::TopRight => REF_W - (w - px as f32) / s,
            Anchor::TopCenter | Anchor::Center | Anchor::BottomCenter => REF_W / 2.0 + (px as f32 - w / 2.0) / s,
        }
    }

    /// 同一锚点下，水平偏移 `dx` 个参考像素
    pub fn shift_x(self, dx: f32) -> Spot {
        Spot { x: self.x + dx, ..self }
    }
}

// 右上角观战菜单（刚加载录像时面板是收起的）
/// 展开 / 收起面板的小三角
pub const PANEL_TOGGLE: Spot = spot(Anchor::TopRight, 2465.0, 106.0);
/// 面板展开后：镜头模式下拉框，以及其中的"英雄追踪"
pub const CAMERA_DROPDOWN: Spot = spot(Anchor::TopRight, 2380.0, 182.0);
pub const CAMERA_HERO_CHASE: Spot = spot(Anchor::TopRight, 2300.0, 370.0);
/// 面板展开后：战争迷雾下拉框，以及其中的"迷雾：天辉"/"迷雾：夜魇"
pub const FOG_DROPDOWN: Spot = spot(Anchor::TopRight, 2380.0, 242.0);
pub const FOG_RADIANT: Spot = spot(Anchor::TopRight, 2310.0, 337.0);
pub const FOG_DIRE: Spot = spot(Anchor::TopRight, 2310.0, 384.0);
/// 面板展开后：左下角的剪刀（视频剪辑工具）
pub const SCISSORS: Spot = spot(Anchor::TopRight, 2198.0, 520.0);

// 顶部英雄栏
/// 玩家 `player`（0–4 天辉在左，5–9 夜魇在右）在顶部英雄栏的头像。从截图估的位置
/// （离中间比分最近的头像中心约 207 像素，头像间距约 91 像素），用于"点头像选人"的对比实验
pub fn top_bar_hero(player: usize) -> Spot {
    let x = if player < 5 { 1280.0 - 207.0 - 91.0 * (4 - player) as f32 } else { 1280.0 + 207.0 + 91.0 * (player - 5) as f32 };
    spot(Anchor::TopCenter, x, 30.0)
}

// 剪辑工具（屏幕下方）
/// 时间轴横条两端（把手指向的位置）
pub const BAR_START: Spot = spot(Anchor::BottomCenter, 715.0, 1452.0);
pub const BAR_END: Spot = spot(Anchor::BottomCenter, 1807.0, 1452.0);
/// 时间轴上方、播放头滑块穿过的一行：这一行只有滑块是亮的，用来找播放头的实际位置
/// （2026-10-22 实测播放头不一定在时间轴正中，偏了 22 像素）
pub const PLAYHEAD_ROW: Spot = spot(Anchor::BottomCenter, 1280.0, 1415.0);
/// 刚打开时两个选区把手的位置（固定像素，不随时间轴长度变化）
pub const HANDLE_LEFT: Spot = spot(Anchor::BottomCenter, 1117.0, 1452.0);
pub const HANDLE_RIGHT: Spot = spot(Anchor::BottomCenter, 1438.0, 1452.0);
/// "导出视频"按钮、关闭按钮
pub const EXPORT: Spot = spot(Anchor::BottomCenter, 1840.0, 1528.0);
pub const CLOSE: Spot = spot(Anchor::BottomCenter, 1914.0, 1278.0);

// 对话框（屏幕中间）
/// "视频导出设置"里的"导出视频"
pub const DIALOG_EXPORT: Spot = spot(Anchor::Center, 1124.0, 1036.0);
/// "导出视频成功"里的"确定"
pub const SUCCESS_OK: Spot = spot(Anchor::Center, 1280.0, 880.0);
