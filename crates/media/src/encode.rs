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

    /// 把录好的片段做成成品（集锦、单个时刻）：片段之间交叉溶解，开头从黑场淡入、结尾淡出到黑场，声音跟着画面。
    /// 要重新编码一遍（流复制做不了过渡），编码器和画质跟录制时一样。2026-11-23 定：照完美时刻的做法，
    /// 他们的集锦片段之间约 0.3 秒交叉溶解、首尾约 0.5 秒黑场（逐帧看过一条他们的击杀集锦）
    /// `inputs`：每段的文件，和只取它的哪一截（文件里从第几秒到第几秒，None = 整段）
    pub fn compile(&self, codec: Codec, inputs: &[(std::path::PathBuf, Option<(f32, f32)>)], out: &Path) -> Result<()> {
        let mut cuts = Vec::new();
        for (p, range) in inputs {
            // 画面和声音轨长短差几十毫秒，截得比总时长短一点，按同一个长度截，过渡的起点才算得准、声画不错位
            let end = crate::mp4_duration(p)? - 0.05;
            let (a, b) = range.map_or((0.0, end), |(a, b)| (a as f64, (b as f64).min(end)));
            cuts.push((a.max(0.0), (b - a).max(0.1)));
        }
        let graph = compile_graph(&cuts);
        let mut cmd = self.command();
        cmd.creation_flags(crate::ffmpeg::CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
        for (p, _) in inputs {
            cmd.arg("-i").arg(p);
        }
        cmd.args(["-filter_complex", &graph, "-map", "[v]", "-map", "[a]", "-c:v", codec.name()])
            .args(codec.quality_args())
            .args(["-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart"])
            .arg(out)
            .stdin(Stdio::null());
        let o = cmd.output().context("运行不了 ffmpeg")?;
        if !o.status.success() {
            bail!("ffmpeg 生成视频出错：{}", String::from_utf8_lossy(&o.stderr).trim());
        }
        Ok(())
    }
}

/// 片段之间交叉溶解的秒数
pub const CROSSFADE: f64 = 0.3;
/// 开头淡入、结尾淡出的秒数
pub const FADE: f64 = 0.5;

/// `compile` 的滤镜图。`cuts`：每段从文件第几秒起、取多少秒。
/// 第 i 次溶解从前面已拼好的长度减去一次溶解的地方开始
fn compile_graph(cuts: &[(f64, f64)]) -> String {
    let lens: Vec<f64> = cuts.iter().map(|c| c.1).collect();
    // 片段都有好几秒；万一有很短的，溶解不超过它的三分之一
    let x = lens.iter().copied().fold(CROSSFADE, |m, l| m.min(l / 3.0));
    let mut g = String::new();
    for (i, (from, l)) in cuts.iter().enumerate() {
        let to = from + l;
        g += &format!("[{i}:v]trim=start={from:.3}:end={to:.3},setpts=PTS-STARTPTS,settb=AVTB,format=yuv420p[v{i}];");
        g += &format!("[{i}:a]atrim=start={from:.3}:end={to:.3},asetpts=PTS-STARTPTS,aformat=sample_fmts=fltp:channel_layouts=stereo[a{i}];");
    }
    let (mut v, mut a) = ("v0".to_string(), "a0".to_string());
    let mut total = lens[0];
    for (i, l) in lens.iter().enumerate().skip(1) {
        let offset = total - x;
        g += &format!("[{v}][v{i}]xfade=transition=fade:duration={x:.3}:offset={offset:.3}[xv{i}];");
        g += &format!("[{a}][a{i}]acrossfade=d={x:.3}[xa{i}];");
        (v, a) = (format!("xv{i}"), format!("xa{i}"));
        total += l - x;
    }
    let f = FADE.min(total / 4.0);
    let out_at = total - f;
    g += &format!("[{v}]fade=t=in:st=0:d={f:.3},fade=t=out:st={out_at:.3}:d={f:.3}[v];");
    g += &format!("[{a}]afade=t=in:st=0:d={f:.3},afade=t=out:st={out_at:.3}:d={f:.3}[a]");
    g
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_offsets() {
        // 9.95 + 7.95 + 5.95 秒：溶解在 9.65、17.3 处开始，总长 23.25，淡出从 22.75 开始
        let g = compile_graph(&[(0.0, 9.95), (0.0, 7.95), (2.0, 5.95)]);
        assert!(g.contains("[2:v]trim=start=2.000:end=7.950,"), "{g}");
        assert!(g.contains("[v0][v1]xfade=transition=fade:duration=0.300:offset=9.650[xv1]"), "{g}");
        assert!(g.contains("[xv1][v2]xfade=transition=fade:duration=0.300:offset=17.300[xv2]"), "{g}");
        assert!(g.contains("[xa1][a2]acrossfade=d=0.300[xa2]"), "{g}");
        assert!(g.ends_with("[xa2]afade=t=in:st=0:d=0.500,afade=t=out:st=22.750:d=0.500[a]"), "{g}");
    }

    #[test]
    fn graph_one_part() {
        // 只有一段：没有溶解，只有首尾淡入淡出
        let g = compile_graph(&[(0.0, 12.0)]);
        assert!(!g.contains("xfade"), "{g}");
        assert!(g.contains("[v0]fade=t=in:st=0:d=0.500,fade=t=out:st=11.500:d=0.500[v]"), "{g}");
    }
}
