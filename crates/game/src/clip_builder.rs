//! 用游戏自带的"视频剪辑工具"把一段回放导出成 mp4（验证 6）。
//!
//! 剪辑工具没有控制台命令，只能点界面：打开时时间轴是以当前播放位置为中心、长
//! `dota_clip_builder_max_duration` 秒的窗口，选区把手刚打开时在固定像素位置，要拖到起止时间对应的位置。
//! 导出后游戏从选区起点播到终点并渲染，写出 `clipN.mp4`。单次导出超过约 60 秒会得到 0 字节文件
//! （对话框照样说成功），所以要检查文件。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::gsi::State;
use crate::layout::{self, Pt, Spot};
use crate::netcon::Netcon;
use crate::ui::{Foreground, Frame, Ui};

/// 一次导出
#[derive(Debug, Clone)]
pub struct Job<'a> {
    /// 起止游戏时钟（秒）
    pub from: f32,
    pub to: f32,
    /// `(from + to) / 2` 对应的 tick：跳到这里再打开剪辑工具，时间轴就以它为中心
    pub center_tick: u32,
    /// 当前的 `dota_clip_builder_max_duration`
    pub window: f32,
    /// 当前的 `dota_clip_builder_output_folder`
    pub out_dir: &'a Path,
    /// 点完导出就把前台还回去（测试渲染时游戏不在前台行不行）
    pub background_render: bool,
}

#[derive(Debug)]
pub struct Exported {
    /// 游戏写出的 clipN.mp4
    pub path: PathBuf,
    pub bytes: u64,
    /// 从点导出到文件写完
    pub render: Duration,
    /// 渲染期间 GSI 报的游戏时钟（第一个、最后一个），用来核对实际导出的范围
    pub clock_first: Option<i64>,
    pub clock_last: Option<i64>,
}

/// 时间轴上时间 `t` 的位置。`playhead` 是播放头滑块的参考横坐标，它对应 `(from + to) / 2`
fn bar(job: &Job, playhead: f32, t: f32) -> Spot {
    let center = (job.from + job.to) / 2.0;
    let px_per_sec = (layout::BAR_END.x - layout::BAR_START.x) / job.window;
    Spot { x: playhead + (t - center) * px_per_sec, ..layout::HANDLE_LEFT }
}

/// 在截图里找播放头滑块，返回它中心的参考横坐标。时间轴那一行上只有滑块是亮的
fn find_playhead(ui: &Ui, frame: &Frame) -> Option<f32> {
    let row = ui.at(layout::PLAYHEAD_ROW).y;
    let (a, b) = (ui.at(layout::BAR_START).x, ui.at(layout::BAR_END).x);
    let bright: Vec<i32> = (a..=b)
        .filter(|&x| {
            let (r, g, bl) = frame.rgb(Pt { x, y: row });
            r > 150 && g > 150 && bl > 170
        })
        .collect();
    let (&first, &last) = (bright.first()?, bright.last()?);
    // 滑块约 15 像素宽（2560×1600 下）；太宽说明有别的亮东西，宁可报错
    let width = (last - first + 1) as f32 / (ui.height as f32 / 1600.0);
    if !(6.0..=30.0).contains(&width) || bright.len() as i32 != last - first + 1 {
        return None;
    }
    Some(layout::PLAYHEAD_ROW.ref_x((first + last) / 2, ui.width, ui.height))
}

/// 跳到 `tick`（游戏时钟 `clock` 秒）并暂停，用 GSI 确认真的到了。
/// 要在播放状态下跳：暂停时跳转，控制台照样报完成，但画面和剪辑工具的时间轴都不更新（2026-10-22 实测）
pub fn goto(con: &mut Netcon, gsi: &Receiver<Value>, tick: u32, clock: f32) -> Result<()> {
    con.send("demo_resume")?;
    con.seek(tick, Duration::from_secs(60))?;
    crate::gsi::drain(gsi);
    con.send("demo_pause")?;
    let what = format!("GSI 时钟到 {clock:.0} 秒");
    crate::gsi::wait_next(gsi, Duration::from_secs(15), &what, |s| s.clock.is_some_and(|c| (c as f32 - clock).abs() <= 2.0))
        .map(drop)
}

fn mp4s(dir: &Path) -> HashSet<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")))
                .collect()
        })
        .unwrap_or_default()
}

/// 导出 `job` 描述的一段。调用前录像已加载、镜头已设好、面板是展开的；结束时剪辑工具已关闭、回放暂停
pub fn export(ui: &mut Ui, con: &mut Netcon, gsi: &Receiver<Value>, job: &Job) -> Result<Exported> {
    let len = job.to - job.from;
    if len <= 0.0 || len > job.window - 4.0 {
        bail!("导出范围 {len:.1} 秒不合理（时间轴窗口 {} 秒）", job.window);
    }
    goto(con, gsi, job.center_tick, (job.from + job.to) / 2.0)?;
    std::thread::sleep(Duration::from_millis(500));
    let before = mp4s(job.out_dir);

    let mut front = Some(ui.front()?);
    ui.click(layout::SCISSORS, "打开剪辑工具")?;
    let playhead = match find_playhead(ui, &ui.grab()?) {
        Some(x) => x,
        None => {
            ui.click(layout::CLOSE, "找不到播放头")?;
            bail!("剪辑工具的时间轴上找不到播放头（剪辑工具没打开？截图在诊断目录）");
        }
    };
    // 先拖右把手：左把手的目标可能在右把手默认位置的右边
    ui.drag(layout::HANDLE_RIGHT, bar(job, playhead, job.to), "右把手")?;
    ui.drag(layout::HANDLE_LEFT, bar(job, playhead, job.from), "左把手")?;
    // 悬停时把手旁边会显示时间，截图留着核对
    ui.hover(bar(job, playhead, job.to))?;
    std::thread::sleep(Duration::from_millis(500));
    ui.shot("右把手时间")?;
    ui.hover(bar(job, playhead, job.from))?;
    std::thread::sleep(Duration::from_millis(500));
    ui.shot("左把手时间")?;
    ui.click(layout::EXPORT, "导出视频")?;
    // 丢掉之前积压的包，渲染期间的时钟才是导出的实际范围
    while gsi.try_recv().is_ok() {}
    ui.click(layout::DIALOG_EXPORT, "开始导出")?;
    if job.background_render {
        front = None;
    }

    let waited = wait_render(gsi, job, &before);

    // 不管成败都会弹"导出视频成功"，关掉它再关剪辑工具
    let _front: Foreground = match front {
        Some(f) => f,
        None => ui.front()?,
    };
    ui.shot("渲染结束")?;
    ui.click(layout::SUCCESS_OK, "关掉提示")?;
    ui.click(layout::CLOSE, "关闭剪辑工具")?;
    con.send("demo_pause")?;
    waited
}

fn wait_render(gsi: &Receiver<Value>, job: &Job, before: &HashSet<PathBuf>) -> Result<Exported> {
    let len = job.to - job.from;
    let started = Instant::now();
    let timeout = Duration::from_secs_f32(len * 3.0 + 60.0);
    let (mut first, mut last) = (None, None);
    let mut file: Option<PathBuf> = None;
    let (mut size, mut stable_since) = (0, Instant::now());
    loop {
        std::thread::sleep(Duration::from_millis(500));
        while let Ok(v) = gsi.try_recv() {
            if let Some(c) = State::from_json(&v).clock {
                first.get_or_insert(c);
                last = Some(c);
            }
        }
        if file.is_none() {
            file = mp4s(job.out_dir).difference(before).next().cloned();
        }
        let now = file.as_deref().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
        if now != size {
            size = now;
            stable_since = Instant::now();
        }
        let advanced = matches!((first, last), (Some(a), Some(b)) if b > a + 1);
        let reached_end = last.is_some_and(|c| c as f32 >= job.to.floor() - 1.0);
        let stable = stable_since.elapsed();
        if size > 0 && (stable >= Duration::from_secs(4) && reached_end || stable >= Duration::from_secs(10)) {
            let path = file.context("文件写完了却找不到路径")?;
            return Ok(Exported { path, bytes: size, render: started.elapsed(), clock_first: first, clock_last: last });
        }
        // 超长导出的失败表现：文件一直是 0 字节，游戏也不播放
        if size == 0 && !advanced && started.elapsed() > Duration::from_secs(20) {
            if let Some(p) = &file {
                let _ = std::fs::remove_file(p);
            }
            bail!("导出没有开始（文件 0 字节、回放没动），片段太长或没点到按钮");
        }
        if started.elapsed() > timeout {
            bail!("等了 {timeout:?} 导出还没完成（文件 {size} 字节，GSI 时钟 {first:?} → {last:?}）");
        }
    }
}
