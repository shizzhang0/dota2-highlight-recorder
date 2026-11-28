//! 游戏窗口：大小、切到前台、截图。坐标按物理像素算（每显示器 DPI 感知）。
//!
//! 原来还有真实移动鼠标、点击、拖动（点观战面板和视频剪辑工具用，验证 5、6）：`SetCursorPos` 放好系统光标，
//! 再用 `SendInput` 发 ±1 像素的相对移动游戏才认，点击用 `PostMessage` 发 `WM_LBUTTONDOWN/UP`。
//! 观战设置早就改成控制台命令、录制改成 `startmovie`，2026-11-28 删掉了，要用翻 git 历史

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
use windows::Win32::UI::Input::KeyboardAndMouse::{KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_MENU, keybd_event};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetCursorPos, GetForegroundWindow, SetCursorPos, SetForegroundWindow};

pub struct Ui {
    hwnd: HWND,
    pub width: i32,
    pub height: i32,
    /// 诊断截图存到这里；None 时不截图
    diag: Option<PathBuf>,
    shots: u32,
}

fn dpi_aware() {
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// 把前台还给原来的窗口
pub struct Foreground {
    previous: HWND,
    cursor: POINT,
}

impl Drop for Foreground {
    fn drop(&mut self) {
        unsafe {
            let _ = SetCursorPos(self.cursor.x, self.cursor.y);
            alt_tap();
            let _ = SetForegroundWindow(self.previous);
        }
    }
}

/// 当前前台窗口（原始句柄值）。录制开始时记下，结束时用 `bring_to_front` 还回去
pub fn foreground() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
}

/// 把窗口切回前台，最多试 5 次，成功返回 true。
/// 每段导出完 `Foreground` 已经还过一次，但 2026-10-26 测试时录完后游戏仍在前台，所以结束时再兜底切一次
pub fn bring_to_front(hwnd: isize) -> bool {
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    if hwnd.is_invalid() {
        return false;
    }
    for _ in 0..5 {
        unsafe {
            alt_tap();
            let _ = SetForegroundWindow(hwnd);
            std::thread::sleep(Duration::from_millis(300));
            if GetForegroundWindow() == hwnd {
                return true;
            }
        }
    }
    false
}

/// 后台进程直接 SetForegroundWindow 常被系统拒绝，先模拟按一下 Alt
unsafe fn alt_tap() {
    unsafe {
        keybd_event(VK_MENU.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
    }
}

impl Ui {
    /// 找到游戏主窗口（类名 SDL_app；`Process.MainWindowHandle` 可能拿到输入法窗口，不能用）
    pub fn attach() -> Result<Ui> {
        dpi_aware();
        let hwnd = crate::window::dota_window()?;
        let mut rc = RECT::default();
        unsafe { GetClientRect(hwnd, &mut rc)? };
        Ok(Ui { hwnd, width: rc.right - rc.left, height: rc.bottom - rc.top, diag: None, shots: 0 })
    }

    pub fn set_diag_dir(&mut self, dir: impl Into<PathBuf>) -> Result<()> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        self.diag = Some(dir);
        Ok(())
    }

    /// 把游戏切到前台；返回值离开作用域时，前台和鼠标位置还给原来的窗口
    pub fn front(&self) -> Result<Foreground> {
        dpi_aware();
        let mut cursor = POINT::default();
        unsafe {
            let previous = GetForegroundWindow();
            GetCursorPos(&mut cursor)?;
            alt_tap();
            let _ = SetForegroundWindow(self.hwnd);
            std::thread::sleep(Duration::from_millis(400));
            if GetForegroundWindow() != self.hwnd {
                bail!("没能把游戏切到前台");
            }
            Ok(Foreground { previous, cursor })
        }
    }

    /// 客户区左上角在屏幕上的位置
    fn origin(&self) -> POINT {
        let mut s = POINT { x: 0, y: 0 };
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut s);
        }
        s
    }

    /// 截游戏窗口客户区（截的是屏幕，所以游戏要在前台），存成 BMP 供排查。没设诊断目录时什么都不做
    pub fn shot(&mut self, label: &str) -> Result<()> {
        let Some(dir) = self.diag.clone() else { return Ok(()) };
        self.shots += 1;
        let path = dir.join(format!("{:03}_{label}.bmp", self.shots));
        self.capture(&path)
    }

    fn capture(&self, path: &Path) -> Result<()> {
        let frame = self.grab()?;
        write_bmp(path, frame.width, frame.height, &frame.bgra).with_context(|| format!("存不了截图 {}", path.display()))
    }

    /// 截下游戏窗口客户区的像素（截的是屏幕，游戏要在前台）
    pub fn grab(&self) -> Result<Frame> {
        let (w, h) = (self.width, self.height);
        let origin = self.origin();
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        unsafe {
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(screen);
            let bmp = CreateCompatibleBitmap(screen, w, h);
            let old = SelectObject(mem, bmp);
            BitBlt(mem, 0, 0, w, h, screen, origin.x, origin.y, SRCCOPY)?;
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // 负数 = 自上而下
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            GetDIBits(mem, bmp, 0, h as u32, Some(pixels.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
        }
        Ok(Frame { width: w, height: h, bgra: pixels })
    }
}

/// 一张截图：自上而下、每像素 BGRA
pub struct Frame {
    pub width: i32,
    pub height: i32,
    pub bgra: Vec<u8>,
}

/// 32 位、自上而下的 BMP，不为截图再加图片库依赖
fn write_bmp(path: &Path, w: i32, h: i32, bgra: &[u8]) -> std::io::Result<()> {
    let mut out = Vec::with_capacity(54 + bgra.len());
    let file_size = 54 + bgra.len() as u32;
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&file_size.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&(-h).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    out.extend_from_slice(bgra);
    std::fs::write(path, out)
}
