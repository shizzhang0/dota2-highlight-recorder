//! 找到 Dota 2 安装目录，直接启动 dota2.exe（不经过 Steam 的 -applaunch，不弹确认框）。

use std::net::TcpStream;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::steam::steam_dir;

const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Steam 可能有多个库，Dota 不一定装在主库里
fn libraries(steam: &Path) -> Vec<PathBuf> {
    let mut out = vec![steam.to_path_buf()];
    if let Ok(s) = std::fs::read_to_string(steam.join(r"steamapps\libraryfolders.vdf")) {
        for line in s.lines().map(str::trim).filter(|l| l.starts_with("\"path\"")) {
            if let Some(v) = line.split('"').nth(3) {
                out.push(PathBuf::from(v.replace("\\\\", "\\")));
            }
        }
    }
    out
}

/// `.../steamapps/common/dota 2 beta/game`
pub fn game_dir() -> Result<PathBuf> {
    for lib in libraries(&steam_dir()?) {
        let dir = lib.join(r"steamapps\common\dota 2 beta\game");
        if dir.join(r"bin\win64\dota2.exe").is_file() {
            return Ok(dir);
        }
    }
    bail!("没找到 Dota 2 安装目录")
}

/// 游戏客户端下载的录像所在目录
pub fn replays_dir() -> Result<PathBuf> {
    Ok(game_dir()?.join(r"dota\replays"))
}

/// 本机 netcon 端口是否已在监听（游戏是否以 -netconport -insecure 运行）
pub fn netcon_ready(port: u16) -> bool {
    TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(300)).is_ok()
}

/// 启动游戏并开放 netcon。`-insecure` 必须带：不带时游戏不开 netcon 端口，带了 VAC 不加载、进不了匹配。
/// `extra` 放用户自己的启动参数（如国服的 `-perfectworld`）。要求 Steam 已登录
pub fn launch(port: u16, extra: &[String]) -> Result<Child> {
    let game = game_dir()?;
    let bin = game.join(r"bin\win64");
    let port = port.to_string();
    let mut args = vec!["-netconport", &port, "-insecure", "-novid", "-console"];
    args.extend(extra.iter().map(String::as_str));
    dont_inherit_std_handles();
    Command::new(bin.join("dota2.exe"))
        .args(&args)
        .current_dir(&bin)
        // 不经过 Steam 启动时，不设 SteamAppId 游戏会不知道自己是谁
        .env("SteamAppId", "570")
        .env("SteamGameId", "570")
        // 不挂在我们的控制台上：否则用户按 Ctrl+C 时游戏也会收到信号被关掉
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        // 游戏会一直开着，不能让它继承我们的输出管道，否则调用方会等它退出才算结束
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("启动 dota2.exe 失败")
}

/// 子进程会继承所有可继承的句柄，包括我们自己的标准输出（调用方给的管道），即使给子进程设了 Stdio::null。
/// 游戏继承了管道，调用方就要等游戏退出才读到管道结束（2026-10-22 测试时命令一直不返回）
fn dont_inherit_std_handles() {
    use windows::Win32::Foundation::{HANDLE_FLAG_INHERIT, HANDLE_FLAGS, SetHandleInformation};
    use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    for std in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        unsafe {
            if let Ok(h) = GetStdHandle(std)
                && !h.is_invalid()
            {
                let _ = SetHandleInformation(h, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0));
            }
        }
    }
}

/// 等 netcon 端口开始监听
pub fn wait_netcon(port: u16, timeout: Duration) -> Result<()> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        crate::cancel::check()?;
        if netcon_ready(port) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    bail!("等了 {timeout:?} netcon 端口 {port} 仍未开放")
}
