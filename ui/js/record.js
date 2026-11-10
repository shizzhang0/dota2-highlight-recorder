// 录制：开始前确认 → 进度（事件来自 Rust 的 pipeline）。录完的结果在"已录视频"里看（videos.js）。
// 录制中可以切到别的页面，回到进度页时按保存的状态重画
// Dota 放在最前面录，发给它的点击和按键被拦下；Dota 左上角有置顶的进度窗（overlay.js），那里和这里都能停止
import * as api from "./api.js";
import { resolve, t } from "./i18n.js";
import { el, length } from "./format.js";
import { describe } from "./events.js";
import { settings } from "./state.js";

const listen = (name, fn) => window.__TAURI__.event.listen(name, (e) => fn(e.payload));

/** 开始前的确认框，返回用户是否点了开始 */
export function confirmStart(dialog, { count, secs, minutes, overwrite }) {
  return new Promise((done) => {
    const close = (ok) => { dialog.close(); done(ok); };
    const native = settings.capture === "native";
    dialog.replaceChildren(
      el("h2", {}, t("rec.confirm.title")),
      el("p", {}, t("rec.confirm.what", { n: count, secs: length(secs), minutes })),
      // replaceChildren 会把 null 写成文字 "null"，没有就放空字符串
      overwrite ? el("p", { class: "muted" }, t("rec.confirm.overwrite", { n: overwrite })) : "",
      el("p", { class: "warn" }, t("rec.confirm.window")),
      el("p", { class: "muted" }, t("rec.confirm.block")),
      el("p", { class: "muted" }, t(native ? "rec.confirm.restartNative" : "rec.confirm.restart1080")),
      el("div", { class: "dialog-actions" },
        el("button", { type: "button", onclick: () => close(false) }, t("settings.cancel")),
        el("button", { type: "button", class: "primary", onclick: () => close(true) }, t("rec.confirm.start")),
      ),
    );
    dialog.addEventListener("cancel", () => done(false), { once: true });
    dialog.showModal();
  });
}

/** 录制中途的是/否问题（例如要不要重启游戏） */
// 正在主窗口上问的问题：置顶对话框那边先回答了，就把这边收起（返回 null，不再回答一次）
let asking = null;

function ask(dialog, question, id) {
  return new Promise((done) => {
    const close = (yes) => { asking = null; dialog.close(); done(yes); };
    asking = { id, close: () => close(null) };
    dialog.replaceChildren(
      el("h2", {}, t("rec.ask.title")),
      el("p", {}, question),
      el("div", { class: "dialog-actions" },
        el("button", { type: "button", onclick: () => close(false) }, t("rec.ask.no")),
        el("button", { type: "button", class: "primary", onclick: () => close(true) }, t("rec.ask.yes")),
      ),
    );
    dialog.addEventListener("cancel", () => { asking = null; done(false); }, { once: true });
    dialog.showModal();
  });
}

// 正在录的这一次：状态不跟着页面走。录制中可以切到别的页面（2026-11-10 放开：录制不再暂停，点主窗口不影响录制），
// 点导航上的"录制中"回到进度页，按这里的状态重画。null = 没在录
// { matchId, step, clip, bar（这一段的进度 0–1；null = 还没开始录）, log: [文字], stopping }
let rec = null;
/** 当前进度页的刷新函数（切走后它的元素已经不在文档里，调了也不显示） */
let page = null;

/** 正在录的比赛 ID；没在录返回 null */
export function recordingMatch() {
  return rec?.matchId ?? null;
}

/** 开始录制：`clips` 是要录的片段（plan 返回的行），`camera` 是比赛页上选的镜头。
 * 录制结束调 `onFinished(结果)`；没能开始调 `onFailed(原因)` */
export async function startRecording(dialog, { matchId, player, clips, camera, onFinished, onFailed }) {
  rec = { matchId, step: t("rec.starting"), clip: "", bar: null, log: [], stopping: false };
  const update = () => page?.();
  const unlisten = await Promise.all([
    listen("record-event", (e) => {
      const text = describe(e);
      if (text) rec.log.push(text);
      if (e.kind === "clip_started") rec.clip = text;
      // 这一段的进度：按游戏报来的帧数走
      else if (e.kind === "segment_started") rec.bar = 0;
      else if (e.kind === "segment_progress") rec.bar = e.frames / e.total;
      else if (e.kind === "segment_done") rec.bar = 1;
      if (text && e.kind !== "clip_started") rec.step = text;
      update();
    }),
    listen("record-stopping", (reason) => {
      rec.stopping = true;
      rec.step = t("rec.stopping", { reason });
      update();
    }),
    listen("record-confirm", async (q) => {
      const yes = await ask(dialog, q.question, q.id);
      if (yes !== null) api.answerConfirm(q.id, yes);
    }),
    listen("record-confirm-done", (id) => { if (asking?.id === id) asking.close(); }),
    listen("record-finished", (f) => {
      unlisten.forEach((u) => u());
      rec = null;
      page = null;
      onFinished(f);
    }),
  ]);

  try {
    await api.startRecording(matchId, player, clips.map((c) => c.index), camera, resolve(settings.language));
  } catch (e) {
    unlisten.forEach((u) => u());
    rec = null;
    page = null;
    onFailed(e.message);
  }
}

/** 进度页：按 `rec` 画；录制状态变了就重画 */
export function showRecording(view) {
  const step = el("div", { class: "step" });
  const clipLine = el("div", { class: "muted" });
  const bar = el("div", { class: "bar-fill" });
  const barBox = el("div", { class: "bar", hidden: true }, bar);
  const log = el("ol", { class: "log" });
  const stopBtn = el("button", {
    type: "button",
    onclick: () => { stopBtn.disabled = true; api.stopRecording(); },
  }, t("rec.stop"));
  view.replaceChildren(
    el("div", { class: "card" },
      el("div", { class: "row" }, el("h2", { style: "margin:0;flex:1" }, t("rec.title")), stopBtn),
      el("p", { class: "warn" }, t("rec.window")),
      step, clipLine, barBox,
    ),
    el("div", { class: "card" }, el("h2", {}, t("rec.log")), log),
  );
  page = () => {
    if (!rec || !step.isConnected) return;
    step.textContent = rec.step;
    clipLine.textContent = rec.clip;
    barBox.hidden = rec.bar === null;
    bar.style.width = `${Math.min(100, (rec.bar ?? 0) * 100)}%`;
    stopBtn.disabled = rec.stopping;
    // 记录只追加新的几行，免得每半秒整个重画、滚动位置跳回去
    for (const text of rec.log.slice(log.children.length)) log.append(el("li", {}, text));
  };
  page();
}

/** 没能开始录制：显示原因和返回按钮 */
export function showRecordingFailed(view, message, back) {
  view.replaceChildren(
    el("button", { type: "button", onclick: back }, t("nav.back")),
    el("div", { class: "card error" }, message),
  );
}

/** 正在录别的时又点了开始录制：说明一下，可以去看进度。返回是否点了"查看进度" */
export function confirmBusy(dialog, matchId) {
  return new Promise((done) => {
    const close = (view) => { dialog.close(); done(view); };
    dialog.replaceChildren(
      el("h2", {}, t("rec.busy.title")),
      el("p", {}, t("rec.busy.what", { id: matchId })),
      el("div", { class: "dialog-actions" },
        el("button", { type: "button", onclick: () => close(false) }, t("settings.cancel")),
        el("button", { type: "button", class: "primary", onclick: () => close(true) }, t("rec.busy.view")),
      ),
    );
    dialog.addEventListener("cancel", () => done(false), { once: true });
    dialog.showModal();
  });
}
