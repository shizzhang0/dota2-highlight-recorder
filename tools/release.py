#!/usr/bin/env python3
"""发版脚本，从 dota2-game-helper2 改过来。本地和 GitHub Actions（.github/workflows/release.yml）用同一个。

    python tools/release.py bump 0.2.0      # 改三处版本号（之后照常提 PR 合进 main）
    python tools/release.py ffmpeg          # 只下载、核对 ffmpeg，放到 dist/ffmpeg/（build 会自动做）
    python tools/release.py build           # 打包：安装包、latest.json（文件名不带版本号）→ dist/release/
    python tools/release.py publish NOTES   # 用 build 的产物建**草稿** Release、传文件，NOTES 是正文文件
    python tools/release.py verify 0.2.0    # 把 Release 上的文件下载回来核对 SHA256，全对才把草稿转正

build 要签名私钥（一键更新只认签过名的安装包）：环境变量 TAURI_SIGNING_PRIVATE_KEY（私钥内容）；
没设的话读本机 ~/.tauri/dota2-highlight-recorder.key。私钥没设密码。
"""
import datetime, hashlib, json, os, re, shutil, subprocess, sys, tempfile, urllib.request, zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TAURI = ROOT / "src-tauri"
OUT = ROOT / "dist" / "release"
REPO = "shizzhang0/dota2-highlight-recorder"
# Tauri 命令行版本跟着 Cargo.lock 里的 tauri 走（命令行和库的版本号不完全一一对应，取同一时期的）
TAURI_CLI = "@tauri-apps/cli@2.12.4"
KEY_FILE = Path.home() / ".tauri" / "dota2-highlight-recorder.key"
# 文件名不带版本号：README 的下载链接指向 releases/latest/download/<这个名字>，永远是最新版（同 helper2）
STABLE_SETUP = "dota2-highlight-recorder-setup.exe"

# 安装包附带的 ffmpeg：BtbN 的 GPL shared 构建，取月底存档（每天的构建只留两周左右，月底的长期保留）。
# 换版本时三项一起改，SHA256 从 GitHub Release 页面的 digest 抄，下载后核对
FFMPEG_TAG = "autobuild-2026-10-31-12-59"
FFMPEG_ZIP = "ffmpeg-n9.0.2-39-g7adfbe40dc-win64-gpl-shared-9.0.zip"
FFMPEG_SHA256 = "1D9823C35C7A7E2F54D1438CF61030D6EFABE3278DE01C191309D1E828BB74DF"
FFMPEG_URL = f"https://github.com/BtbN/FFmpeg-Builds/releases/download/{FFMPEG_TAG}/{FFMPEG_ZIP}"
FFMPEG_DIR = ROOT / "dist" / "ffmpeg"
CACHE = ROOT / "dist" / "cache"
FFMPEG_NOTE = f"""This folder contains FFmpeg, used by dota2-highlight-recorder to encode the recorded videos.
It is a separate program, run as ffmpeg.exe; it is not linked into dota2-highlight-recorder.

Version: n9.0.2-39-g7adfbe40dc (release/9.0 branch), GPL shared build by BtbN
  Binary:        https://github.com/BtbN/FFmpeg-Builds/releases/tag/{FFMPEG_TAG} ({FFMPEG_ZIP})
  Source code:   https://github.com/FFmpeg/FFmpeg/tree/7adfbe40dc
  Build scripts: https://github.com/BtbN/FFmpeg-Builds
License: GNU General Public License version 3, see LICENSE.txt in this folder.
Only ffmpeg.exe and the DLLs it needs are included (ffplay and ffprobe are left out).

本文件夹是 FFmpeg（录制时用来编码视频），是独立的程序，许可证见 LICENSE.txt（GPLv3），源码地址见上。
"""

# Windows 上（包括 GitHub 的 windows runner）stdout 默认不是 UTF-8，打印中文会直接抛错
for _s in (sys.stdout, sys.stderr):
    if hasattr(_s, "reconfigure"):
        _s.reconfigure(encoding="utf-8")


def version():
    return json.loads((TAURI / "tauri.conf.json").read_text(encoding="utf-8"))["version"]


def run(cmd, **kw):
    print("+", " ".join(cmd) if isinstance(cmd, list) else cmd, flush=True)
    subprocess.run(cmd, check=True, cwd=ROOT, **kw)


def sha256(p):
    return hashlib.sha256(p.read_bytes()).hexdigest().upper()


def sub_one(path, pattern, repl):
    """只替换一处，替换不到就报错——宁可停下来，不要发出一个版本号没改全的包"""
    text = path.read_text(encoding="utf-8")
    new, n = re.subn(pattern, repl, text, count=1, flags=re.M)
    if n != 1:
        sys.exit(f"{path.name} 里没找到版本号")
    path.write_text(new, encoding="utf-8", newline="")


def bump(v):
    if not re.fullmatch(r"\d+\.\d+\.\d+", v):
        sys.exit(f"版本号要写成 0.2.0 这样：{v}")
    sub_one(TAURI / "Cargo.toml", r'^version = "[^"]+"', f'version = "{v}"')
    sub_one(TAURI / "tauri.conf.json", r'^  "version": "[^"]+"', f'  "version": "{v}"')
    sub_one(ROOT / "Cargo.lock", r'(name = "d2hr-app"\r?\nversion = )"[^"]+"', rf'\g<1>"{v}"')
    print(f"三处版本号已改成 {v}")


def ffmpeg():
    """下载（有缓存就用缓存）、核对 SHA256，把 ffmpeg.exe、DLL、许可证放到 dist/ffmpeg/，再写一份说明"""
    CACHE.mkdir(parents=True, exist_ok=True)
    zp = CACHE / FFMPEG_ZIP
    if not zp.is_file() or sha256(zp) != FFMPEG_SHA256:
        print(f"下载 {FFMPEG_URL}", flush=True)
        part = zp.with_suffix(".part")
        with urllib.request.urlopen(FFMPEG_URL, timeout=300) as r, open(part, "wb") as f:
            shutil.copyfileobj(r, f)
        part.replace(zp)
    got = sha256(zp)
    if got != FFMPEG_SHA256:
        zp.unlink()
        sys.exit(f"ffmpeg 压缩包的 SHA256 对不上：应为 {FFMPEG_SHA256}，实际 {got}")

    if FFMPEG_DIR.exists():
        shutil.rmtree(FFMPEG_DIR)
    FFMPEG_DIR.mkdir(parents=True)
    with zipfile.ZipFile(zp) as z:
        wanted = [n for n in z.namelist()
                  if re.search(r"/bin/(ffmpeg\.exe|[^/]+\.dll)$", n) or n.endswith("/LICENSE.txt")]
        if not any(n.endswith("/bin/ffmpeg.exe") for n in wanted):
            sys.exit("ffmpeg 压缩包里没有 bin/ffmpeg.exe")
        for n in wanted:
            (FFMPEG_DIR / Path(n).name).write_bytes(z.read(n))
    (FFMPEG_DIR / "README.txt").write_text(FFMPEG_NOTE, encoding="utf-8")
    # 跑一下，确认能用（缺 DLL 时这里就会失败）
    out = subprocess.run([str(FFMPEG_DIR / "ffmpeg.exe"), "-hide_banner", "-version"], capture_output=True, text=True, check=True).stdout
    total = sum(p.stat().st_size for p in FFMPEG_DIR.iterdir())
    print(f"ffmpeg 就绪：{out.splitlines()[0]}（{len(list(FFMPEG_DIR.iterdir()))} 个文件，共 {total / 1048576:.0f} MB）")


def build():
    v = version()
    env = dict(os.environ)
    if not env.get("TAURI_SIGNING_PRIVATE_KEY"):
        if not KEY_FILE.is_file():
            sys.exit(f"没有签名私钥：设 TAURI_SIGNING_PRIVATE_KEY，或者放在 {KEY_FILE}")
        env["TAURI_SIGNING_PRIVATE_KEY"] = KEY_FILE.read_text(encoding="utf-8")
    env.setdefault("TAURI_SIGNING_PRIVATE_KEY_PASSWORD", "")

    # frontendDist 是整个 ui/，gitignore 挡不住打包：ui/ 里任何没进仓库的文件都会被嵌进 exe（helper2 吃过亏）
    stray = subprocess.run(["git", "ls-files", "--others", "ui"], cwd=ROOT, capture_output=True, text=True, check=True).stdout.split()
    if stray:
        sys.exit("ui/ 里有没进仓库的文件，会被打进 exe，先挪走：\n  " + "\n  ".join(stray))

    ffmpeg()
    npx = "npx.cmd" if os.name == "nt" else "npx"
    # tauri.bundle.json 只在打包时加上 ffmpeg 资源，平时编译不用先下 ffmpeg。
    # `--` 后面的参数传给 cargo。--locked：必须用 Cargo.lock 锁住的那套依赖编
    run([npx, "-y", TAURI_CLI, "build", "--config", "src-tauri/tauri.bundle.json", "--", "--locked"], env=env)

    nsis = ROOT / "target" / "release" / "bundle" / "nsis"
    setup = nsis / f"dota2-highlight-recorder_{v}_x64-setup.exe"
    sig = Path(str(setup) + ".sig")
    for p in (setup, sig):
        if not p.is_file():
            sys.exit(f"打包产物缺了：{p}")

    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir(parents=True)
    shutil.copy2(setup, OUT / STABLE_SETUP)

    # 一键更新读的清单：安装包地址（带 tag，指得到确切那一版）+ 签名
    latest = {
        "version": v,
        "notes": f"v{v}",
        "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": {"windows-x86_64": {
            "signature": sig.read_text(encoding="utf-8").strip(),
            "url": f"https://github.com/{REPO}/releases/download/v{v}/{STABLE_SETUP}",
        }},
    }
    (OUT / "latest.json").write_text(json.dumps(latest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    for p in sorted(OUT.iterdir()):
        print(f"  {p.name:40} {p.stat().st_size / 1048576:7.1f} MB  {sha256(p) if p.suffix == '.exe' else ''}")


def release_files():
    """Release 上应有的文件。漏传 latest.json，装了的人收不到这一版；漏传安装包，下载链接就 404"""
    return [OUT / STABLE_SETUP, OUT / "latest.json"]


def release_state(tag):
    """None = 还没有这个 Release；否则返回它是不是草稿"""
    r = subprocess.run(["gh", "release", "view", tag, "--json", "isDraft", "-q", ".isDraft"], cwd=ROOT, capture_output=True, text=True)
    return None if r.returncode != 0 else r.stdout.strip() == "true"


def publish(notes):
    tag = f"v{version()}"
    files = release_files()
    for p in files:
        if not p.is_file():
            sys.exit(f"先 build：缺 {p.name}")
    # 先建草稿，传完、verify 核对通过才转正：草稿对外不可见、不算 latest，传到一半时不会有人拿到坏的（同 helper2）
    state = release_state(tag)
    if state is None:
        # --verify-tag：远端没有这个 tag 就不建（否则 gh 会替你在默认分支上新打一个）
        run(["gh", "release", "create", tag, "--draft", "--verify-tag", "--title", tag, "--notes-file", notes])
    elif state:
        print(f"{tag} 的草稿已存在（上次中途失败？），接着用它")
        run(["gh", "release", "edit", tag, "--notes-file", notes])
    else:
        sys.exit(f"{tag} 已经正式发布了，不再改它的文件")
    run(["gh", "release", "upload", tag, "--clobber", *map(str, files)])


def verify(v):
    tag = f"v{v}"
    tmp = Path(tempfile.mkdtemp())
    run(["gh", "release", "download", tag, "-D", str(tmp)])
    want = {p.name: p for p in release_files()}
    got = {p.name: p for p in tmp.iterdir()}
    bad = []
    for name in sorted(want.keys() | got.keys()):
        local, remote = want.get(name), got.get(name)
        if remote is None:
            mark = "**Release 上没有**"
        elif local is None:
            mark = "**Release 上多出来的**"
        elif not local.is_file():
            mark = "**本地没有，没法核对**"
        elif sha256(local) != sha256(remote):
            mark = "**不一致**"
        else:
            mark = "一致"
        print(f"  {name:40} {sha256(remote) if remote else '':64} {mark}")
        if mark != "一致":
            bad.append(name)
    if bad:
        sys.exit(f"核对没通过：{', '.join(bad)}" + ("（Release 还是草稿，没有对外公开）" if release_state(tag) else ""))
    if release_state(tag):
        run(["gh", "release", "edit", tag, "--draft=false"])
        print(f"{tag} 已转正")


if __name__ == "__main__":
    a = sys.argv[1:]
    if not a:
        sys.exit(__doc__)
    cmd = a[0]
    if cmd == "bump" and len(a) == 2:
        bump(a[1])
    elif cmd == "ffmpeg" and len(a) == 1:
        ffmpeg()
    elif cmd == "build" and len(a) == 1:
        build()
    elif cmd == "publish" and len(a) == 2:
        publish(a[1])
    elif cmd == "verify" and len(a) == 2:
        verify(a[1])
    else:
        sys.exit(__doc__)
