//! 给游戏窗口发按键，用于观战界面的操作（切换追踪的玩家）。
//!
//! 用 PostMessage 直接投递到窗口，不需要游戏在前台，也不影响用户的键盘。

use anyhow::{Result, bail};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VK_TO_VSC, MapVirtualKeyW};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_KEYDOWN, WM_KEYUP};
use windows::core::w;

/// 游戏主窗口（类名 SDL_app）。注意 `Process.MainWindowHandle` 可能拿到输入法窗口 VALVEIME001，不能用它
pub fn dota_window() -> Result<HWND> {
    match unsafe { FindWindowW(w!("SDL_app"), w!("Dota 2")) } {
        Ok(h) if !h.is_invalid() => Ok(h),
        _ => bail!("找不到 Dota 2 主窗口"),
    }
}

pub fn post_key(window: HWND, vk: u16) -> Result<()> {
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as isize;
    let down = 1 | (scan << 16);
    let up = down | (0b11 << 30);
    unsafe {
        PostMessageW(window, WM_KEYDOWN, WPARAM(vk as usize), LPARAM(down))?;
        std::thread::sleep(std::time::Duration::from_millis(60));
        PostMessageW(window, WM_KEYUP, WPARAM(vk as usize), LPARAM(up))?;
    }
    Ok(())
}

/// 追踪玩家序号 `player`（0–9）的默认热键：数字键 1–9、0（`SpectatorCameraFocusPlayer1..10`）。
/// 用户改过热键时不对，正式版要读 dotakeys_personal.lst
pub fn focus_player_key(player: usize) -> u16 {
    let digit = (player + 1) % 10;
    b'0' as u16 + digit as u16
}
