// 入口：读设置和语言，顶部固定导航（本地录像 · 已录视频 · 设置），在各页之间切换。交互见 docs/app-interaction.md
import { applyStatic, loadLang, resolve, t } from "./i18n.js";
import { loadSettings, settings } from "./state.js";
import { setHeroNames } from "./format.js";
import * as api from "./api.js";
import { showHome } from "./list.js";
import { showMatch } from "./match.js";
import { confirmBusy, confirmStart, recordingMatch, showRecording, showRecordingFailed, startRecording } from "./record.js";
import { showVideos } from "./videos.js";
import { leaveSettings, showSettings } from "./settings.js";
import { checkUpdate, onUpdate } from "./update.js";

// 页面上没接住的报错记进应用日志（d2hr.log，类别「界面」），出问题时能看到前因后果
const logError = (text) => api.writeLog("界面", text).catch(() => {});
window.addEventListener("error", (e) => logError(`报错：${e.message}（${e.filename}:${e.lineno}）`));
window.addEventListener("unhandledrejection", (e) => logError(`没接住的错误：${e.reason?.stack || e.reason}`));

// 页面被藏起、冻住、脚本停了多久也记下来：2026-11-18/19 录完后后台早就发了「录完了」，页面要点一下窗口才收到
// （日志里差了 2 分多钟，每 3 秒的保底查询也没跑），查是哪一种。括号里是页面自己看到的时间，和日志行首的时间对照
const jsTime = () => new Date().toLocaleTimeString("zh-CN", { hour12: false });
document.addEventListener("visibilitychange", () => logError(`页面${document.visibilityState === "hidden" ? "被藏起" : "重新显示"}（${jsTime()}）`));
document.addEventListener("freeze", () => logError(`页面被冻住（${jsTime()}）`));
document.addEventListener("resume", () => logError(`页面解冻（${jsTime()}）`));
let beat = Date.now();
setInterval(() => {
  const now = Date.now();
  if (now - beat > 5000) logError(`页面的定时器停了 ${((now - beat) / 1000).toFixed(0)} 秒，现在恢复（${jsTime()}，${document.visibilityState}，${document.hasFocus() ? "有焦点" : "没焦点"}）`);
  beat = now;
}, 1000);

const view = document.getElementById("view");
const confirmDialog = document.getElementById("confirm");
const navButtons = [...document.querySelectorAll("[data-nav]")];
const badge = document.getElementById("recording-badge");
const gameStatus = document.getElementById("game-status");
const quitBtn = document.getElementById("quit-game");

/** 当前页：home / match {id, from?（"videos" = 从已录视频进来）, player?, preselect?, preselectProducts?} / videos / settings / recording */
let current = { name: "home" };

const nav = {
  home: () => go({ name: "home" }),
  openMatch: (id, extra = {}) => go({ name: "match", id, ...extra }),
  videos: () => go({ name: "videos" }),
  settings: () => go({ name: "settings" }),
  record,
};

function render() {
  for (const b of navButtons) {
    const page = b.dataset.nav;
    // 比赛页算在它是从哪进来的那一项下面
    const at = current.name === "match" ? (current.from === "videos" ? "videos" : "home") : current.name;
    b.classList.toggle("active", page === at);
  }
  badge.classList.toggle("active", current.name === "recording");
  // 内容区自己滚（导航栏不动），换页时回到顶上
  view.scrollTop = 0;
  switch (current.name) {
    case "recording": return showRecording(view, { onVideos: nav.videos, onRetry: retry });
    case "settings": return showSettings(view, { onSaved: settingsSaved });
    case "match": {
      const fromVideos = current.from === "videos";
      return showMatch(view, current.id, {
        ...nav,
        player: current.player,
        preselect: current.preselect,
        preselectProducts: current.preselectProducts,
        back: fromVideos ? nav.videos : nav.home,
        backLabel: t(fromVideos ? "nav.backVideos" : "nav.backMatches"),
      });
    }
    case "videos": return showVideos(view, { ...nav, dialog: confirmDialog });
    default: return showHome(view, confirmDialog, nav);
  }
}

async function go(page) {
  // 录制中也能换页（录制不再暂停，点主窗口不影响录制）；点导航上的"录制中"回到进度页
  // 设置改了没保存：先问保存、不保存还是留下
  if (current.name === "settings" && !(await leaveSettings(confirmDialog))) return;
  current = page;
  render();
}

/** 设置保存后：语言可能改了，重新加载词条和英雄名，重画设置页并显示"已保存" */
async function settingsSaved() {
  await applyLanguage();
  if (current.name === "settings") render()?.saved();
}

/** 录制中导航上显示"录制中"（点它回到进度页） */
function setRecording(on) {
  badge.hidden = !on;
  if (on) gameStatus.hidden = true;
}

/** 上一次开始的录制（`record` 的参数），停止或出错后录制页上「重新录」用 */
let lastRequest = null;

/** 「重新录」（2026-11-20 起，录制已停止、录制出错的结果页上）：去比赛页，选好这个英雄、勾好上次录的成品（「推荐」）
 * 或片段（「自定义」），镜头本来就记着上次的。可以直接开始，也可以先改（用户定：不要原样一键重录，停下来常常就是想改点什么） */
function retry() {
  if (!lastRequest) return;
  const { matchId, player, products, clips } = lastRequest;
  go(products?.length
    ? { name: "match", id: matchId, player, preselectProducts: products }
    : { name: "match", id: matchId, player, preselect: clips.map((c) => c.file_name) });
}

/** 比赛页点了开始录制（`clips` 是 plan 返回的片段行）：确认后进入进度页。
 * 录完不换页：进度页原地显示结果（record.js），点「查看视频」去"已录视频"；在别的页面就不打扰（系统通知会提示），
 * 正看着"已录视频"就刷新一下。2026-11-17 前录完会自动跳到"已录视频"、顶上显示结果，用户觉得不好 */
async function record(request) {
  const { matchId, player, hero, clips, products, camera, compile, secs, minutes, overwrite } = request;
  const busy = recordingMatch();
  if (busy !== null) {
    if (await confirmBusy(confirmDialog, busy)) go({ name: "recording" });
    return;
  }
  if (!(await confirmStart(confirmDialog, { count: clips.length, secs, minutes, overwrite }))) return;
  lastRequest = request;
  setRecording(true);
  const started = startRecording(confirmDialog, {
    matchId,
    player,
    hero,
    clips,
    products,
    camera,
    compile,
    onFinished: (f) => {
      setRecording(false);
      if (current.name === "videos") render();
      pollGame();
    },
    onFailed: (message) => {
      setRecording(false);
      if (current.name !== "recording") return;
      current = { name: "match", id: matchId, player };
      showRecordingFailed(view, message, render);
    },
  });
  current = { name: "recording", id: matchId };
  render();
  await started;
}

/** 录制用的游戏还开着时，导航栏右侧显示"游戏运行中 · 退出" */
async function pollGame() {
  if (recordingMatch() !== null) return;
  gameStatus.hidden = !(await api.gameOpen().catch(() => false));
}

quitBtn.addEventListener("click", async () => {
  quitBtn.disabled = true;
  try {
    await api.quitGame();
    gameStatus.hidden = true;
  } catch (e) {
    alert(e.message);
  } finally {
    quitBtn.disabled = false;
  }
});

/** 加载界面语言和对应语言的英雄名 */
async function applyLanguage() {
  const lang = resolve(settings.language);
  const [, names] = await Promise.all([loadLang(lang), api.heroNames(lang).catch(() => ({}))]);
  setHeroNames(names);
  applyStatic();
}

async function start() {
  await loadSettings();
  await applyLanguage();
  badge.addEventListener("click", () => go({ name: "recording" }));
  // 没有系统标题栏，最小化、最大化、关闭自己做；关闭走的还是系统的关闭流程（录制中会先当作点了停止，见 src-tauri/src/main.rs）
  const win = window.__TAURI__.window.getCurrentWindow();
  document.getElementById("win-min").addEventListener("click", () => win.minimize());
  document.getElementById("win-max").addEventListener("click", () => win.toggleMaximize());
  document.getElementById("win-close").addEventListener("click", () => win.close());
  const syncMax = async () => document.body.classList.toggle("maximized", await win.isMaximized());
  win.onResized(syncMax);
  syncMax();
  for (const b of navButtons) {
    b.addEventListener("click", () => {
      if (b.dataset.nav === "settings") nav.settings();
      else if (b.dataset.nav === "videos") nav.videos();
      else nav.home();
    });
  }
  render();
  pollGame();
  setInterval(pollGame, 3000);
  // 启动时查一次更新：有新版在导航的"设置"上亮一个点，更新按钮在设置页的「关于」里
  const settingsNav = navButtons.find((b) => b.dataset.nav === "settings");
  onUpdate((r) => settingsNav.classList.toggle("has-update", r.state === "new"));
  setTimeout(checkUpdate, 3000);
}

start();
