//! 调用 ffmpeg 裁剪、拼接导出的视频，全部流复制，不重新编码。
//!
//! 剪辑工具导出的 mp4 是 H.264 + AAC，每 1 秒一个关键帧（第一个在 1/60 秒处，之后每整秒一个，
//! 2026-10-22 用 ffprobe 看的），所以按整秒裁掉两头多留的部分可以直接切在关键帧上。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

pub struct Ffmpeg {
    exe: PathBuf,
}

impl Ffmpeg {
    /// 找 ffmpeg：先用给定路径（文件或所在目录），再看环境变量 `D2HR_FFMPEG`，最后在 PATH 里找
    pub fn locate(given: Option<&Path>) -> Result<Ffmpeg> {
        let from_env = std::env::var_os("D2HR_FFMPEG").map(PathBuf::from);
        if let Some(p) = given.map(Path::to_path_buf).or(from_env) {
            let exe = if p.is_dir() { p.join("ffmpeg.exe") } else { p.clone() };
            if exe.is_file() {
                return Ok(Ffmpeg { exe });
            }
            bail!("找不到 ffmpeg：{}", p.display());
        }
        let in_path = std::env::var_os("PATH")
            .into_iter()
            .flat_map(|v| std::env::split_paths(&v).collect::<Vec<_>>())
            .map(|d| d.join("ffmpeg.exe"))
            .find(|p| p.is_file());
        match in_path {
            Some(exe) => Ok(Ffmpeg { exe }),
            None => bail!("找不到 ffmpeg。用 --ffmpeg 指定 ffmpeg.exe 或它所在的目录，或者设环境变量 D2HR_FFMPEG"),
        }
    }

    fn run(&self, args: &[&str]) -> Result<()> {
        let out = Command::new(&self.exe)
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("运行不了 {}", self.exe.display()))?;
        if !out.status.success() {
            bail!("ffmpeg 出错：{}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }

    /// 从 `input` 的第 `start` 秒起取 `len` 秒写到 `out`。起点会落在 `start` 之前最近的关键帧上
    pub fn cut(&self, input: &Path, start: f64, len: f64, out: &Path) -> Result<()> {
        let (start, len) = (format!("{start:.3}"), format!("{len:.3}"));
        let (input, out) = (input.to_string_lossy(), out.to_string_lossy());
        self.run(&["-ss", &start, "-i", &input, "-t", &len, "-c", "copy", "-avoid_negative_ts", "make_zero", "-movflags", "+faststart", &out])
    }

    /// 按顺序首尾相接拼成一个文件。各段编码参数必须一样（都是剪辑工具用同一套设置导出的）
    pub fn concat(&self, inputs: &[PathBuf], out: &Path) -> Result<()> {
        if inputs.len() == 1 {
            std::fs::copy(&inputs[0], out)?;
            return Ok(());
        }
        // concat 分离器的列表文件；路径里的单引号要转义
        let list = out.with_extension("txt");
        let text: String = inputs
            .iter()
            .map(|p| format!("file '{}'\n", p.to_string_lossy().replace('\\', "/").replace('\'', r"'\''")))
            .collect();
        std::fs::write(&list, text)?;
        let (l, o) = (list.to_string_lossy(), out.to_string_lossy());
        let result = self.run(&["-f", "concat", "-safe", "0", "-i", &l, "-c", "copy", "-movflags", "+faststart", &o]);
        let _ = std::fs::remove_file(&list);
        result
    }
}
