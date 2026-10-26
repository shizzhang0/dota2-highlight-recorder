//! `d2hr record`：录制流程在 `pipeline`，这里把进度打印出来、从标准输入读确认。

use std::io::{BufRead, Write};

use anyhow::{Result, bail};
use clips::Params;
use media::ffmpeg::Ffmpeg;
use pipeline::{Event, Host, Options, Selected};
use replay::Replay;

struct Console;

impl Host for Console {
    fn event(&self, e: Event) {
        match e {
            Event::WaitingForDotaToQuit => {
                println!("Dota 正以普通模式运行。录制要带控制台端口（-netconport -insecure）重新启动游戏。");
                println!("请先退出 Dota（主菜单 → 退出游戏），退出后会自动继续；按 Ctrl+C 取消。");
            }
            Event::LaunchingGame { args } => println!("启动游戏：{}", args.join(" ")),
            Event::ReplayLoaded => {}
            Event::Countdown { secs, hotkey } => match hotkey {
                Some(k) => println!("{secs} 秒后开始操作游戏界面，录完之前请不要动鼠标键盘（一动就会停下；也可以按 {k} 或 Ctrl+C）…"),
                None => println!("{secs} 秒后开始操作游戏界面，录完之前请不要动鼠标键盘（要停下就按 Ctrl+C）…"),
            },
            Event::HotkeyUnavailable { hotkey } => println!("取消快捷键 {hotkey} 被别的程序占用了，要停下请按 Ctrl+C 或动一下鼠标"),
            Event::ClipStarted { n, total, index, name } => println!("[{n}/{total}] 片段 {index}：{name}"),
            Event::ExportStarted { .. } => {}
            Event::Exported { from, to, secs, render_secs } => {
                println!("  导出 {}–{}：{secs:.1} 秒，渲染 {render_secs:.0} 秒", clips::mmss(from), clips::mmss(to))
            }
            Event::Retrying { reason } => println!("  第一次导出不对（{reason}），重来一次"),
            Event::ClipDone { path, secs, .. } => println!("  → {}（{secs:.1} 秒）", path.display()),
            Event::CompilationDone { path, secs } => println!("合集 → {}（{secs:.1} 秒）", path.display()),
            Event::RestoringWindowSize { width, height } => println!("让游戏退出，把显示设置里的窗口大小改回 {width}×{height}…"),
            Event::WindowSizeRestored { width, height } => println!("窗口大小已改回 {width}×{height}"),
            Event::WindowSizeNotRestored { width, height, error } => {
                println!("没能改回窗口大小（{error}），请在游戏的视频设置里自己改回 {width}×{height}")
            }
            Event::RestoreFailed { error } => println!("恢复游戏设置时出错：{error}"),
            Event::DiagnosticsKept { dir } => println!("排查用的截图和控制台日志在 {}", dir.display()),
        }
    }

    fn confirm(&self, question: &str) -> bool {
        print!("{question} [y/N] ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).is_ok() && line.trim().eq_ignore_ascii_case("y")
    }
}

pub fn run(r: &Replay, player: usize, selected: &[Selected], p: &Params, ff: &Ffmpeg, o: &Options) -> Result<()> {
    // 第一次 Ctrl+C 只做标记：做完手上这一步就停，照常恢复设置
    game::cancel::install()?;
    let report = pipeline::run(&Console, r, player, selected, p, ff, o);

    let left: Vec<String> =
        selected.iter().map(|s| s.index).filter(|i| !report.completed.contains(i)).map(|i| i.to_string()).collect();
    if let Some(reason) = report.cancelled {
        println!("\n已停止（{}），游戏设置已恢复。", reason.describe());
    }
    if !report.files.is_empty() && (report.cancelled.is_some() || report.error.is_some()) {
        println!("已经做好的 {} 个文件：", report.files.len());
        for f in &report.files {
            println!("  {}", f.display());
        }
    }
    if (report.cancelled.is_some() || report.error.is_some()) && !left.is_empty() {
        println!("没录完的片段可以用 --clips {} 接着录。", left.join(","));
    }
    if let Some(e) = report.error {
        return Err(e);
    }
    if report.cancelled.is_some() {
        bail!("已停止");
    }
    println!("\n完成，用时 {:.1} 分钟，输出 {} 个文件：", report.elapsed.as_secs_f32() / 60.0, report.files.len());
    for f in &report.files {
        println!("  {}", f.display());
    }
    Ok(())
}
