//! Ctrl+C：第一次只做标记，各个等待点看到标记就停下来，让调用方照常恢复设置、删 GSI 配置、交还前台；
//! 第二次按才交给系统默认处理（直接结束进程，设置来不及恢复）。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use anyhow::{Result, bail};
use windows::Win32::Foundation::BOOL;
use windows::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};

static REQUESTED: AtomicBool = AtomicBool::new(false);
/// 大于 0 时暂时不理会 Ctrl+C 标记：恢复设置本身也要等控制台回应，不能被打断
static HELD: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn handler(event: u32) -> BOOL {
    if event != CTRL_C_EVENT && event != CTRL_BREAK_EVENT {
        return BOOL(0);
    }
    if REQUESTED.swap(true, Ordering::SeqCst) {
        // 第二次：交给默认处理
        return BOOL(0);
    }
    eprintln!("\n收到 Ctrl+C：做完手上这一步就停下，恢复游戏设置。再按一次 Ctrl+C 会立刻退出，设置来不及恢复。");
    BOOL(1)
}

/// 接管 Ctrl+C
pub fn install() -> Result<()> {
    unsafe { SetConsoleCtrlHandler(Some(handler), true)? };
    Ok(())
}

pub fn requested() -> bool {
    REQUESTED.load(Ordering::SeqCst) && HELD.load(Ordering::SeqCst) == 0
}

/// 返回值存活期间不响应 Ctrl+C 标记（再按一次 Ctrl+C 仍会直接结束进程）
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

/// 用户按过 Ctrl+C 就返回错误
pub fn check() -> Result<()> {
    if requested() {
        bail!("已取消（Ctrl+C）");
    }
    Ok(())
}
