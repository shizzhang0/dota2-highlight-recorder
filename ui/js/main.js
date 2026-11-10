// 入口：读设置和语言，顶部固定导航（比赛 · 已录视频 · 设置），在各页之间切换。交互见 docs/app-interaction.md
import { applyStatic, loadLang, resolve } from "./i18n.js";
import { loadSettings, settings } from "./state.js";
import { setHeroNames } from "./format.js";
import * as api from "./api.js";
import { showHome } from "./list.js";
import { showMatch } from "./match.js";
import { confirmBusy, confirmStart, recordingMatch, showRecording, showRecordingFailed, startRecording } from "./record.js";
import { showVideos, showVideoMatch } from "./videos.js";
import { leaveSettings, showSettings } from "./settings.js";
import { checkUpdate, onUpdate } from "./update.js";

const view = document.getElementById("view");
const confirmDialog = document.getElementById("confirm");
const navButtons = [...document.querySelectorAll("[data-nav]")];
const badge = document.getElementById("recording-badge");
const gameStatus = document.getElementById("game-status");
const quitBtn = document.getElementById("quit-game");

/** 当前页：home / match {id, player?, preselect?} / videos / video {id, latest?} / settings / recording */
let current = { name: "home" };

const nav = {
  home: () => go({ name: "home" }),
  openMatch: (id, extra = {}) => go({ name: "match", id, ...extra }),
  videos: () => go({ name: "videos" }),
  settings: () => go({ name: "settings" }),
  openVideo: (id, extra = {}) => go({ name: "video", id, ...extra }),
  record,
};

function render() {
  for (const b of navButtons) {
    const page = b.dataset.nav;
    const on = { home: ["home", "match"], videos: ["videos", "video"], settings: ["settings"] }[page] ?? [];
    b.classList.toggle("active", on.includes(current.name));
  }
  badge.classList.toggle("active", current.name === "recording");
  switch (current.name) {
    case "recording": return showRecording(view);
    case "settings": return showSettings(view, { onSaved: settingsSaved });
    case "match": return showMatch(view, current.id, { ...nav, player: current.player, preselect: current.preselect });
    case "videos": return showVideos(view, nav);
    case "video": return showVideoMatch(view, current.id, confirmDialog, { ...nav, latest: current.latest });
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

/** 比赛页点了开始录制（`clips` 是 plan 返回的片段行）：确认后进入进度页。
 * 录完时还在进度页就跳到这场比赛的录制详情；在别的页面就不打扰（系统通知会提示），正看着"已录视频"就刷新一下 */
async function record({ matchId, player, clips, camera, secs, minutes, overwrite }) {
  const busy = recordingMatch();
  if (busy !== null) {
    if (await confirmBusy(confirmDialog, busy)) go({ name: "recording" });
    return;
  }
  if (!(await confirmStart(confirmDialog, { count: clips.length, secs, minutes, overwrite }))) return;
  setRecording(true);
  const started = startRecording(confirmDialog, {
    matchId,
    player,
    clips,
    camera,
    onFinished: (f) => {
      setRecording(false);
      if (current.name === "recording") {
        current = { name: "video", id: matchId, latest: f };
        render();
      } else if (["videos", "video"].includes(current.name)) {
        render();
      }
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
