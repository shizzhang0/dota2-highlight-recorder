//! 游戏窗口：发按键（切换追踪的玩家、迷雾），最小化。
//!
//! 按键用 PostMessage 直接投递到窗口，不需要游戏在前台，也不影响用户的键盘。

use anyhow::{Result, bail};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VK_TO_VSC, MapVirtualKeyW};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, HWND_BOTTOM, PostMessageW, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos, ShowWindow,
    WM_KEYDOWN, WM_KEYUP,
};
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

/// 最小化，不抢焦点。录制期间用：最小化了照样出帧（2026-11-07 实测），用户也不会误点进游戏
pub fn minimize(window: HWND) {
    unsafe {
        let _ = ShowWindow(window, SW_SHOWMINNOACTIVE);
    }
}

/// 从最小化恢复，不抢焦点
pub fn restore(window: HWND) {
    unsafe {
        let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
    }
}

/// 放到所有窗口后面，不抢焦点
pub fn send_to_back(window: HWND) {
    unsafe {
        let _ = SetWindowPos(window, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}
