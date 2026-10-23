//! 操作游戏界面：真实移动鼠标、点击、拖动、截图。
//!
//! 观战菜单和剪辑工具没有控制台命令，只能点界面。实测有效的做法（验证 5、6）：
//! `SetCursorPos` 把系统光标放到目标点，再用 `SendInput` 发一个 ±1 像素的相对移动，游戏才会把它自己的光标移过去；
//! 点击本身用 `PostMessage` 发 `WM_LBUTTONDOWN/UP`（`SendInput` 的点击游戏不响应）。
//! 这些操作要求游戏在前台，期间用户不能碰鼠标键盘。坐标全部按物理像素算（每显示器 DPI 感知）。

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput, VK_MENU,
    keybd_event,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetCursorPos, GetForegroundWindow, PostMessageW, SetCursorPos, SetForegroundWindow, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE,
};

use crate::layout::{Pt, Spot};

/// 鼠标消息 wParam 里"左键按着"的标记（MK_LBUTTON）
const LBUTTON: WPARAM = WPARAM(0x0001);

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

fn lparam(p: Pt) -> LPARAM {
    LPARAM((((p.y as u32) << 16) | (p.x as u32 & 0xFFFF)) as isize)
}

fn nudge(dx: i32, dy: i32) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, dwFlags: MOUSEEVENTF_MOVE, ..Default::default() } },
    };
    unsafe {
        SendInput(&[input], size_of::<INPUT>() as i32);
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

    pub fn at(&self, spot: Spot) -> Pt {
        spot.at(self.width, self.height)
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

    fn to_screen(&self, p: Pt) -> POINT {
        let mut s = POINT { x: p.x, y: p.y };
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut s);
        }
        s
    }

    /// 让游戏内的光标移到 `p`：放好系统光标，再微移让游戏收到移动事件；有偏差就按实际位置校正
    fn move_to(&self, p: Pt, held: bool) -> Result<()> {
        // 按了 Ctrl+C 就不再动鼠标（拖动到一半时也停；按下的左键由调用方之后的点击或用户自己松开）
        crate::cancel::check()?;
        let target = self.to_screen(p);
        unsafe {
            SetCursorPos(target.x, target.y)?;
        }
        nudge(1, 0);
        std::thread::sleep(Duration::from_millis(30));
        nudge(-1, 0);
        for _ in 0..15 {
            std::thread::sleep(Duration::from_millis(60));
            let mut c = POINT::default();
            unsafe { GetCursorPos(&mut c)? };
            let (ex, ey) = (target.x - c.x, target.y - c.y);
            if ex.abs() <= 2 && ey.abs() <= 2 {
                break;
            }
            nudge((ex as f32 * 0.7).round() as i32, (ey as f32 * 0.7).round() as i32);
        }
        let wparam = if held { LBUTTON } else { WPARAM(0) };
        unsafe { PostMessageW(self.hwnd, WM_MOUSEMOVE, wparam, lparam(p))? };
        Ok(())
    }

    pub fn hover(&self, spot: Spot) -> Result<()> {
        self.move_to(self.at(spot), false)
    }

    pub fn click(&mut self, spot: Spot, label: &str) -> Result<()> {
        let p = self.at(spot);
        self.move_to(p, false)?;
        std::thread::sleep(Duration::from_millis(250));
        unsafe {
            PostMessageW(self.hwnd, WM_LBUTTONDOWN, LBUTTON, lparam(p))?;
            std::thread::sleep(Duration::from_millis(120));
            PostMessageW(self.hwnd, WM_LBUTTONUP, WPARAM(0), lparam(p))?;
        }
        std::thread::sleep(Duration::from_millis(700));
        self.shot(label)
    }

    /// 从 `from` 按住拖到 `to`（分 12 步，每步都带按键状态）
    pub fn drag(&mut self, from: Spot, to: Spot, label: &str) -> Result<()> {
        let (a, b) = (self.at(from), self.at(to));
        self.move_to(a, false)?;
        std::thread::sleep(Duration::from_millis(250));
        unsafe { PostMessageW(self.hwnd, WM_LBUTTONDOWN, LBUTTON, lparam(a))? };
        std::thread::sleep(Duration::from_millis(150));
        for i in 1..=12 {
            let p = Pt { x: a.x + (b.x - a.x) * i / 12, y: a.y + (b.y - a.y) * i / 12 };
            if let Err(e) = self.move_to(p, true) {
                // 拖到一半被打断（比如 Ctrl+C）也要松开左键
                unsafe {
                    let _ = PostMessageW(self.hwnd, WM_LBUTTONUP, WPARAM(0), lparam(p));
                }
                return Err(e);
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        std::thread::sleep(Duration::from_millis(150));
        unsafe { PostMessageW(self.hwnd, WM_LBUTTONUP, WPARAM(0), lparam(b))? };
        std::thread::sleep(Duration::from_millis(500));
        self.shot(label)
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
        let origin = self.to_screen(Pt { x: 0, y: 0 });
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

impl Frame {
    /// (r, g, b)；越界返回黑色
    pub fn rgb(&self, p: Pt) -> (u8, u8, u8) {
        if p.x < 0 || p.y < 0 || p.x >= self.width || p.y >= self.height {
            return (0, 0, 0);
        }
        let i = ((p.y * self.width + p.x) * 4) as usize;
        (self.bgra[i + 2], self.bgra[i + 1], self.bgra[i])
    }
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
