// 录制：开始前确认 → 进度（事件来自 Rust 的 pipeline）→ 结果。录制中途要用户拍板的事弹对话框
import * as api from "./api.js";
import { t } from "./i18n.js";
import { clock, el, heroName, length } from "./format.js";
import { describe } from "./events.js";

const listen = (name, fn) => window.__TAURI__.event.listen(name, (e) => fn(e.payload));

/** 开始前的确认框，返回用户是否点了开始 */
export function confirmStart(dialog, { count, secs, minutes }) {
  return new Promise((resolve) => {
    const done = (ok) => { dialog.close(); resolve(ok); };
    dialog.replaceChildren(
      el("h2", {}, t("rec.confirm.title")),
      el("p", {}, t("rec.confirm.what", { n: count, secs: length(secs), minutes })),
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

/** 进度页。`clips` 是要录的片段（plan 返回的行，含序号、时间、被杀英雄）；`back` 回比赛页 */
export async function showRecording(view, dialog, { matchId, player, clips, back }) {
  const step = el("div", { class: "step" }, t("rec.starting"));
  const clipLine = el("div", { class: "muted" });
  const bar = el("div", { class: "bar-fill" });
  const barBox = el("div", { class: "bar", hidden: true }, bar);
  const log = el("ol", { class: "log" });
  const stopBtn = el("button", { type: "button", onclick: () => api.stopRecording() }, t("rec.stop"));
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
    el("div", { class: "row" }, el("h2", { style: "margin:0" }, t("rec.title")), el("div", { class: "spacer" }), stopBtn),
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
  // 每个做好的片段：序号 → 文件路径
  const outputs = new Map();

  const unlisten = await Promise.all([
    listen("record-event", (e) => {
      const text = describe(e);
      if (text) log.append(el("li", {}, text));
      if (e.kind === "clip_started") clipLine.textContent = text;
      else if (e.kind === "export_started") startBar(e.estimated_secs + 15);
      else if (e.kind === "exported") stopBar();
      else if (e.kind === "clip_done") outputs.set(e.index, e.path);
      if (text && e.kind !== "clip_started") step.textContent = text;
    }),
    listen("record-paused", () => {
      pausedBox.hidden = false;
      handsOff.hidden = true;
      resumeBtn.disabled = false;
      stopBtn.hidden = true;
    }),
    listen("record-resumed", () => {
      pausedBox.hidden = true;
      handsOff.hidden = false;
      stopBtn.hidden = false;
    }),
    listen("record-stopping", (reason) => {
      clearInterval(timer);
      pausedBox.hidden = true;
      step.textContent = t("ov.stoppingWhy", { reason });
      stopBtn.disabled = true;
    }),
    listen("record-confirm", async (q) => {
      const yes = await ask(dialog, q.question, q.id);
      if (yes !== null) api.answerConfirm(q.id, yes);
    }),
    listen("record-confirm-done", (id) => { if (asking?.id === id) asking.close(); }),
    listen("record-finished", (f) => {
      clearInterval(timer);
      unlisten.forEach((u) => u());
      showResult(view, f, { matchId, player, back, dialog, clips, outputs });
    }),
  ]);

  try {
    await api.startRecording(matchId, player, clips.map((c) => c.index));
  } catch (e) {
    clearInterval(timer);
    unlisten.forEach((u) => u());
    view.replaceChildren(
      el("button", { type: "button", onclick: back }, t("nav.back")),
      el("div", { class: "card error" }, e.message),
    );
  }
}

function showResult(view, f, { matchId, player, back, dialog, clips, outputs }) {
  const left = clips.filter((c) => !f.completed.includes(c.index));
  const kind = f.error ? "error" : f.cancelled ? "stopped" : "done";
  const title = {
    done: t("rec.result.done", { n: f.completed.length }),
    stopped: t("rec.result.stopped", { reason: f.cancelled }),
    error: t("rec.result.error"),
  }[kind];
  const sub = [t("rec.result.progress", { done: f.completed.length, total: f.selected.length, minutes: (f.elapsed_secs / 60).toFixed(1) })];
  if (kind !== "done") sub.push(t("rec.result.restored"));
  if (f.game_open) sub.push(t(kind === "done" ? "rec.result.gameOpen" : "rec.result.gameKept"));

  const rows = clips.map((c) => {
    const path = outputs.get(c.index);
    return el("tr", {},
      el("td", { class: "num" }, String(c.index)),
      el("td", {}, `${clock(c.start)} – ${clock(c.end)}`),
      el("td", { class: "wrap" }, c.victims.map(heroName).join(", ")),
      el("td", {}, path
        ? el("span", {}, el("span", { class: "ok" }, `✓ ${t("rec.result.clipDone")}`), " ",
          el("button", { type: "button", class: "link", onclick: () => api.reveal(path) }, t("rec.result.show")))
        : el("span", { class: "muted" }, t("rec.result.clipLeft"))),
    );
  });
  // 合集是 files 里不属于任何片段的那个
  const compilation = f.files.find((p) => ![...outputs.values()].includes(p));

  const quitBtn = f.game_open
    ? el("button", {
      type: "button",
      onclick: async () => {
        quitBtn.disabled = true;
        try { await api.quitGame(); quitBtn.textContent = t("rec.result.quitSent"); } catch { quitBtn.disabled = false; }
      },
    }, t("rec.result.quit"))
    : null;

  view.replaceChildren(el("div", { class: `card result ${kind}` },
    el("h2", {}, title),
    el("p", { class: "muted" }, sub.join(" ")),
    f.error ? el("p", { class: "error" }, f.error) : null,
    el("table", {},
      el("thead", {}, el("tr", {},
        el("th", { class: "num" }, "#"), el("th", {}, t("match.col.time")),
        el("th", {}, t("match.col.kills")), el("th", {}, t("rec.result.status")),
      )),
      el("tbody", {}, rows),
    ),
    compilation ? el("p", {}, t("rec.result.compilation"), " ", compilation.split(/[\\/]/).pop(), " ",
      el("button", { type: "button", class: "link", onclick: () => api.reveal(compilation) }, t("rec.result.show"))) : null,
    left.length ? el("p", { class: "warn" }, t("rec.result.resumeHint")) : null,
    el("div", { class: "row", style: "margin-top:12px" },
      left.length
        ? el("button", {
          type: "button",
          class: "primary",
          onclick: () => showRecording(view, dialog, { matchId, player, clips: left, back }),
        }, t("rec.result.resume", { n: left.length }))
        : null,
      el("button", { type: "button", onclick: () => api.reveal(f.out_dir) }, t("rec.result.folder")),
      quitBtn,
      el("div", { class: "spacer" }),
      el("button", { type: "button", onclick: back }, t("rec.result.back")),
    ),
  ));
}
