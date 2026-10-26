// 入口：读设置和语言，在录像列表和比赛页之间切换
import { applyStatic, loadLang, resolve } from "./i18n.js";
import { loadSettings, settings } from "./state.js";
import { setHeroNames } from "./format.js";
import { heroNames } from "./api.js";
import { showList } from "./list.js";
import { showMatch } from "./match.js";
import { openSettings } from "./settings.js";

const view = document.getElementById("view");
const dialog = document.getElementById("settings");
let current = { name: "list" };

function render() {
  if (current.name === "match") showMatch(view, current.id, { back: home });
  else showList(view, { openMatch });
}

function home() {
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
