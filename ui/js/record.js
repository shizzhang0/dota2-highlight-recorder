// 录制：开始前确认 → 进度（事件来自 Rust 的 pipeline）。录完的结果在"已录视频"里看（videos.js）。
// 录制中途要用户拍板的事弹对话框；停止只走暂停对话框（录制中在主窗口点按钮本身就会先触发暂停）
import * as api from "./api.js";
import { t } from "./i18n.js";
import { el, length } from "./format.js";
import { describe } from "./events.js";

const listen = (name, fn) => window.__TAURI__.event.listen(name, (e) => fn(e.payload));

/** 开始前的确认框，返回用户是否点了开始 */
export function confirmStart(dialog, { count, secs, minutes, overwrite }) {
  return new Promise((resolve) => {
    const done = (ok) => { dialog.close(); resolve(ok); };
    dialog.replaceChildren(
      el("h2", {}, t("rec.confirm.title")),
      el("p", {}, t("rec.confirm.what", { n: count, secs: length(secs), minutes })),
      overwrite ? el("p", { class: "muted" }, t("rec.confirm.overwrite", { n: overwrite })) : null,
      el("p", { class: "warn" }, t("rec.confirm.handsOff")),
      el("p", { class: "muted" }, t("rec.confirm.stop")),
      el("p", { class: "muted" }, t("rec.confirm.restart")),
      el("div", { class: "dialog-actions" },
        el("button", { type: "button", onclick: () => done(false) }, t("settings.cancel")),
        el("button", { type: "button", class: "primary", onclick: () => done(true) }, t("rec.confirm.start")),
      ),
    );
    dialog.addEventListener("cancel", () => resolve(false), { once: true });
    dialog.showModal();
  });
}

/** 录制中途的是/否问题（例如要不要重启游戏） */
// 正在主窗口上问的问题：置顶对话框那边先回答了，就把这边收起（返回 null，不再回答一次）
let asking = null;

function ask(dialog, question, id) {
  return new Promise((resolve) => {
    const done = (yes) => { asking = null; dialog.close(); resolve(yes); };
    asking = { id, close: () => done(null) };
    dialog.replaceChildren(
      el("h2", {}, t("rec.ask.title")),
      el("p", {}, question),
      el("div", { class: "dialog-actions" },
        el("button", { type: "button", onclick: () => done(false) }, t("rec.ask.no")),
        el("button", { type: "button", class: "primary", onclick: () => done(true) }, t("rec.ask.yes")),
      ),
    );
    dialog.addEventListener("cancel", () => { asking = null; resolve(false); }, { once: true });
    dialog.showModal();
  });
}

/** 进度页。`clips` 是要录的片段（plan 返回的行）。录制结束调 `onFinished(结果)`；
 * 没能开始先调 `onFailed()`，页面上显示原因，点返回调 `back()` */
export async function showRecording(view, dialog, { matchId, player, clips, onFinished, onFailed, back }) {
  const step = el("div", { class: "step" }, t("rec.starting"));
  const clipLine = el("div", { class: "muted" });
  const bar = el("div", { class: "bar-fill" });
  const barBox = el("div", { class: "bar", hidden: true }, bar);
  const log = el("ol", { class: "log" });
  // 暂停面板：用户动了鼠标键盘时出现，点"继续"接着录，点"停止"彻底结束
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
  const handsOff = el("p", { class: "warn" }, t("rec.handsOff"));
  const body = el("div", { class: "card" },
    el("h2", { style: "margin:0" }, t("rec.title")),
    handsOff, pausedBox,
    step, clipLine, barBox,
  );
  view.replaceChildren(body, el("div", { class: "card" }, el("h2", {}, t("rec.log")), log));

  // 渲染进度：按导出开始时给的预计秒数走，导出完成时填满
  let timer = null;
  const startBar = (secs) => {
    clearInterval(timer);
    barBox.hidden = false;
    const t0 = Date.now();
    bar.style.width = "0%";
    timer = setInterval(() => {
      bar.style.width = `${Math.min(97, ((Date.now() - t0) / 1000 / secs) * 100)}%`;
    }, 300);
  };
  const stopBar = () => { clearInterval(timer); bar.style.width = "100%"; };
  const unlisten = await Promise.all([
    listen("record-event", (e) => {
      const text = describe(e);
      if (text) log.append(el("li", {}, text));
      if (e.kind === "clip_started") clipLine.textContent = text;
      else if (e.kind === "export_started") startBar(e.estimated_secs + 15);
      else if (e.kind === "exported") stopBar();
      if (text && e.kind !== "clip_started") step.textContent = text;
    }),
    listen("record-paused", () => {
      pausedBox.hidden = false;
      handsOff.hidden = true;
      resumeBtn.disabled = false;
    }),
    listen("record-resumed", () => {
      pausedBox.hidden = true;
      handsOff.hidden = false;
    }),
    listen("record-stopping", (reason) => {
      clearInterval(timer);
      pausedBox.hidden = true;
      step.textContent = t("ov.stoppingWhy", { reason });
    }),
    listen("record-confirm", async (q) => {
      const yes = await ask(dialog, q.question, q.id);
      if (yes !== null) api.answerConfirm(q.id, yes);
    }),
    listen("record-confirm-done", (id) => { if (asking?.id === id) asking.close(); }),
    listen("record-finished", (f) => {
      clearInterval(timer);
      unlisten.forEach((u) => u());
      onFinished(f);
    }),
  ]);

  try {
    await api.startRecording(matchId, player, clips.map((c) => c.index));
  } catch (e) {
    clearInterval(timer);
    unlisten.forEach((u) => u());
    onFailed();
    view.replaceChildren(
      el("button", { type: "button", onclick: back }, t("nav.back")),
      el("div", { class: "card error" }, e.message),
    );
  }
}
