//! 边录边编码：游戏用 `startmovie` 一帧一帧写出图片，我们把每帧的像素直接喂给 ffmpeg 的标准输入编码成 H.264，
//! 编完一帧就可以删掉那张图，临时空间只占几帧。声音是游戏录完才写好的 WAV，最后再合进去。
//!
//! 编码器优先用显卡（NVIDIA NVENC、AMD AMF、Intel QSV），都不能用时退回 CPU（x264）。
//! 2026-11-06 实测（RTX 5070 Ti Laptop）：NVENC 编 2560×1600 每秒 118 帧、1080p 每秒 233 帧，远快于游戏出帧。

use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::thread::JoinHandle;

use anyhow::{Context, Result, bail};

use crate::ffmpeg::Ffmpeg;

/// H.264 编码器
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Nvenc,
    Amf,
    Qsv,
    X264,
}

impl Codec {
    /// 按优先顺序
    const ALL: [Codec; 4] = [Codec::Nvenc, Codec::Amf, Codec::Qsv, Codec::X264];

    pub fn name(self) -> &'static str {
        match self {
            Codec::Nvenc => "h264_nvenc",
            Codec::Amf => "h264_amf",
            Codec::Qsv => "h264_qsv",
            Codec::X264 => "libx264",
        }
    }

    /// 画质参数：按质量定码率（画面复杂时码率高），再封个顶。数值是起点，实测画质后再调
    fn quality_args(self) -> &'static [&'static str] {
        match self {
            Codec::Nvenc => &["-preset", "p5", "-tune", "hq", "-rc", "vbr", "-cq", "21", "-b:v", "0", "-maxrate", "25M", "-bufsize", "50M"],
            Codec::Amf => &["-quality", "quality", "-rc", "qvbr", "-qvbr_quality_level", "23", "-maxrate", "25M"],
            Codec::Qsv => &["-preset", "slower", "-global_quality", "23", "-maxrate", "25M"],
            Codec::X264 => &["-preset", "veryfast", "-crf", "20", "-maxrate", "25M", "-bufsize", "50M"],
        }
    }
}

const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

/// 喂给编码器的画面
#[derive(Debug, Clone, Copy)]
pub struct Format {
    pub width: u32,
    pub height: u32,
    /// 每秒帧数
    pub fps: u32,
    /// 图片是从下往上存的（TGA 默认如此），编码时上下翻转
    pub bottom_up: bool,
}

/// 编码进程怎么跑（2026-11-07 第一次完整实测：边录边编码时游戏出帧从每秒 86 帧掉到 37 帧，用这两项对比找原因）
#[derive(Debug, Clone, Copy, Default)]
pub struct EncodeHow {
    /// 在 CPU 上把 BGRA 转成 YUV 再交给编码器（不设时 NVENC 在显卡上转）
    pub cpu_convert: bool,
    /// 编码进程用"低于正常"的优先级
    pub low_priority: bool,
}

impl Ffmpeg {
    /// 第一个能用的 H.264 编码器：编一帧小画面试试（显卡编码器在 ffmpeg 里列着，不代表这台机器的显卡支持）
    pub fn pick_codec(&self) -> Codec {
        for codec in Codec::ALL {
            let ok = self
                .command()
                .args(["-f", "lavfi", "-i", "color=black:s=256x256:r=60", "-frames:v", "2", "-c:v", codec.name(), "-f", "null", "-"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if ok {
                return codec;
            }
        }
        Codec::X264
    }

    /// 开一个编码进程：从标准输入收 `f` 描述的 BGRA 像素，编成 H.264 写到 `out`（没有声音）
    pub fn encoder(&self, codec: Codec, f: Format, out: &Path, how: EncodeHow) -> Result<Encoder> {
        let size = format!("{}x{}", f.width, f.height);
        let rate = f.fps.to_string();
        let gop = f.fps.to_string();
        let mut cmd = self.command();
        if how.low_priority {
            // 游戏在同一台机器上逐帧渲染，编码别跟它抢 CPU
            cmd.creation_flags(crate::ffmpeg::CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
        }
        cmd.args(["-f", "rawvideo", "-pix_fmt", "bgra", "-s", &size, "-framerate", &rate, "-i", "-"]);
        if f.bottom_up {
            // 翻转只是倒着读每行，不拷贝像素
            cmd.args(["-vf", "vflip"]);
        }
        // 每秒一个关键帧：之后按秒裁剪、拼接都能直接流复制
        cmd.args(["-c:v", codec.name()]).args(codec.quality_args()).args(["-g", &gop, "-an"]);
        // NVENC 能直接收 BGRA、在显卡上转成 YUV；不指定输出像素格式就不会先在 CPU 上转一遍
        if how.cpu_convert || codec != Codec::Nvenc {
            cmd.args(["-pix_fmt", "yuv420p"]);
        }
        cmd.arg(out).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
        let mut child = cmd.spawn().context("启动不了 ffmpeg 编码")?;
        let stdin = child.stdin.take().context("拿不到 ffmpeg 的标准输入")?;
        // 另起线程收 stderr，免得管道写满把 ffmpeg 卡住
        let mut stderr = child.stderr.take().context("拿不到 ffmpeg 的错误输出")?;
        let errors = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        Ok(Encoder { child, stdin: Some(stdin), errors: Some(errors), frame_bytes: f.width as usize * f.height as usize * 4, frames: 0 })
    }

    /// 把 `video`（只有画面）和 `audio`（WAV）合成 `out`：画面原样复制，声音编成 AAC。
    /// `audio_skip`：声音开头跳过这么多秒（画面开头丢掉的那段，声音也要去掉）
    pub fn mux_audio(&self, video: &Path, audio: &Path, audio_skip: f32, out: &Path) -> Result<()> {
        let (v, a, o) = (video.to_string_lossy(), audio.to_string_lossy(), out.to_string_lossy());
        let skip = format!("{audio_skip:.3}");
        self.run(&[
            "-i", &v, "-ss", &skip, "-i", &a, "-map", "0:v", "-map", "1:a", "-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-shortest", "-movflags",
            "+faststart", &o,
        ])
    }

    /// WAV 的平均音量（dB）；全静音约 -91
    pub fn mean_volume(&self, audio: &Path) -> Result<f32> {
        let out = self
            .command()
            .args(["-loglevel", "info", "-i"])
            .arg(audio)
            .args(["-af", "volumedetect", "-f", "null", "-"])
            .stdin(Stdio::null())
            .output()
            .context("运行不了 ffmpeg")?;
        let text = String::from_utf8_lossy(&out.stderr);
        text.lines()
            .find_map(|l| l.split("mean_volume:").nth(1))
            .and_then(|v| v.trim().trim_end_matches("dB").trim().parse().ok())
            .context("ffmpeg 没给出音量")
    }
}

/// 正在编码的 ffmpeg 进程
pub struct Encoder {
    child: Child,
    stdin: Option<ChildStdin>,
    errors: Option<JoinHandle<String>>,
    frame_bytes: usize,
    frames: u64,
}

impl Encoder {
    /// 喂一帧 BGRA 像素。ffmpeg 编得慢时这里会等（管道满了）
    pub fn write(&mut self, bgra: &[u8]) -> Result<()> {
        if bgra.len() != self.frame_bytes {
            bail!("这一帧 {} 字节，应该是 {} 字节（画面大小变了？）", bgra.len(), self.frame_bytes);
        }
        let stdin = self.stdin.as_mut().context("编码已经结束")?;
        if let Err(e) = stdin.write_all(bgra) {
            // 多半是 ffmpeg 自己出错退出了，把它的错误信息带上
            let _ = self.stdin.take();
            let _ = self.child.wait();
            let msg = self.errors.take().and_then(|h| h.join().ok()).unwrap_or_default();
            bail!("往 ffmpeg 写画面失败（{e}）：{}", msg.trim());
        }
        self.frames += 1;
        Ok(())
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// 关掉输入、等 ffmpeg 写完文件，返回编了几帧
    pub fn finish(mut self) -> Result<u64> {
        drop(self.stdin.take());
        let status = self.child.wait().context("等 ffmpeg 结束失败")?;
        let msg = self.errors.take().and_then(|h| h.join().ok()).unwrap_or_default();
        if !status.success() {
            bail!("ffmpeg 编码出错：{}", msg.trim());
        }
        Ok(self.frames)
    }
}

impl Drop for Encoder {
    /// 出错提前放弃时别留下 ffmpeg 进程
    fn drop(&mut self) {
        if self.stdin.take().is_some() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
