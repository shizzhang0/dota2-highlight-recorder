// 入口：读设置和语言，顶部固定导航（比赛 · 已录视频 · 设置），在各页之间切换。交互见 docs/app-interaction.md
import { applyStatic, loadLang, resolve } from "./i18n.js";
import { loadSettings, settings } from "./state.js";
import { setHeroNames } from "./format.js";
import * as api from "./api.js";
import { showHome } from "./list.js";
import { showMatch } from "./match.js";
import { confirmStart, showRecording } from "./record.js";
import { showVideos, showVideoMatch } from "./videos.js";
import { openSettings } from "./settings.js";

const view = document.getElementById("view");
const dialog = document.getElementById("settings");
const confirmDialog = document.getElementById("confirm");
const navButtons = [...document.querySelectorAll("[data-nav]")];
const badge = document.getElementById("recording-badge");
const gameStatus = document.getElementById("game-status");
const quitBtn = document.getElementById("quit-game");

/** 当前页：home / match {id, player?, preselect?} / videos / video {id, latest?} / recording */
let current = { name: "home" };

const nav = {
  home: () => go({ name: "home" }),
  openMatch: (id, extra = {}) => go({ name: "match", id, ...extra }),
  videos: () => go({ name: "videos" }),
  openVideo: (id, extra = {}) => go({ name: "video", id, ...extra }),
  record,
};

function render() {
  for (const b of navButtons) {
    const page = b.dataset.nav;
    b.classList.toggle("active", page === "home" ? ["home", "match"].includes(current.name) : page === "videos" && ["videos", "video"].includes(current.name));
  }
  switch (current.name) {
    case "match": return showMatch(view, current.id, { ...nav, player: current.player, preselect: current.preselect });
    case "videos": return showVideos(view, nav);
    case "video": return showVideoMatch(view, current.id, confirmDialog, { ...nav, latest: current.latest });
    default: return showHome(view, confirmDialog, nav);
  }
}

function go(page) {
  // 录制中只看进度：在主窗口上点东西本身就会触发暂停，要操作走暂停对话框
  if (current.name === "recording") return;
  current = page;
  render();
}

/** 录制中导航变灰，显示"录制中" */
function setRecording(on) {
  for (const b of navButtons) b.disabled = on;
  badge.hidden = !on;
  if (on) gameStatus.hidden = true;
}

/** 比赛页点了开始录制（`clips` 是 plan 返回的片段行）：确认后进入进度页；录完跳到这场比赛的录制详情 */
async function record({ matchId, player, clips, secs, minutes, overwrite }) {
  if (!(await confirmStart(confirmDialog, { count: clips.length, secs, minutes, overwrite }))) return;
  current = { name: "recording", id: matchId };
  setRecording(true);
  showRecording(view, confirmDialog, {
    matchId,
    player,
    clips,
    onFinished: (f) => {
      setRecording(false);
      current = { name: "video", id: matchId, latest: f };
      render();
      pollGame();
    },
    onFailed: () => {
      setRecording(false);
      current = { name: "match", id: matchId, player };
    },
    back: render,
  });
}

/** 录制用的游戏还开着时，导航栏右侧显示"游戏运行中 · 退出" */
async function pollGame() {
  if (current.name === "recording") return;
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
  for (const b of navButtons) {
    b.addEventListener("click", () => {
      if (b.dataset.nav === "settings") {
        openSettings(dialog, {
          onSaved: async () => {
            // 语言可能改了：重新加载词条和英雄名，当前页面重画
            await applyLanguage();
            if (current.name !== "recording") render();
          },
        });
      } else if (b.dataset.nav === "videos") nav.videos();
      else nav.home();
    });
  }
  render();
  pollGame();
  setInterval(pollGame, 3000);
}

start();
