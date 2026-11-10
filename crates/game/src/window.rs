//! 游戏窗口：发按键（切换追踪的玩家、迷雾），最小化、恢复，窗口在屏幕上的位置。
//!
//! 按键用 PostMessage 直接投递到窗口，不需要游戏在前台，也不影响用户的键盘。
//!
//! 回放每次从暂停开始播放（`demo_resume`），Dota 都会把自己的窗口提到最前面、抢走前台：最小化的会恢复，
//! 放在后面的会跳到最前；移到屏幕外也照抢（2026-11-08 实测）。所以录制时游戏就放在最前面（`pipeline`）。

use anyhow::{Result, bail};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VK_TO_VSC, MapVirtualKeyW};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetClientRect, IsIconic, PostMessageW, SW_HIDE, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, ShowWindow, WM_KEYDOWN, WM_KEYUP,
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

/// 隐藏窗口
pub fn hide(window: HWND) {
    unsafe {
        let _ = ShowWindow(window, SW_HIDE);
    }
}

/// 窗口现在是不是最小化的
pub fn is_minimized(window: HWND) -> bool {
    unsafe { IsIconic(window) }.as_bool()
}

/// 游戏画面（客户区）左上角在屏幕上的位置和画面大小，物理像素
pub fn client_rect(window: HWND) -> Option<(i32, i32, i32, i32)> {
    let mut r = RECT::default();
    let mut origin = POINT::default();
    unsafe {
        GetClientRect(window, &mut r).ok()?;
        ClientToScreen(window, &mut origin).ok().ok()?;
    }
    Some((origin.x, origin.y, r.right - r.left, r.bottom - r.top))
}
