// 录制：开始前确认 → 进度（事件来自 Rust 的 pipeline）。录完的结果在"已录视频"里看（videos.js）。
// 录制中可以切到别的页面，回到进度页时按保存的状态重画
// Dota 放在最前面录，发给它的点击和按键被拦下；Dota 左上角有置顶的进度窗（overlay.js），那里和这里都能停止
import * as api from "./api.js";
import { resolve, t } from "./i18n.js";
import { clock, el, heroIcon, heroName, heroPic, length } from "./format.js";
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
// { matchId, hero, clips（plan 的片段行）, current（正在录的片段序号）, done（录好的序号）, started（开录的时刻）,
//   step, bar（这一段的进度 0–1；null = 还没开始录）, frames: [已录帧, 共几帧], log: [文字], stopping }
let rec = null;
/** 当前进度页的刷新函数（切走后它的元素已经不在文档里，调了也不显示） */
let page = null;

/** 正在录的比赛 ID；没在录返回 null */
export function recordingMatch() {
  return rec?.matchId ?? null;
}

/** 开始录制：`clips` 是要录的片段（plan 返回的行），`camera` 是比赛页上选的镜头。
 * 录制结束调 `onFinished(结果)`；没能开始调 `onFailed(原因)` */
export async function startRecording(dialog, { matchId, player, hero, clips, camera, onFinished, onFailed }) {
  rec = { matchId, hero, clips, current: null, done: new Set(), started: null, step: t("rec.starting"), bar: null, frames: null, log: [], stopping: false };
  const update = () => page?.();
  const unlisten = await Promise.all([
    listen("record-event", (e) => {
      const text = describe(e);
      if (text) rec.log.push(text);
      if (e.kind === "recording") rec.started = Date.now();
      else if (e.kind === "clip_started") { rec.current = e.index; rec.bar = null; rec.frames = null; }
      else if (e.kind === "clip_done") rec.done.add(e.index);
      // 这一段的进度：按游戏报来的帧数走
      else if (e.kind === "segment_started") { rec.bar = 0; rec.frames = [0, e.frames]; }
      else if (e.kind === "segment_progress") { rec.bar = e.frames / e.total; rec.frames = [e.frames, e.total]; }
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

/** 进度页（2026-11-11 改版）：上面是正在录的片段（英雄大图、时间、被杀英雄、这一段的进度），
 * 下面一排小卡片是全部片段的状态，详细记录折叠起来。按 `rec` 画，录制状态变了就重画会变的部分 */
export function showRecording(view) {
  const meta = el("span", { class: "meta" });
  const stopBtn = el("button", {
    type: "button",
    onclick: () => { stopBtn.disabled = true; api.stopRecording(); },
  }, t("rec.stop"));
  const lbl = el("span", { class: "lbl" });
  const big = el("span", { class: "big" });
  const victims = el("span", { class: "hero-icons" });
  const fill = el("div", { class: "bar-fill" });
  const barBox = el("div", { class: "bar", hidden: true }, fill);
  const pct = el("div", { class: "pct" });
  const steps = el("div", { class: "rec-steps" });
  const log = el("ol", { class: "log" });
  const logTitle = el("summary", {});
  view.replaceChildren(
    el("div", { class: "page-head" }, el("h1", {}, t("rec.title")), meta, el("div", { class: "spacer" }), stopBtn),
    el("p", { class: "warn" }, t("rec.window")),
    el("div", { class: "card" }, el("div", { class: "rec-now" },
      rec ? heroPic(rec.hero) : "",
      el("div", { class: "what" }, lbl, big, victims, barBox, pct),
    )),
    el("div", { class: "card" }, el("h2", {}, t("rec.allClips")), el("p", { class: "sub" }, t("rec.allClipsHint")), steps),
    el("details", { class: "card rec-log" }, logTitle, log),
  );
  page = () => {
    if (!rec || !big.isConnected) return;
    const total = rec.clips.length;
    const n = rec.current == null ? 0 : rec.clips.findIndex((c) => c.index === rec.current) + 1;
    const parts = [String(rec.matchId), heroName(rec.hero)];
    if (n) parts.push(t("rec.nth", { n, total }));
    if (rec.started) parts.push(t("rec.elapsed", { time: clock((Date.now() - rec.started) / 1000) }));
    meta.textContent = parts.join(" · ");
    stopBtn.disabled = rec.stopping;

    const c = rec.clips.find((x) => x.index === rec.current);
    if (c && !rec.stopping) {
      lbl.textContent = t("rec.nowClip", { n: c.index });
      big.textContent = `${clock(c.start)}–${clock(c.end)}`;
      victims.replaceChildren(el("span", { class: "lbl", style: "margin-right:6px" }, t("rec.kills")), ...c.victims.map((v) => heroIcon(v)));
    } else {
      // 启动游戏、加载录像、正在停下：没有正在录的片段，大字显示现在在做什么
      lbl.textContent = "";
      big.textContent = rec.step;
      big.style.fontSize = "20px";
      victims.replaceChildren();
    }
    if (c) big.style.fontSize = "";
    barBox.hidden = rec.bar === null;
    fill.style.width = `${Math.min(100, (rec.bar ?? 0) * 100)}%`;
    pct.replaceChildren(...(rec.frames ? [
      el("span", {}, t("rec.segmentPct", { pct: Math.round((rec.bar ?? 0) * 100) })),
      el("span", {}, t("rec.frames", { done: rec.frames[0], total: rec.frames[1] })),
    ] : []));

    steps.replaceChildren(...rec.clips.map((x) => {
      const state = rec.done.has(x.index) ? "done" : x.index === rec.current ? "cur" : "";
      return el("div", { class: `rec-step ${state}` },
        el("span", { class: "tr" }, `${clock(x.start)}–${clock(x.end)}`),
        el("span", { class: "hero-icons" }, x.victims.map((v) => heroIcon(v, true))),
        el("span", { class: "st" }, t(state === "done" ? "rec.st.done" : state === "cur" ? "rec.st.cur" : "rec.st.wait")),
      );
    }));
    // 记录只追加新的几行，免得每半秒整个重画、展开状态和滚动位置跳掉
    for (const text of rec.log.slice(log.children.length)) log.append(el("li", {}, text));
    logTitle.textContent = t("rec.logN", { n: rec.log.length });
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
