//! 录制期间检测用户动了鼠标键盘：按任意键或移动鼠标，命令行里就停止（`cancel::Reason::Input`），桌面应用里暂停。
//! 原先另注册了全局快捷键 `Ctrl+Shift+F12`，但按下 Ctrl 那一刻就已经停了，快捷键多余，2026-10-26 去掉。
//!
//! 用低级键盘、鼠标钩子（`WH_KEYBOARD_LL` / `WH_MOUSE_LL`）：程序用 `SendInput` / `keybd_event`
//! 发的事件带 `INJECTED` 标记，`PostMessage` 发的点击和按键根本不经过钩子，所以没标记的就是用户的真实输入。
//! 低级钩子要求装钩子的线程跑消息循环，所以单独开一个线程。

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLKHF_LOWER_IL_INJECTED, LLMHF_INJECTED,
    LLMHF_LOWER_IL_INJECTED, MSG, MSLLHOOKSTRUCT, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_MOUSEMOVE, WM_QUIT,
};

use crate::cancel::{self, Reason};

// 程序自己刚用 SetCursorPos 放过光标的位置和时间（毫秒）：这种移动万一不带 INJECTED 标记，也别当成用户输入
static EXPECT_X: AtomicI32 = AtomicI32::new(i32::MIN);
static EXPECT_Y: AtomicI32 = AtomicI32::new(i32::MIN);
static EXPECT_AT: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// 程序要把光标放到屏幕坐标 (x, y) 了（`ui` 在调用 SetCursorPos 前告诉这里）
pub fn expect_cursor(x: i32, y: i32) {
    EXPECT_X.store(x, Ordering::SeqCst);
    EXPECT_Y.store(y, Ordering::SeqCst);
    EXPECT_AT.store(now_ms(), Ordering::SeqCst);
}

fn ours(x: i32, y: i32) -> bool {
    now_ms().saturating_sub(EXPECT_AT.load(Ordering::SeqCst)) < 500
        && (x - EXPECT_X.load(Ordering::SeqCst)).abs() <= 2
        && (y - EXPECT_Y.load(Ordering::SeqCst)).abs() <= 2
}

/// 检测到用户输入时怎么办
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnInput {
    /// 停止（命令行：没地方点"继续"）
    Stop,
    /// 暂停，等用户在应用里点"继续"或"停止"
    Pause,
}

static PAUSE_MODE: AtomicBool = AtomicBool::new(false);

fn user_input(what: &str) {
    if PAUSE_MODE.load(Ordering::SeqCst) {
        cancel::request_pause();
    } else if cancel::request(Reason::Input) {
        eprintln!("\n检测到{what}操作：做完手上这一步就停下，恢复游戏设置。");
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let injected = info.flags & (LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED) != 0;
        let ours = wparam.0 as u32 == WM_MOUSEMOVE && ours(info.pt.x, info.pt.y);
        if !injected && !ours {
            user_input("鼠标");
        }
    }
    unsafe { CallNextHookEx(HHOOK::default(), code, wparam, lparam) }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let injected = info.flags.0 & (LLKHF_INJECTED.0 | LLKHF_LOWER_IL_INJECTED.0) != 0;
        if !injected {
            user_input("键盘");
        }
    }
    unsafe { CallNextHookEx(HHOOK::default(), code, wparam, lparam) }
}

/// 开着检测；离开作用域时卸掉钩子
pub struct Guard {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

/// 开始检测
pub fn start(on_input: OnInput) -> Result<Guard> {
    PAUSE_MODE.store(on_input == OnInput::Pause, Ordering::SeqCst);
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
    let thread_id = rx.recv().map_err(|_| anyhow!("输入检测线程没起来"))??;
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
