// 检查更新、一键更新，做法同 dota2-game-helper2。
// 查 GitHub 最新 Release 的 tag：请求从前端发，WebView2 和 Edge 用同一套代理设置（系统代理、PAC），
// Rust 的 HTTP 库默认不读系统代理。装由 Rust 侧（src-tauri/src/updater.rs）做，只对安装版生效。
// 结果三种：{ state: "new", version } 有新版、{ state: "latest" } 已是最新、{ state: "fail" } 连不上
const REPO = "shizzhang0/dota2-highlight-recorder";
const TIMEOUT_MS = 5000;
const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);

/** `v1.3.5` / `1.3.5` → [1, 3, 5]；认不出来返回 null */
function parse(v) {
  const m = /^v?(\d+)\.(\d+)\.(\d+)/.exec(String(v || "").trim());
  return m ? m.slice(1).map(Number) : null;
}

function newer(a, b) {
  for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] > b[i];
  return false;
}

/** 最新 Release 的版本号；还没有发过版返回 null，连不上抛错 */
async function latestRelease() {
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), TIMEOUT_MS);
  try {
    const r = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`,
      { signal: ctl.signal, headers: { Accept: "application/vnd.github+json" } });
    if (r.status === 404) return null;
    if (!r.ok) throw new Error(`github ${r.status}`);
    return (await r.json()).tag_name;
  } finally {
    clearTimeout(timer);
  }
}

let last = null;
let running = null;
const listeners = new Set();

/** 最近一次的结果（还没查过是 null） */
export function lastUpdate() { return last; }
/** 结果变了就回调（包括 { state: "checking" }）；返回取消订阅的函数 */
export function onUpdate(cb) { listeners.add(cb); return () => listeners.delete(cb); }

/** 查一次。同时只跑一个：上一次没回来时又点了，就等同一个结果 */
export function checkUpdate() {
  if (running) return running;
  for (const cb of listeners) cb({ state: "checking" });
  running = (async () => {
    try {
      const [tag, mine] = await Promise.all([latestRelease(), invoke("app_version")]);
      last = tag && parse(tag) && newer(parse(tag), parse(mine))
        ? { state: "new", version: tag.replace(/^v?/, "v") }
        : { state: "latest" };
    } catch {
      last = { state: "fail" };
    }
    for (const cb of listeners) cb(last);
    return last;
  })().finally(() => { running = null; });
  return running;
}

/** 是不是安装版（有 NSIS 的 uninstall.exe）。一键更新只对安装版生效 */
let installed = null;
export function isInstalled() {
  installed ??= invoke("app_installed").catch(() => false);
  return installed;
}

export const appVersion = () => invoke("app_version");
export const openReleasePage = () => invoke("open_release_page");

/** 一键更新：下载、验签、静默安装，装完程序自己重启。`onProgress(已下字节, 总字节|null)`。
 * 成功时程序在安装器启动那一刻就结束了，这个 Promise 不会有结果；有结果就是失败，值是错误
 * （"portable" 不是安装版、"recording" 正在录制、"none" 没有可装的更新，其余是原文） */
export async function installUpdate(onProgress) {
  const off = await window.__TAURI__.event.listen("update-progress", (e) => onProgress?.(e.payload.downloaded, e.payload.total));
  try {
    await invoke("update_install");
    return "unknown";
  } catch (e) {
    return String(e);
  } finally {
    off();
  }
}
