// 录制中盖在游戏上的置顶小对话框：录制流程要问的是/否问题（例如要不要重启游戏）
import { loadLang, resolve, t } from "./i18n.js";

const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const $ = (id) => document.getElementById(id);
let current = null;

function show(p) {
  current = p;
  $("title").textContent = t("rec.ask.title");
  $("text").textContent = p.question;
  $("yes").textContent = t("rec.ask.yes");
  $("no").textContent = t("rec.ask.no");
  $("yes").focus();
}

function answer(yes) {
  if (!current) return;
  const p = current;
  current = null;
  invoke("answer_confirm", { id: p.id, yes });
}

async function start() {
  const settings = await invoke("get_settings");
  await loadLang(resolve(settings.language));
  $("yes").addEventListener("click", () => answer(true));
  $("no").addEventListener("click", () => answer(false));
  window.__TAURI__.event.listen("prompt-show", (e) => show(e.payload));
}

start();
