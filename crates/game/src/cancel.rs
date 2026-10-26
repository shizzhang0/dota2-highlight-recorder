//! 停止和暂停。
//!
//! 停止：Ctrl+C、录制期间检测到用户动了鼠标键盘（命令行）、在应用里点了停止，都只做标记，
//! 各个等待点看到标记就停下来，让调用方照常恢复设置、删 GSI 配置、交还前台。
//! Ctrl+C 第二次按才交给系统默认处理（直接结束进程，设置来不及恢复）。
//!
//! 暂停（桌面应用）：检测到用户动了鼠标键盘只是暂停。正在点界面就马上停手（`ui` 看到暂停就报错），
//! 正在渲染就让它渲染完；之后等用户在应用里点"继续"（`resume`）或"停止"。

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use windows::Win32::Foundation::BOOL;
use windows::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};

/// 为什么停下
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    CtrlC = 1,
    /// 录制期间检测到真实的鼠标键盘输入（包括按 Esc）
    Input = 2,
    /// 在桌面应用里点了停止
    Stop = 3,
}

impl Reason {
    pub fn describe(self) -> &'static str {
        match self {
            Reason::CtrlC => "按了 Ctrl+C",
            Reason::Input => "检测到鼠标或键盘操作",
            Reason::Stop => "点了停止",
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
    pending()
}

/// 有没有人要求停下（不管是否在 `hold` 期间）。界面用来马上显示"正在停下"
pub fn pending() -> Option<Reason> {
    match REQUESTED.load(Ordering::SeqCst) {
        1 => Some(Reason::CtrlC),
        2 => Some(Reason::Input),
        3 => Some(Reason::Stop),
        _ => None,
    }
}

pub fn requested() -> bool {
    reason().is_some()
}

/// 清掉停止和暂停标记，开始新的一轮录制前调用（桌面应用里一个进程会录很多次）
pub fn reset() {
    REQUESTED.store(0, Ordering::SeqCst);
    PAUSED.store(false, Ordering::SeqCst);
    PAUSE_SEEN.store(false, Ordering::SeqCst);
}

static PAUSED: AtomicBool = AtomicBool::new(false);
/// 暂停过、还没重新加载录像恢复界面状态
static PAUSE_SEEN: AtomicBool = AtomicBool::new(false);
/// 点"继续"后的这段时间（Unix 毫秒）里不理会输入：点按钮、松开鼠标本身也是输入
static GRACE_UNTIL: AtomicU64 = AtomicU64::new(0);
const GRACE: Duration = Duration::from_millis(1500);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// 检测到用户输入时暂停；刚点完"继续"的宽限期内不算。返回是否这次才进入暂停
pub fn request_pause() -> bool {
    if now_ms() < GRACE_UNTIL.load(Ordering::SeqCst) {
        return false;
    }
    let first = !PAUSED.swap(true, Ordering::SeqCst);
    if first {
        PAUSE_SEEN.store(true, Ordering::SeqCst);
    }
    first
}

pub fn paused() -> bool {
    PAUSED.load(Ordering::SeqCst)
}

/// 用户点了"继续"
pub fn resume() {
    GRACE_UNTIL.store(now_ms() + GRACE.as_millis() as u64, Ordering::SeqCst);
    PAUSED.store(false, Ordering::SeqCst);
}

/// 暂停过的话返回 true 并清掉记号：调用方要重新加载录像、重设镜头，恢复到已知的界面状态
pub fn take_pause_seen() -> bool {
    PAUSE_SEEN.swap(false, Ordering::SeqCst)
}

/// 暂停中就一直等，直到用户点"继续"（返回 Ok）或者停止（返回错误）
pub fn wait_while_paused() -> Result<()> {
    while paused() {
        check()?;
        std::thread::sleep(Duration::from_millis(200));
    }
    check()
}

/// 暂停了就返回错误（点界面前用：用户在动鼠标，再点就会点错）
pub fn check_not_paused() -> Result<()> {
    check()?;
    if paused() {
        bail!("已暂停");
    }
    Ok(())
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
