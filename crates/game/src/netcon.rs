//! 游戏控制台的 TCP 接口。
//!
//! 游戏带 `-netconport <端口>` 启动后会在本机监听该端口：连上后收到的是控制台输出（逐行文本），
//! 发过去的每一行都当作一条控制台命令执行。

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

pub struct Netcon {
    stream: TcpStream,
    lines: Receiver<String>,
}

impl Netcon {
    pub fn connect(port: u16) -> Result<Self> {
        Self::connect_inner(port, None)
    }

    /// 同 `connect`，另把收到的每行控制台输出（带距连接时的秒数）追加写进 `log`，供排查
    pub fn connect_logged(port: u16, log: &Path) -> Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(log).with_context(|| format!("打不开 {}", log.display()))?;
        Self::connect_inner(port, Some(file))
    }

    fn connect_inner(port: u16, mut log: Option<File>) -> Result<Self> {
        let stream = TcpStream::connect(("127.0.0.1", port))
            .with_context(|| format!("连不上游戏控制台 127.0.0.1:{port}，游戏是否带 -netconport {port} 启动？"))?;
        let reader = BufReader::new(stream.try_clone()?);
        let (tx, lines) = mpsc::channel();
        let started = Instant::now();
        std::thread::spawn(move || {
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if let Some(f) = log.as_mut() {
                    let _ = writeln!(f, "{:>8.2} {line}", started.elapsed().as_secs_f32());
                }
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Self { stream, lines })
    }

    pub fn send(&mut self, command: &str) -> Result<()> {
        self.stream.write_all(command.as_bytes())?;
        self.stream.write_all(b"\n")?;
        Ok(())
    }

    /// 等到控制台真正能执行命令。游戏刚启动时端口很早就开了，但命令要等加载到一定程度才有回应
    pub fn wait_responsive(&mut self, timeout: Duration) -> Result<()> {
        let started = Instant::now();
        while started.elapsed() < timeout {
            crate::cancel::check()?;
            self.send("echo d2hr_ready")?;
            if self.wait_for(Duration::from_secs(2), |l| l.trim() == "d2hr_ready").is_some() {
                return Ok(());
            }
        }
        anyhow::bail!("等了 {timeout:?} 游戏控制台仍无回应")
    }

    /// 读控制台变量的当前值（输出形如 `snd_mute_losefocus = true`）
    pub fn get(&mut self, name: &str) -> Result<String> {
        self.drain(Duration::from_millis(100), Duration::from_millis(500));
        self.send(name)?;
        // 值为空时那一行是 `name = `，末尾空格可能被去掉，所以只认 `name =`
        let prefix = format!("{name} =");
        self.wait_for(Duration::from_secs(3), |l| l.trim_start().starts_with(&prefix))
            .map(|l| l.trim_start()[prefix.len()..].trim().to_owned())
            .with_context(|| format!("读不到控制台变量 {name}"))
    }

    /// 跳到录像的某个 tick 并等它完成（控制台输出 `Demo Skipping finished at tick N`）。
    /// 回放正暂停在目标 tick 时，游戏应该不会输出完成标志（2026-10-22：暂停在 62146 时再跳到 62146，
    /// 等满 60 秒都没有；是推断，下次测试用控制台日志确认），调用方要保证目标和当前位置不同
    pub fn seek(&mut self, tick: u32, timeout: Duration) -> Result<()> {
        self.drain(Duration::from_millis(100), Duration::from_millis(500));
        self.send(&format!("demo_gototick {tick}"))?;
        let done = format!("Demo Skipping finished at tick {tick}");
        self.wait_for(timeout, |l| l.contains(&done)).map(drop).with_context(|| format!("跳到 tick {tick} 没有完成"))
    }

    /// 收集 `quiet` 时间内没有新输出为止（最多 `max`）的所有输出行
    pub fn drain(&self, quiet: Duration, max: Duration) -> Vec<String> {
        let started = Instant::now();
        let mut out = Vec::new();
        while started.elapsed() < max {
            match self.lines.recv_timeout(quiet) {
                Ok(line) => out.push(line),
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
            }
        }
        out
    }

    /// 等到某行输出满足条件，返回该行；超时、连接断开或用户按了 Ctrl+C 返回 None
    pub fn wait_for(&self, timeout: Duration, mut pred: impl FnMut(&str) -> bool) -> Option<String> {
        let started = Instant::now();
        while let Some(left) = timeout.checked_sub(started.elapsed()) {
            if crate::cancel::requested() {
                return None;
            }
            // 分小段等，好及时响应 Ctrl+C
            match self.lines.recv_timeout(left.min(Duration::from_millis(300))) {
                Ok(line) if pred(&line) => return Some(line),
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
        None
    }
}
