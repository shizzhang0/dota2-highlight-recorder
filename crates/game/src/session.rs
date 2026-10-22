//! 一次录制会话：持有 netcon 连接，改游戏设置前记下原值，结束时（正常结束、出错、提前退出）一定恢复，
//! 并删掉我们的 GSI 配置。
//!
//! 我们改的设置都带 archive 标记，会写进用户的配置文件，所以必须恢复。

use anyhow::Result;

use crate::netcon::Netcon;

pub struct Session {
    con: Netcon,
    /// 改过的设置和原值，按修改顺序
    saved: Vec<(String, String)>,
    /// 结束时要不要删 GSI 配置
    remove_gsi: bool,
}

/// 控制台读出来的布尔值是 true/false，写回时用 1/0；字符串值加引号
fn console_value(v: &str) -> String {
    match v {
        "true" => "1".into(),
        "false" => "0".into(),
        v if v.parse::<f64>().is_ok() => v.into(),
        v if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') => v.into(),
        v => format!("\"{v}\""),
    }
}

impl Session {
    pub fn new(con: Netcon) -> Session {
        Session { con, saved: Vec::new(), remove_gsi: false }
    }

    pub fn con(&mut self) -> &mut Netcon {
        &mut self.con
    }

    /// 结束时删掉 GSI 配置
    pub fn remove_gsi_on_exit(&mut self) {
        self.remove_gsi = true;
    }

    /// 改一个控制台变量；同一个变量只在第一次改时记原值
    pub fn set(&mut self, name: &str, value: &str) -> Result<()> {
        if !self.saved.iter().any(|(n, _)| n == name) {
            let original = self.con.get(name)?;
            self.saved.push((name.to_owned(), original));
        }
        self.con.send(&format!("{name} {}", console_value(value)))
    }

    /// 按相反顺序恢复所有改过的设置，并核对恢复结果。某项失败（比如游戏崩溃、连接断了）也继续做完其余的，
    /// GSI 配置无论如何都删。游戏崩溃时改动还没写进配置文件（游戏退出时才写），不恢复也不会留下
    pub fn restore(&mut self) -> Result<()> {
        let mut failed = Vec::new();
        while let Some((name, original)) = self.saved.pop() {
            let restored = self.con.send(&format!("{name} {}", console_value(&original))).and_then(|()| self.con.get(&name));
            match restored {
                Ok(now) if now == original => {}
                Ok(now) => failed.push(format!("{name}：应为 {original}，实际 {now}")),
                Err(e) => failed.push(format!("{name}：{e:#}")),
            }
        }
        if self.remove_gsi {
            if let Err(e) = crate::gsi::remove() {
                failed.push(format!("GSI 配置：{e:#}"));
            }
            self.remove_gsi = false;
        }
        if failed.is_empty() { Ok(()) } else { anyhow::bail!("这些设置没能恢复：{}", failed.join("；")) }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Err(e) = self.restore() {
            eprintln!("恢复游戏设置时出错：{e:#}");
        }
    }
}
