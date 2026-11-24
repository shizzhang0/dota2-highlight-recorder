//! 片头：成品开头几秒叠在画面左下的一张卡片（2026-11-24 和用户一起定的样式，样片 `d2hr-片头样片\C_字幕条_定稿.mp4`）。
//!
//! 左边英雄立绘，右边四行字：金色小字的视频类型（击杀集锦）、大号英雄名、战绩（只写数字 `12 / 3 / 6`）、
//! 圆形头像 + 玩家名（只在录自己时有，录别人不露名字，同 2026-11-17 的规矩）。底下是从左往右渐隐的黑底。
//! 0.3 秒起从左边滑进来并淡入，停到 3.2 秒，0.5 秒淡出（`Ffmpeg::compile` 里做动画）。
//!
//! 这里先用 ffmpeg 画出一张和视频一样大的透明 PNG（图层），坐标按 1080 高设计，别的分辨率按高度等比缩放。
//! 字用系统自带的微软雅黑（粗体 / 常规），Windows 都有；找不到就不做片头。

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, bail};

use crate::ffmpeg::Ffmpeg;

/// 片头卡片上的内容
#[derive(Debug, Clone)]
pub struct Card {
    /// 视频类型，例如「击杀集锦」「Kill highlights」
    pub kind: String,
    /// 英雄显示名
    pub hero: String,
    /// 战绩，例如 `12 / 3 / 6`
    pub kda: String,
    /// 玩家名和头像（Steam 缓存的 png，可能没有）；None = 不显示这一行
    pub player: Option<(String, Option<PathBuf>)>,
    /// 英雄立绘（透明底 WebP，安装包里的 `img/heroes/renders/<英雄>.webp`）；None = 不放立绘，字往左挪
    pub render: Option<Vec<u8>>,
}

/// 卡片出现、滑入、停住、淡出的时间（秒）
pub const SHOW_AT: f64 = 0.3;
pub const SLIDE: f64 = 0.45;
pub const FADE_IN: f64 = 0.4;
pub const HIDE_AT: f64 = 3.2;
pub const FADE_OUT: f64 = 0.5;
/// 从左边滑进来的距离（1080 高时的像素）
pub const SLIDE_PX: f64 = 90.0;
/// 成品短于这么多秒就不加片头（卡片会盖住大半个视频）
pub const MIN_VIDEO: f64 = 8.0;

const GOLD: &str = "0xf0c35a";
const KDA_COLOR: &str = "0xe6e8ec";
const SHADOW: &str = "shadowcolor=black@0.55:shadowx=2:shadowy=2";
/// 英雄名最宽这么多像素（1080 高时），太长就缩小字号（英文「Keeper of the Light」），最小到 `HERO_MIN`
const HERO_MAX_W: f64 = 440.0;
const HERO_SIZE: f64 = 68.0;
const HERO_MIN: f64 = 44.0;

/// 微软雅黑粗体、常规
fn fonts() -> Option<(PathBuf, PathBuf)> {
    let dir = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts");
    let (bold, regular) = (dir.join("msyhbd.ttc"), dir.join("msyh.ttc"));
    (bold.is_file() && regular.is_file()).then_some((bold, regular))
}

/// 滤镜参数里的路径：反斜杠换成斜杠，冒号转义，放在单引号里
fn filter_path(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\\', "/").replace(':', "\\:").replace('\'', "'\\''"))
}

/// 一段文字：字写进 `dir` 里的文件（drawtext 直接收文字要转义一堆字符），`expansion=none` 免得 `%` 被当成变量
fn text(dir: &Path, name: &str, s: &str) -> Result<String> {
    std::fs::write(dir.join(name), s).context("写片头文字失败")?;
    Ok(format!("textfile={name}:expansion=none"))
}

impl Ffmpeg {
    /// 文字的宽度（像素）：白字画在黑底上，用 cropdetect 量出亮的范围
    fn text_width(&self, dir: &Path, font: &Path, size: u32, file: &str) -> Result<u32> {
        let vf = format!("drawtext=fontfile={}:textfile={file}:expansion=none:x=0:y=20:fontsize={size}:fontcolor=white,cropdetect=limit=24:round=1:reset=0", filter_path(font));
        let canvas = format!("color=black:s={}x{}:d=0.1", (size * 30).min(8000), size * 2);
        let o = self
            .command()
            .args(["-loglevel", "info", "-f", "lavfi", "-i", &canvas, "-vf", &vf, "-frames:v", "2", "-f", "null", "-"])
            .current_dir(dir)
            .stdin(Stdio::null())
            .output()
            .context("运行不了 ffmpeg")?;
        let log = String::from_utf8_lossy(&o.stderr);
        log.lines()
            .rev()
            .find_map(|l| l.split("crop=").nth(1))
            .and_then(|c| c.split(':').next())
            .and_then(|w| w.trim().parse().ok())
            .context("量不出文字宽度")
    }

    /// 画出片头图层：`w`×`h` 的透明 PNG，写到 `dir/intro.png`。`dir` 放中间文件，调用方负责删
    pub fn intro_layer(&self, card: &Card, w: u32, h: u32, dir: &Path) -> Result<PathBuf> {
        let Some((bold, regular)) = fonts() else { bail!("系统里没有微软雅黑（msyh.ttc / msyhbd.ttc）") };
        std::fs::create_dir_all(dir).context("建不了片头的临时文件夹")?;
        let s = h as f64 / 1080.0;
        let px = |v: f64| (v * s).round() as i64;
        // 没有立绘时字往左挪到立绘的位置
        let dx = if card.render.is_some() { 0.0 } else { -262.0 };
        let has_player = card.player.is_some();
        // 黑底的上下边（1080 高时）：没有玩家那一行就短一截
        let (top, bottom) = (490.0, if has_player { 800.0 } else { 752.0 });

        // 英雄名太长就缩小字号
        let hero_file = text(dir, "hero.txt", &card.hero)?;
        let full = px(HERO_SIZE) as u32;
        let width = self.text_width(dir, &bold, full, "hero.txt").unwrap_or(0) as f64;
        let size = if width > HERO_MAX_W * s { (HERO_SIZE * HERO_MAX_W * s / width).max(HERO_MIN) } else { HERO_SIZE };
        // 缩小后底边对齐原来的位置
        let hero_y = 584.0 + (HERO_SIZE - size) * 0.85;

        let mut cmd = self.command();
        cmd.current_dir(dir);
        let mut inputs = 0;
        let mut g = format!(
            "color=c=black@0.0:s={w}x{h}:d=0.04,format=rgba,geq=r=0:g=0:b=0:a='if(between(Y\\,{t}\\,{b})*lt(X\\,{gw})\\,225*pow(1-X/{gw}\\,1.2)*min(1\\,min((Y-{t})/{f}\\,({b}-Y)/{f}))\\,0)'[g0];",
            t = px(top),
            b = px(bottom),
            gw = px(1050.0 + dx),
            f = px(30.0).max(1),
        );
        let mut last = "g0".to_string();
        if let Some(bytes) = &card.render {
            std::fs::write(dir.join("render.webp"), bytes).context("写英雄立绘失败")?;
            cmd.args(["-i", "render.webp"]);
            g += &format!("[{inputs}:v]scale={0}:{0},format=rgba[r];[{last}][r]overlay={1}:{2}:format=auto[g1];", px(350.0), px(-10.0), px(448.0));
            inputs += 1;
            last = "g1".into();
        }
        if let Some((_, Some(avatar))) = &card.player
            && std::fs::copy(avatar, dir.join("avatar.png")).is_ok()
        {
            cmd.args(["-i", "avatar.png"]);
            let d = px(44.0);
            let r = d as f64 / 2.0;
            g += &format!(
                "[{inputs}:v]scale={d}:{d},format=rgba,geq=r='r(X\\,Y)':g='g(X\\,Y)':b='b(X\\,Y)':a='if(lte(hypot(X-{c}\\,Y-{c})\\,{r})\\,255\\,0)'[av];[{last}][av]overlay={x}:{y}:format=auto[g2];",
                c = r - 0.5,
                x = px(324.0 + dx),
                y = px(716.0),
            );
            last = "g2".into();
        }
        let draw = |file: String, font: &Path, x: f64, y: f64, size: f64, color: &str| {
            format!("drawtext=fontfile={}:{file}:x={}:y={}:fontsize={}:fontcolor={color}:{SHADOW}", filter_path(font), px(x + dx), px(y), px(size))
        };
        let mut texts = vec![
            draw(text(dir, "kind.txt", &card.kind)?, &bold, 326.0, 542.0, 28.0, GOLD),
            draw(hero_file, &bold, 322.0, hero_y, size, "white"),
            draw(text(dir, "kda.txt", &card.kda)?, &bold, 326.0, 670.0, 28.0, KDA_COLOR),
        ];
        if let Some((name, _)) = &card.player {
            texts.push(draw(text(dir, "player.txt", name)?, &regular, 380.0, 720.0, 26.0, "white"));
        }
        g += &format!("[{last}]{}", texts.join(","));
        let out = dir.join("intro.png");
        let o = cmd
            .args(["-filter_complex", &g, "-frames:v", "1", "intro.png"])
            .stdin(Stdio::null())
            .output()
            .context("运行不了 ffmpeg")?;
        if !o.status.success() {
            bail!("ffmpeg 画片头出错：{}", String::from_utf8_lossy(&o.stderr).trim());
        }
        Ok(out)
    }
}

/// 片头动画的滤镜：图层输入 `input` 淡入淡出、从左边滑进来，叠在 `[main]` 上，输出 `[out]`。`h` 是视频高度
pub(crate) fn overlay_graph(input: usize, main: &str, out: &str, h: u32) -> String {
    let slide = (SLIDE_PX * h as f64 / 1080.0).round();
    format!(
        "[{input}:v]format=rgba,fade=t=in:st={SHOW_AT}:d={FADE_IN}:alpha=1,fade=t=out:st={HIDE_AT}:d={FADE_OUT}:alpha=1[ovl];\
         [{main}][ovl]overlay=x='-{slide}*pow(1-min(1\\,max(0\\,(t-{SHOW_AT})/{SLIDE}))\\,3)':y=0:eof_action=pass:format=auto[{out}];"
    )
}

/// 图层输入要循环多久（秒）：淡出完再多一点
pub(crate) fn layer_secs() -> f64 {
    HIDE_AT + FADE_OUT + 0.1
}
