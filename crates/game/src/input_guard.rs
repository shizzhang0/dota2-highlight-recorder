//! 录制期间把用户发给 Dota 的输入拦下，怎么操作都不会打乱录制（2026-11-10 定，见 docs/prototype-walkthrough.md D10）。
//! 原先是检测到操作就暂停（应用）或停止（命令行），暂停后要重设镜头、重录这一段；拦下来就用不着了。
//!
//! 拦什么：
//! - Dota 在前台时的按键；但 Ctrl、Shift、Alt、Win 键本身，以及 Alt+Tab、Alt+Esc、Ctrl+Esc（含 Ctrl+Shift+Esc）放行，
//!   用户能切到别的程序、关别的窗口。Alt+F4 这类按着 Alt 的其他键照样拦（不然会把游戏关掉）；
//! - 光标在 Dota 窗口上时的鼠标点击、滚轮。鼠标移动放行（不然光标会卡在 Dota 边上），光标在别的窗口（比如进度窗）上的点击放行。
//!
//! 按下被拦了，对应的松开也拦；按下没拦（比如在别的程序里按下、Dota 抢走前台后才松开）的松开放行，免得别的程序以为键一直按着。
//! 程序自己用 `SendInput` / `keybd_event` 发的输入带 `INJECTED` 标记，放行；`PostMessage` 发的根本不经过钩子。
//!
//! 用低级键盘、鼠标钩子（`WH_KEYBOARD_LL` / `WH_MOUSE_LL`），回调返回非零就把这次输入吞掉，后面的程序（包括 Dota）都收不到。
//! 低级钩子要求装钩子的线程跑消息循环，所以单独开一个线程。原先另注册的全局快捷键 `Ctrl+Shift+F12` 2026-10-26 去掉了。

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

use anyhow::{Result, anyhow};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_ESCAPE, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RMENU, VK_RSHIFT,
    VK_RWIN, VK_SHIFT, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GA_ROOT, GetAncestor, GetForegroundWindow, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, LLKHF_ALTDOWN, LLKHF_INJECTED,
    LLKHF_LOWER_IL_INJECTED, LLKHF_UP, LLMHF_INJECTED, LLMHF_LOWER_IL_INJECTED, MSG, MSLLHOOKSTRUCT, PostThreadMessageW,
    SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_XBUTTONDOWN, WM_XBUTTONUP, WindowFromPoint,
};

/// 游戏窗口句柄（开始拦截时找到的）
static DOTA: AtomicIsize = AtomicIsize::new(0);
/// 拦下了多少次（按下算一次，松开不算）
static BLOCKED: AtomicU64 = AtomicU64::new(0);
/// 哪些键的按下被拦了：它们的松开也要拦
static KEYS_HELD: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];
/// 哪些鼠标键的按下被拦了：左、右、中、侧键
static BUTTONS_HELD: [AtomicBool; 4] = [const { AtomicBool::new(false) }; 4];

/// 开始拦截以来拦下了多少次操作（界面、日志用来告诉用户"你的操作被拦下了，不影响录制"）
pub fn blocked() -> u64 {
    BLOCKED.load(Ordering::SeqCst)
}

fn dota() -> HWND {
    HWND(DOTA.load(Ordering::SeqCst) as *mut _)
}

/// 按着不拦的键：修饰键本身。切窗口的组合键要靠它们
fn is_modifier(vk: u16) -> bool {
    [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LSHIFT, VK_RSHIFT, VK_LCONTROL, VK_RCONTROL, VK_LMENU, VK_RMENU, VK_LWIN, VK_RWIN]
        .iter()
        .any(|k| k.0 == vk)
}

/// 切窗口的组合键：Alt+Tab、Alt+Esc、Ctrl+Esc、Ctrl+Shift+Esc
fn is_switch(vk: u16, alt: bool) -> bool {
    let ctrl = unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } < 0;
    (alt && (vk == VK_TAB.0 || vk == VK_ESCAPE.0)) || (ctrl && vk == VK_ESCAPE.0)
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let injected = info.flags.0 & (LLKHF_INJECTED.0 | LLKHF_LOWER_IL_INJECTED.0) != 0;
        let vk = info.vkCode as u16;
        let held = &KEYS_HELD[(vk & 0xff) as usize];
        if info.flags.0 & LLKHF_UP.0 != 0 {
            if held.swap(false, Ordering::SeqCst) {
                return LRESULT(1);
            }
        } else if !injected && unsafe { GetForegroundWindow() } == dota() && !is_modifier(vk) && !is_switch(vk, info.flags.0 & LLKHF_ALTDOWN.0 != 0) {
            // 按住不放会连续来按下，只在第一次计数
            if !held.swap(true, Ordering::SeqCst) {
                BLOCKED.fetch_add(1, Ordering::SeqCst);
            }
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(HHOOK::default(), code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let injected = info.flags & (LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED) != 0;
        let over_dota = || unsafe { GetAncestor(WindowFromPoint(info.pt), GA_ROOT) } == dota();
        let msg = wparam.0 as u32;
        let button = match msg {
            WM_LBUTTONDOWN | WM_LBUTTONUP => Some(0),
            WM_RBUTTONDOWN | WM_RBUTTONUP => Some(1),
            WM_MBUTTONDOWN | WM_MBUTTONUP => Some(2),
            WM_XBUTTONDOWN | WM_XBUTTONUP => Some(3),
            _ => None,
        };
        let down = matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN);
        let up = matches!(msg, WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP);
        let wheel = matches!(msg, WM_MOUSEWHEEL | WM_MOUSEHWHEEL);
        if up && let Some(b) = button {
            if BUTTONS_HELD[b].swap(false, Ordering::SeqCst) {
                return LRESULT(1);
            }
        } else if !injected && (down || wheel) && over_dota() {
            if let Some(b) = button {
                BUTTONS_HELD[b].store(true, Ordering::SeqCst);
            }
            BLOCKED.fetch_add(1, Ordering::SeqCst);
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(HHOOK::default(), code, wparam, lparam) }
}

/// 拦截开着；离开作用域时卸掉钩子
pub struct Guard {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

/// 开始拦截（游戏要已经开着）
pub fn start() -> Result<Guard> {
    let window = crate::window::dota_window()?;
    DOTA.store(window.0 as isize, Ordering::SeqCst);
    BLOCKED.store(0, Ordering::SeqCst);
    KEYS_HELD.iter().chain(&BUTTONS_HELD).for_each(|k| k.store(false, Ordering::SeqCst));
    let (tx, rx) = mpsc::channel();
    let thread = std::thread::spawn(move || unsafe {
        let module = GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default();
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0);
        let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0);
        let ready = match (&mouse, &keyboard) {
            (Ok(_), Ok(_)) => Ok(GetCurrentThreadId()),
            (Err(e), _) | (_, Err(e)) => Err(anyhow!("装不上鼠标键盘钩子：{e}")),
        };
        let failed = ready.is_err();
        let _ = tx.send(ready);
        if !failed {
            // 低级钩子靠这个线程的消息循环送达；GetMessage 收到 WM_QUIT 返回 0，出错返回 -1
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {}
        }
        if let Ok(h) = mouse {
            let _ = UnhookWindowsHookEx(h);
        }
        if let Ok(h) = keyboard {
            let _ = UnhookWindowsHookEx(h);
        }
    });
    let thread_id = rx.recv().map_err(|_| anyhow!("拦截输入的线程没起来"))??;
    Ok(Guard { thread_id, thread: Some(thread) })
}

impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
