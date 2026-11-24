#!/usr/bin/env python3
"""把全部英雄的图片下载下来、转成 WebP，放进 ui/img/heroes/，跟着安装包走（2026-11-24 定：不在运行时下载）。

    python tools/hero_images.py           # 只补缺的
    python tools/hero_images.py --all     # 全部重新下载

出新英雄时，发版前跑一次，把新文件提交进仓库。英雄列表读本机游戏文件（要装 Dota 2），
图片取自 Valve 自己的图片服务器，只用英雄名请求。要 ffmpeg：环境变量 D2HR_FFMPEG，没有就用 dist/ffmpeg/。

三种图（括号里是原图）：
  <英雄>.webp          大图（256×144），首页卡片、比赛页
  icons/<英雄>.webp    小图标（32×32），时间线、被杀英雄
  renders/<英雄>.webp  立绘（1440×1440 透明底，缩到 350×350），片头用
"""
import os, subprocess, sys, tempfile, urllib.error, urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "ui" / "img" / "heroes"
CDN = "https://cdn.cloudflare.steamstatic.com/apps/dota2"
# (输出子目录, 地址, ffmpeg 参数)
KINDS = [
    ("", CDN + "/images/dota_react/heroes/{id}.png", ["-c:v", "libwebp", "-quality", "85"]),
    ("icons", CDN + "/images/dota_react/heroes/icons/{id}.png", ["-c:v", "libwebp", "-lossless", "1"]),
    ("renders", CDN + "/videos/dota_react/heroes/renders/{id}.png", ["-vf", "scale=350:350", "-c:v", "libwebp", "-quality", "85"]),
]


def ffmpeg():
    p = Path(os.environ.get("D2HR_FFMPEG") or ROOT / "dist" / "ffmpeg")
    exe = p / "ffmpeg.exe" if p.is_dir() else p
    if not exe.is_file():
        sys.exit(f"找不到 ffmpeg：{exe}。先 python tools/release.py ffmpeg，或设 D2HR_FFMPEG")
    return exe


def hero_ids():
    out = subprocess.run(["cargo", "run", "-q", "-p", "game", "--example", "hero_ids"], cwd=ROOT,
                         capture_output=True, text=True, encoding="utf-8", check=True).stdout
    return [l.strip() for l in out.splitlines() if l.strip()]


def fetch(url):
    """下载一张图；404 返回 None（游戏文件里有些不是正常英雄，比如活动里的机关单位，服务器上没图）"""
    try:
        with urllib.request.urlopen(url, timeout=60) as r:
            return r.read()
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return None
        raise


def main():
    redo = "--all" in sys.argv[1:]
    exe = ffmpeg()
    ids = hero_ids()
    made, missing = 0, []
    with tempfile.TemporaryDirectory() as tmp:
        src = Path(tmp) / "src.png"
        for hero in ids:
            for sub, url, args in KINDS:
                dest = OUT / sub / f"{hero}.webp"
                if dest.exists() and not redo:
                    continue
                data = fetch(url.format(id=hero))
                if data is None:
                    missing.append(f"{sub}/{hero}" if sub else hero)
                    continue
                src.write_bytes(data)
                dest.parent.mkdir(parents=True, exist_ok=True)
                subprocess.run([str(exe), "-hide_banner", "-loglevel", "error", "-y", "-i", str(src), *args, str(dest)], check=True)
                made += 1
    total = sum(f.stat().st_size for f in OUT.rglob("*.webp"))
    print(f"{len(ids)} 个英雄，新做 {made} 张；ui/img/heroes 共 {len(list(OUT.rglob('*.webp')))} 张，{total / 1048576:.1f} MB")
    if missing:
        print("服务器上没有（跳过）：" + "、".join(missing))


if __name__ == "__main__":
    main()
