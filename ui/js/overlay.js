// 录制时的置顶进度窗：只听事件，不能点（Rust 侧设了鼠标穿透）
import { loadLang, resolve, t } from "./i18n.js";
import { clock } from "./format.js";
import { describe } from "./events.js";

const $ = (id) => document.getElementById(id);
const box = document.querySelector(".box");
const listen = (name, fn) => window.__TAURI__.event.listen(name, (e) => fn(e.payload));

let total = 0, done = 0, estimated = 0, t0 = 0, timer = null, stopping = false, clipTitle = "";

function tick() {
  const elapsed = (Date.now() - t0) / 1000;
  $("time").textContent = t("ov.time", { elapsed: clock(elapsed), total: clock(estimated) });
  // 按时间走，片段完成时至少到"已完成的比例"
  const byTime = estimated > 0 ? elapsed / estimated : 0;
  const pct = Math.min(0.97, Math.max(byTime, total ? done / total : 0));
  $("fill").style.width = `${pct * 100}%`;
}

/** 状态：recording（红）/ stopping（黄）/ done（绿） */
function setState(state) {
  box.dataset.state = state;
}

async function start() {
  const settings = await window.__TAURI__.core.invoke("get_settings");
  await loadLang(resolve(settings.language));

  listen("record-started", (s) => {
    total = s.clips;
    done = 0;
    estimated = s.estimated_secs;
    t0 = 0;
    clipTitle = "";
    stopping = false;
    clearInterval(timer);
    setState("recording");
    $("title").textContent = t("ov.preparing");
    $("step").textContent = t("rec.starting");
    $("time").textContent = "";
    $("fill").style.width = "0%";
    $("hint").textContent = t("ov.hint");
  });

  listen("record-event", (e) => {
    if (stopping) return;
    const text = describe(e);
    if (e.kind === "countdown") {
      // 倒计时开始才算正式录制，计时从这里起（启动游戏的时间不算在预计里）
      if (!t0) {
        t0 = Date.now();
        timer = setInterval(tick, 500);
      }
      $("title").textContent = t("ov.countdown", { secs: e.secs });
    } else if (e.kind === "clip_started") {
      clipTitle = t("ov.clip", { n: e.n, total: e.total });
      $("title").textContent = clipTitle;
      $("step").textContent = e.name;
    } else if (e.kind === "export_started") {
      // 暂停后继续时标题停在倒计时上，开始导出时换回"片段 n/total"
      if (clipTitle) $("title").textContent = clipTitle;
    } else if (e.kind === "clip_done") {
      done += 1;
    }
    if (text && e.kind !== "clip_started" && e.kind !== "countdown") $("step").textContent = text;
  });

  // 用户一动就暂停：先告诉用户"收到了"，去应用里点继续或停止
  listen("record-paused", () => {
    setState("stopping");
    $("title").textContent = t("ov.paused");
    $("step").textContent = t("ov.pausedWhat");
    $("hint").textContent = t("ov.pausedHint");
  });
  listen("record-resumed", () => {
    setState("recording");
    $("title").textContent = t("ov.resuming");
    $("step").textContent = t("ev.resuming");
    $("hint").textContent = t("ov.hint");
  });

  // 点了停止：流程还要做完手上这一步、恢复设置，先告诉用户"收到了"
  listen("record-stopping", (reason) => {
    stopping = true;
    clearInterval(timer);
    setState("stopping");
    $("title").textContent = t("ov.stopping");
    $("step").textContent = t("ov.stoppingWhy", { reason });
    $("hint").textContent = t("ov.stoppingHint");
  });

  listen("record-finished", (f) => {
    clearInterval(timer);
    setState(f.error ? "stopping" : "done");
    $("fill").style.width = f.cancelled || f.error ? $("fill").style.width : "100%";
    if (f.error) $("title").textContent = t("ov.error");
    else if (f.cancelled) $("title").textContent = t("ov.stopped", { done: f.completed.length, total: f.selected.length });
    else $("title").textContent = t("ov.done", { n: f.files.length });
    $("step").textContent = "";
    $("hint").textContent = t("ov.seeApp");
  });
}

start();
