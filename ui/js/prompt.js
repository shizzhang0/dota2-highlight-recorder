// 录制中盖在游戏上的置顶小对话框：暂停后继续还是停止、录制流程要问的是/否问题
import { loadLang, resolve, t } from "./i18n.js";

const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const $ = (id) => document.getElementById(id);
let current = null;

function show(p) {
  current = p;
  if (p.kind === "pause") {
    $("title").textContent = t("rec.paused.title");
    $("text").textContent = t("rec.paused.what");
    $("yes").textContent = t("rec.paused.resume");
    $("no").textContent = t("rec.paused.stop");
  } else {
    $("title").textContent = t("rec.ask.title");
    $("text").textContent = p.question;
    $("yes").textContent = t("rec.ask.yes");
    $("no").textContent = t("rec.ask.no");
  }
  $("yes").focus();
}

function answer(yes) {
  if (!current) return;
  const p = current;
  current = null;
  if (p.kind === "pause") invoke(yes ? "resume_recording" : "stop_recording");
  else invoke("answer_confirm", { id: p.id, yes });
}

async function start() {
  const settings = await invoke("get_settings");
  await loadLang(resolve(settings.language));
  $("yes").addEventListener("click", () => answer(true));
  $("no").addEventListener("click", () => answer(false));
  window.__TAURI__.event.listen("prompt-show", (e) => show(e.payload));
}

start();
