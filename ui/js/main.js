// 入口：读设置和语言，在录像列表和比赛页之间切换
import { applyStatic, loadLang, resolve } from "./i18n.js";
import { loadSettings, settings } from "./state.js";
import { setHeroNames } from "./format.js";
import { heroNames, isRecording } from "./api.js";
import { showList } from "./list.js";
import { showMatch } from "./match.js";
import { confirmStart, showRecording } from "./record.js";
import { openSettings } from "./settings.js";

const view = document.getElementById("view");
const dialog = document.getElementById("settings");
const confirmDialog = document.getElementById("confirm");
let current = { name: "list" };

function render() {
  if (current.name === "match") showMatch(view, current.id, { back: home, record });
  else showList(view, { openMatch });
}

/** 比赛页点了开始录制：确认后进入进度页；录完"返回比赛"回到同一场 */
async function record({ matchId, player, clips, secs, minutes }) {
  if (!(await confirmStart(confirmDialog, { count: clips.length, secs, minutes }))) return;
  current = { name: "recording", id: matchId };
  showRecording(view, confirmDialog, { matchId, player, clips, back: () => openMatch(matchId) });
}

async function home() {
  // 录制中不离开进度页（录完的结果也在这页）
  if (current.name === "recording" && (await isRecording())) return;
  current = { name: "list" };
  render();
}

function openMatch(id) {
  current = { name: "match", id };
  render();
}

/** 加载界面语言和对应语言的英雄名 */
async function applyLanguage() {
  const lang = resolve(settings.language);
  const [, names] = await Promise.all([loadLang(lang), heroNames(lang).catch(() => ({}))]);
  setHeroNames(names);
  applyStatic();
}

async function start() {
  await loadSettings();
  await applyLanguage();
  document.getElementById("home").addEventListener("click", home);
  document.getElementById("open-settings").addEventListener("click", () =>
    openSettings(dialog, {
      onSaved: async () => {
        // 语言可能改了：重新加载词条和英雄名，当前页面重画
        await applyLanguage();
        render();
      },
    }));
  render();
}

start();
