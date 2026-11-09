//! 游戏窗口：发按键（切换追踪的玩家、迷雾），最小化、放到后面，以及"回放开始播放时会抢前台"的记号。
//!
//! 按键用 PostMessage 直接投递到窗口，不需要游戏在前台，也不影响用户的键盘。
//!
//! 回放每次从暂停开始播放（`demo_resume`），Dota 都会把自己的窗口提到最前面、抢走前台：最小化的会恢复，
//! 放在后面的会跳到最前；移到屏幕外也照抢（2026-11-08 实测）。只抢一次，还回去之后不再抢。
//! 所以录制时游戏放在其他窗口后面，程序每次让回放播放前调 `expect_focus_steal`，在随后的一小段时间里
//! 把前台还给用户（`pipeline`），输入检测也不把这段时间的输入算成用户在游戏里操作（`input_guard`）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VK_TO_VSC, MapVirtualKeyW};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, HWND_BOTTOM, IsIconic, PostMessageW, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos, ShowWindow,
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

/// 窗口现在是不是最小化的
pub fn is_minimized(window: HWND) -> bool {
    unsafe { IsIconic(window) }.as_bool()
}

/// 放到所有窗口后面，不抢焦点
pub fn send_to_back(window: HWND) {
    unsafe {
        let _ = SetWindowPos(window, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

/// 程序让回放开始播放后，Dota 抢前台的时间窗
const STEAL_WINDOW: Duration = Duration::from_millis(1500);
/// 上次让回放播放的时间（Unix 毫秒）
static RESUMED_AT: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// 程序要让回放播放了（发 `demo_resume` 前调）：接下来 Dota 会抢前台
pub fn expect_focus_steal() {
    RESUMED_AT.store(now_ms(), Ordering::SeqCst);
}

/// 现在是不是在"程序刚让回放播放、Dota 可能正在抢前台"的时间窗里
pub fn focus_steal_expected() -> bool {
    now_ms().saturating_sub(RESUMED_AT.load(Ordering::SeqCst)) < STEAL_WINDOW.as_millis() as u64
}
