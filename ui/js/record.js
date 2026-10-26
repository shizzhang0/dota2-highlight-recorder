// 录制：开始前确认 → 进度（事件来自 Rust 的 pipeline）→ 结果。录制中途要用户拍板的事弹对话框
import * as api from "./api.js";
import { t } from "./i18n.js";
import { clock, el, length } from "./format.js";

const listen = (name, fn) => window.__TAURI__.event.listen(name, (e) => fn(e.payload));

/** 开始前的确认框，返回用户是否点了开始 */
export function confirmStart(dialog, { count, secs, minutes }) {
  return new Promise((resolve) => {
    const done = (ok) => { dialog.close(); resolve(ok); };
    dialog.replaceChildren(
      el("h2", {}, t("rec.confirm.title")),
      el("p", {}, t("rec.confirm.what", { n: count, secs: length(secs), minutes })),
      el("p", { class: "warn" }, t("rec.confirm.handsOff")),
      el("p", { class: "muted" }, t("rec.confirm.stop", { hotkey: "Ctrl+Shift+F12" })),
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
function ask(dialog, question) {
  return new Promise((resolve) => {
    const done = (yes) => { dialog.close(); resolve(yes); };
    dialog.replaceChildren(
      el("h2", {}, t("rec.ask.title")),
      el("p", {}, question),
      el("div", { class: "dialog-actions" },
        el("button", { type: "button", onclick: () => done(false) }, t("rec.ask.no")),
        el("button", { type: "button", class: "primary", onclick: () => done(true) }, t("rec.ask.yes")),
      ),
    );
    dialog.addEventListener("cancel", () => resolve(false), { once: true });
    dialog.showModal();
  });
}

/** 事件 → 一行文字；不值得显示的返回 null */
function describe(e) {
  switch (e.kind) {
    case "waiting_for_dota_to_quit": return t("ev.waitQuit");
    case "launching_game": return t("ev.launch", { args: e.args.join(" ") });
    case "replay_loaded": return t("ev.loaded");
    case "countdown": return t("ev.countdown", { secs: e.secs });
    case "hotkey_unavailable": return t("ev.noHotkey", { hotkey: e.hotkey });
    case "clip_started": return t("ev.clip", { n: e.n, total: e.total, name: e.name });
    case "export_started": return t("ev.export", { from: clock(e.from), to: clock(e.to) });
    case "exported": return t("ev.exported", { secs: e.secs.toFixed(1), render: Math.round(e.render_secs) });
    case "retrying": return t("ev.retry", { reason: e.reason });
    case "clip_done": return t("ev.clipDone", { path: e.path, secs: e.secs.toFixed(1) });
    case "compilation_done": return t("ev.compiled", { path: e.path, secs: e.secs.toFixed(1) });
    case "restoring_window_size": return t("ev.restoringSize", { w: e.width, h: e.height });
    case "window_size_restored": return t("ev.sizeRestored", { w: e.width, h: e.height });
    case "window_size_not_restored": return t("ev.sizeNotRestored", { w: e.width, h: e.height, error: e.error });
    case "restore_failed": return t("ev.restoreFailed", { error: e.error });
    case "diagnostics_kept": return t("ev.diag", { dir: e.dir });
    default: return null;
  }
}

/** 进度页。`clips` 是要录的片段序号；`back` 回比赛页 */
export async function showRecording(view, dialog, { matchId, player, clips, back }) {
  const step = el("div", { class: "step" }, t("rec.starting"));
  const clipLine = el("div", { class: "muted" });
  const bar = el("div", { class: "bar-fill" });
  const barBox = el("div", { class: "bar", hidden: true }, bar);
  const log = el("ol", { class: "log" });
  const stopBtn = el("button", { type: "button", onclick: () => api.stopRecording() }, t("rec.stop"));
  const body = el("div", { class: "card" },
    el("div", { class: "row" }, el("h2", { style: "margin:0" }, t("rec.title")), el("div", { class: "spacer" }), stopBtn),
    el("p", { class: "warn" }, t("rec.handsOff", { hotkey: "Ctrl+Shift+F12" })),
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
    listen("record-confirm", async (q) => api.answerConfirm(q.id, await ask(dialog, q.question))),
    listen("record-finished", (f) => {
      clearInterval(timer);
      unlisten.forEach((u) => u());
      showResult(view, f, { matchId, player, back, dialog });
    }),
  ]);

  try {
    await api.startRecording(matchId, player, clips);
  } catch (e) {
    clearInterval(timer);
    unlisten.forEach((u) => u());
    view.replaceChildren(
      el("button", { type: "button", onclick: back }, t("nav.back")),
      el("div", { class: "card error" }, e.message),
    );
  }
}

function showResult(view, f, { matchId, player, back, dialog }) {
  const left = f.selected.filter((i) => !f.completed.includes(i));
  let headline, cls = "";
  if (f.error) { headline = t("rec.result.error", { error: f.error }); cls = "error"; }
  else if (f.cancelled) headline = t("rec.result.stopped", { reason: f.cancelled });
  else headline = t("rec.result.done", { n: f.files.length, minutes: (f.elapsed_secs / 60).toFixed(1) });

  const files = f.files.map((p) => el("li", {},
    el("span", {}, p.split(/[\\/]/).pop()), " ",
    el("button", { type: "button", class: "link", onclick: () => api.reveal(p) }, t("rec.result.show")),
  ));

  view.replaceChildren(el("div", { class: "card" },
    el("h2", {}, t("rec.result.title")),
    el("p", { class: cls }, headline),
    f.files.length ? el("ul", { class: "files" }, files) : null,
    el("div", { class: "row", style: "margin-top:12px" },
      el("button", { type: "button", onclick: back }, t("rec.result.back")),
      el("button", { type: "button", onclick: () => api.reveal(f.out_dir) }, t("rec.result.folder")),
      left.length
        ? el("button", {
          type: "button",
          class: "primary",
          onclick: () => showRecording(view, dialog, { matchId, player, clips: left, back }),
        }, t("rec.result.resume", { n: left.length }))
        : null,
    ),
  ));
}
