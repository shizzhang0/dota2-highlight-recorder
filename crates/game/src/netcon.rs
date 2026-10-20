//! 游戏控制台的 TCP 接口。
//!
//! 游戏带 `-netconport <端口>` 启动后会在本机监听该端口：连上后收到的是控制台输出（逐行文本），
//! 发过去的每一行都当作一条控制台命令执行。

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

pub struct Netcon {
    stream: TcpStream,
    lines: Receiver<String>,
}

impl Netcon {
    pub fn connect(port: u16) -> Result<Self> {
        let stream = TcpStream::connect(("127.0.0.1", port))
            .with_context(|| format!("连不上游戏控制台 127.0.0.1:{port}，游戏是否带 -netconport {port} 启动？"))?;
        let reader = BufReader::new(stream.try_clone()?);
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in reader.lines() {
                let Ok(line) = line else { break };
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
            self.send("echo d2hr_ready")?;
            if self.wait_for(Duration::from_secs(2), |l| l.trim() == "d2hr_ready").is_some() {
                return Ok(());
            }
        }
        anyhow::bail!("等了 {timeout:?} 游戏控制台仍无回应")
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

    /// 等到某行输出满足条件，返回该行；超时返回 None
    pub fn wait_for(&self, timeout: Duration, mut pred: impl FnMut(&str) -> bool) -> Option<String> {
        let started = Instant::now();
        while let Some(left) = timeout.checked_sub(started.elapsed()) {
            match self.lines.recv_timeout(left) {
                Ok(line) if pred(&line) => return Some(line),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        None
    }
}
