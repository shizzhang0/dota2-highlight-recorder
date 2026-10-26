//! 取消：Ctrl+C、录制期间检测到用户动了鼠标键盘、按了取消快捷键，都只做标记，
//! 各个等待点看到标记就停下来，让调用方照常恢复设置、删 GSI 配置、交还前台。
//! Ctrl+C 第二次按才交给系统默认处理（直接结束进程，设置来不及恢复）。

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use anyhow::{Result, bail};
use windows::Win32::Foundation::BOOL;
use windows::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};

/// 为什么停下
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    CtrlC = 1,
    /// 录制期间检测到真实的鼠标键盘输入
    Input = 2,
    /// 按了取消快捷键
    Hotkey = 3,
}

impl Reason {
    pub fn describe(self) -> &'static str {
        match self {
            Reason::CtrlC => "按了 Ctrl+C",
            Reason::Input => "检测到鼠标或键盘操作",
            Reason::Hotkey => "按了取消快捷键",
        }
    }
}

/// 0 = 没有取消；否则是 `Reason` 的值（只记第一次）
static REQUESTED: AtomicU8 = AtomicU8::new(0);
/// 大于 0 时暂时不理会取消标记：恢复设置本身也要等控制台回应，不能被打断
static HELD: AtomicUsize = AtomicUsize::new(0);

/// 标记取消；已经标记过就返回 false
pub fn request(reason: Reason) -> bool {
    REQUESTED.compare_exchange(0, reason as u8, Ordering::SeqCst, Ordering::SeqCst).is_ok()
}

unsafe extern "system" fn handler(event: u32) -> BOOL {
    if event != CTRL_C_EVENT && event != CTRL_BREAK_EVENT {
        return BOOL(0);
    }
    if !request(Reason::CtrlC) {
        // 已经在停了：交给默认处理
        return BOOL(0);
    }
    eprintln!("\n收到 Ctrl+C：做完手上这一步就停下，恢复游戏设置。再按一次 Ctrl+C 会立刻退出，设置来不及恢复。");
    BOOL(1)
}

/// 接管 Ctrl+C（命令行用）
pub fn install() -> Result<()> {
    unsafe { SetConsoleCtrlHandler(Some(handler), true)? };
    Ok(())
}

/// 取消原因；没取消、或者正处在 `hold` 期间返回 None
pub fn reason() -> Option<Reason> {
    if HELD.load(Ordering::SeqCst) > 0 {
        return None;
    }
    match REQUESTED.load(Ordering::SeqCst) {
        1 => Some(Reason::CtrlC),
        2 => Some(Reason::Input),
        3 => Some(Reason::Hotkey),
        _ => None,
    }
}

pub fn requested() -> bool {
    reason().is_some()
}

/// 清掉取消标记，开始新的一轮录制前调用（桌面应用里一个进程会录很多次）
pub fn reset() {
    REQUESTED.store(0, Ordering::SeqCst);
}

/// 返回值存活期间不响应取消标记（Ctrl+C 再按一次仍会直接结束进程）
pub struct Hold(());

pub fn hold() -> Hold {
    HELD.fetch_add(1, Ordering::SeqCst);
    Hold(())
}

impl Drop for Hold {
    fn drop(&mut self) {
        HELD.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 取消了就返回错误
pub fn check() -> Result<()> {
    if let Some(r) = reason() {
        bail!("已停止（{}）", r.describe());
    }
    Ok(())
}
