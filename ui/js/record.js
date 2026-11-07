// 录制：开始前确认 → 进度（事件来自 Rust 的 pipeline）。录完的结果在"已录视频"里看（videos.js）。
// 录制在后台进行（游戏最小化），用户可以用别的程序；在 Dota 里操作才会暂停，暂停时弹置顶对话框，这里也能点
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
      overwrite ? el("p", { class: "muted" }, t("rec.confirm.overwrite", { n: overwrite })) : null,
      el("p", { class: "warn" }, t("rec.confirm.background")),
      el("p", { class: "muted" }, t("rec.confirm.pause")),
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

/** 进度页。`clips` 是要录的片段（plan 返回的行），`camera` 是比赛页上选的镜头。录制结束调 `onFinished(结果)`；
 * 没能开始先调 `onFailed()`，页面上显示原因，点返回调 `back()` */
export async function showRecording(view, dialog, { matchId, player, clips, camera, onFinished, onFailed, back }) {
  const step = el("div", { class: "step" }, t("rec.starting"));
  const clipLine = el("div", { class: "muted" });
  const bar = el("div", { class: "bar-fill" });
  const barBox = el("div", { class: "bar", hidden: true }, bar);
  const log = el("ol", { class: "log" });
  // 暂停面板：用户在 Dota 里操作时出现，点"继续"重录被打断的那一段，点"停止"彻底结束
  const resumeBtn = el("button", {
    type: "button",
    class: "primary",
    onclick: () => { resumeBtn.disabled = true; api.resumeRecording(); },
  }, t("rec.paused.resume"));
  const pausedBox = el("div", { class: "paused", hidden: true },
    el("div", { class: "paused-title" }, t("rec.paused.title")),
    el("p", {}, t("rec.paused.what")),
    el("div", { class: "row" }, resumeBtn,
      el("button", { type: "button", onclick: () => api.stopRecording() }, t("rec.paused.stop"))),
  );
  const note = el("p", { class: "warn" }, t("rec.background"));
  const body = el("div", { class: "card" },
    el("h2", { style: "margin:0" }, t("rec.title")),
    note, pausedBox,
    step, clipLine, barBox,
  );
  view.replaceChildren(body, el("div", { class: "card" }, el("h2", {}, t("rec.log")), log));

  // 这一段的进度：按游戏报来的帧数走
  const setBar = (fraction) => {
    barBox.hidden = false;
    bar.style.width = `${Math.min(100, fraction * 100)}%`;
  };
  const unlisten = await Promise.all([
    listen("record-event", (e) => {
      const text = describe(e);
      if (text) log.append(el("li", {}, text));
      if (e.kind === "clip_started") clipLine.textContent = text;
      else if (e.kind === "segment_started") setBar(0);
      else if (e.kind === "segment_progress") setBar(e.frames / e.total);
      else if (e.kind === "segment_done") setBar(1);
      if (text && e.kind !== "clip_started") step.textContent = text;
    }),
    listen("record-paused", () => {
      pausedBox.hidden = false;
      note.hidden = true;
      resumeBtn.disabled = false;
    }),
    listen("record-resumed", () => {
      pausedBox.hidden = true;
      note.hidden = false;
    }),
    listen("record-stopping", (reason) => {
      pausedBox.hidden = true;
      step.textContent = t("rec.stopping", { reason });
    }),
    listen("record-confirm", async (q) => {
      const yes = await ask(dialog, q.question, q.id);
      if (yes !== null) api.answerConfirm(q.id, yes);
    }),
    listen("record-confirm-done", (id) => { if (asking?.id === id) asking.close(); }),
    listen("record-finished", (f) => {
      unlisten.forEach((u) => u());
      onFinished(f);
    }),
  ]);

  try {
    await api.startRecording(matchId, player, clips.map((c) => c.index), camera, resolve(settings.language));
  } catch (e) {
    unlisten.forEach((u) => u());
    onFailed();
    view.replaceChildren(
      el("button", { type: "button", onclick: back }, t("nav.back")),
      el("div", { class: "card error" }, e.message),
    );
  }
}
