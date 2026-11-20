// 录制：开始前确认 → 进度（事件来自 Rust 的 pipeline）→ 录完进度页原地变成结果（2026-11-17 用户定：
// 原来录完跳到"已录视频"、顶上显示结果卡片，用户觉得不好，改成在这个页面的基础上改文字）。
// 录制中可以切到别的页面，回到进度页时按保存的状态重画
// Dota 放在最前面录，发给它的点击和按键被拦下；Dota 左上角有置顶的进度窗（overlay.js），那里和这里都能停止
import * as api from "./api.js";
import { resolve, t } from "./i18n.js";
import { clock, el, heroIcon, heroName, heroPic, length, span } from "./format.js";
import { describe } from "./events.js";
import { settings } from "./state.js";

const listen = (name, fn) => window.__TAURI__.event.listen(name, (e) => fn(e.payload));

/** 开始前的确认框，返回用户是否点了开始 */
export function confirmStart(dialog, { count, secs, minutes, overwrite }) {
  return new Promise((done) => {
    const close = (ok) => { dialog.close(); done(ok); };
    const native = settings.capture === "native";
    // 2026-11-20 起：一句录多少、多久，下面几条短要点。原来是四段话，最要紧的「录制时先别用电脑」埋在中间
    dialog.replaceChildren(
      el("h2", {}, t("rec.confirm.title")),
      el("p", { class: "confirm-what" }, t("rec.confirm.what", { n: count, secs: length(secs), minutes })),
      el("ul", { class: "confirm-points" },
        el("li", { class: "key" }, t("rec.confirm.window")),
        el("li", {}, t("rec.confirm.block")),
        el("li", {}, t(native ? "rec.confirm.restartNative" : "rec.confirm.restart1080"))),
      // replaceChildren 会把 null 写成文字 "null"，没有就放空字符串
      overwrite ? el("p", { class: "muted" }, t("rec.confirm.overwrite", { n: overwrite })) : "",
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
//   step, bar（这一段的进度 0–1；null = 还没开始录）, frames: [已录帧, 共几帧], log: [文字], stopping,
//   result（录完后 Rust 发来的结果；有它就是录完了，进度页显示结果） }
let rec = null;
/** 当前进度页的刷新函数（切走后它的元素已经不在文档里，调了也不显示） */
let page = null;

/** 正在录的比赛 ID；没在录（包括已经录完）返回 null */
export function recordingMatch() {
  return rec && !rec.result ? rec.matchId : null;
}

/** 录完的结果是哪种：录完 / 停止 / 出错 */
const resultKind = (f) => (f.error ? "error" : f.cancelled ? "stopped" : "done");

/** 开始录制：`clips` 是要录的片段（plan 返回的行），`camera` 是比赛页上选的镜头。
 * 录制结束调 `onFinished(结果)`；没能开始调 `onFailed(原因)` */
export async function startRecording(dialog, { matchId, player, hero, clips, products = [], camera, compile, onFinished, onFailed }) {
  const mine = { matchId, hero, clips, current: null, done: new Set(), started: null, step: t("rec.starting"), bar: null, frames: null, log: [], stopping: false };
  rec = mine;
  const update = () => page?.();
  let poll = null;
  // 录完：保留这次的状态，进度页原地改成结果。`record-finished` 和下面的查询谁先到用谁，只处理一次
  const finish = (f, how) => {
    if (mine.result) return;
    clearInterval(poll);
    unlisten.forEach((u) => u());
    api.writeLog("界面", `${how}：比赛 ${f.match_id}`).catch(() => {});
    mine.result = f;
    mine.current = null;
    update();
    onFinished(f);
  };
  const unlisten = await Promise.all([
    // 预计要录多久（秒，从开始录算，不含启动游戏），横幅上算还要多久用
    listen("record-started", (s) => {
      rec.estimated = s.estimated_secs;
      update();
    }),
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
    listen("record-finished", (f) => finish(f, "收到录完")),
  ]);

  try {
    await api.startRecording(matchId, player, clips.map((c) => c.index), products, camera, compile, resolve(settings.language));
    // 保底：每 3 秒问一次后台还在不在录，不录了就直接取结果，免得 `record-finished` 万一没送到就一直停在「录制中」。
    // 2026-11-18/19 页面要点一下窗口才收到「录完了」：是录完后又切了一次前台，之后页面和后台之间的消息
    // （包括这里的查询）都卡住，2026-11-19 起桌面应用录完不再切（`Options::return_focus`）
    poll = setInterval(async () => {
      if (mine.result || (await api.isRecording().catch(() => true))) return;
      const f = await api.recordResult().catch(() => null);
      if (f?.match_id === matchId) finish(f, "没收到录完的消息，查询后才知道录完了");
    }, 3000);
    if (mine.result) clearInterval(poll);
  } catch (e) {
    unlisten.forEach((u) => u());
    rec = null;
    page = null;
    onFailed(e.message);
  }
}

/** 进度页（2026-11-11 改版）：上面是正在录的片段（英雄大图、时间、被杀英雄、这一段的进度），
 * 下面一排小卡片是全部片段的状态，详细记录折叠起来。按 `rec` 画，录制状态变了就重画会变的部分。
 * 录完原地改文字：标题、中间大字换成结果，「停止录制」收起，左上角出现「← 已录视频」（`onVideos`）——
 * 和比赛页的返回一样固定在顶部，鼠标侧键也能用（2026-11-17 用户定：原来右上的蓝色「查看视频」按钮位置和说法都怪） */
export function showRecording(view, { onVideos }) {
  const title = el("h1", {}, t("rec.title"));
  const meta = el("span", { class: "meta" });
  const stopBtn = el("button", {
    type: "button",
    onclick: () => { stopBtn.disabled = true; api.stopRecording(); },
  }, t("rec.stop"));
  const backBar = el("div", { class: "back-bar", hidden: true }, el("button", { type: "button", class: "back", onclick: onVideos }, t("nav.backVideos")));
  // 录完后鼠标侧键的"后退"也去已录视频；切走后（页面元素不在文档里）就卸掉
  const onMouseBack = (e) => {
    if (!backBar.isConnected) {
      document.removeEventListener("mouseup", onMouseBack);
      return;
    }
    if (e.button === 3 && !backBar.hidden) {
      e.preventDefault();
      document.removeEventListener("mouseup", onMouseBack);
      onVideos();
    }
  };
  document.addEventListener("mouseup", onMouseBack);
  const warn = el("p", { class: "warn" });
  const lbl = el("span", { class: "lbl" });
  const big = el("span", { class: "big" });
  const victims = el("span", { class: "hero-icons" });
  const fill = el("div", { class: "bar-fill" });
  const barBox = el("div", { class: "bar", hidden: true }, fill);
  const pct = el("div", { class: "pct" });
  const note = el("div", { class: "note", hidden: true });
  const steps = el("div", { class: "rec-steps" });
  const log = el("ol", { class: "log" });
  const logTitle = el("summary", {});
  view.replaceChildren(
    backBar,
    el("div", { class: "page-head" }, title, meta, el("div", { class: "spacer" }), stopBtn),
    warn,
    el("div", { class: "card" }, el("div", { class: "rec-now" },
      rec ? heroPic(rec.hero) : "",
      el("div", { class: "what" }, lbl, big, victims, barBox, pct, note),
    )),
    el("div", { class: "card" }, el("h2", {}, t("rec.allClips")), el("p", { class: "sub" }, t("rec.allClipsHint")), steps),
    el("details", { class: "card rec-log" }, logTitle, log),
  );
  // 录完：标题、按钮、中间大字换成结果；下面的片段卡片和详细记录照旧
  const finished = (f) => {
    const kind = resultKind(f);
    title.textContent = t(`rec.title.${kind}`);
    meta.textContent = [String(rec.matchId), heroName(rec.hero), t("rec.used", { time: clock(f.elapsed_secs) })].join(" · ");
    stopBtn.hidden = true;
    backBar.hidden = false;
    warn.hidden = true;
    lbl.textContent = "";
    big.className = `big result-${kind}`;
    big.style.fontSize = "24px";
    big.textContent = {
      // 做了成品（集锦、时刻）就说生成了几个视频，单段拼完已经删了
      done: f.products ? t("rec.result.doneProducts", { n: f.products }) : t("rec.result.done", { n: f.completed.length }),
      stopped: t("rec.result.stopped", { reason: f.cancelled }),
      error: t("rec.result.error"),
    }[kind];
    victims.replaceChildren();
    barBox.hidden = true;
    pct.replaceChildren();
    const sub = [t("rec.result.progress", { done: f.completed.length, total: f.selected.length, minutes: (f.elapsed_secs / 60).toFixed(1) })];
    if (kind !== "done") sub.push(t("rec.result.restored"));
    if (f.game_open) sub.push(t(kind === "done" ? "rec.result.gameOpen" : "rec.result.gameKept"));
    note.hidden = false;
    note.replaceChildren(
      el("p", { class: "muted" }, sub.join(" ")),
      f.error ? el("p", { class: "error" }, f.error) : "",
      f.history_error ? el("p", { class: "error" }, t("rec.result.historyError", { error: f.history_error })) : "",
    );
  };
  // 录制中：标题旁的进度、正在录的片段和这一段的进度条
  const progress = () => {
    const total = rec.clips.length;
    const n = rec.current == null ? 0 : rec.clips.findIndex((c) => c.index === rec.current) + 1;
    const parts = [String(rec.matchId), heroName(rec.hero)];
    if (n) parts.push(t("rec.nth", { n, total }));
    if (rec.started) parts.push(t("rec.elapsed", { time: clock((Date.now() - rec.started) / 1000) }));
    meta.textContent = parts.join(" · ");
    stopBtn.disabled = rec.stopping;
    // 预计时长还没报来（刚点开始的一瞬间）就先不写还要多久
    warn.textContent = rec.estimated == null ? t("rec.windowNoTime") : t("rec.window", { left: timeLeft() });

    const c = rec.clips.find((x) => x.index === rec.current);
    if (c && !rec.stopping) {
      lbl.textContent = t("rec.nowClip", { n: c.index });
      big.textContent = span(c.start, c.end);
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
  };
  // 横幅上的"还要多久"：预计录制时长减去已经录了多久（还没开始录就是整个预计时长，不含启动游戏），按分钟往上取整
  const timeLeft = () => {
    const left = rec.estimated - (rec.started ? (Date.now() - rec.started) / 1000 : 0);
    return left < 30 ? t("rec.left.soon") : t("rec.left.min", { n: Math.max(1, Math.ceil(left / 60)) });
  };
  // 跳转、切镜头的十几秒里没有进度事件，每秒自己刷新一次，"已用"和"还要多久"才会走；录完或切走就停
  const tick = setInterval(() => {
    if (!big.isConnected || !rec || rec.result) clearInterval(tick);
    else progress();
  }, 1000);
  page = () => {
    if (!rec || !big.isConnected) return;
    if (rec.result) finished(rec.result);
    else progress();
    // 录完后没录到的片段写「未录」，不再是「等待」
    const waiting = rec.result ? "rec.st.skipped" : "rec.st.wait";
    steps.replaceChildren(...rec.clips.map((x) => {
      const state = rec.done.has(x.index) ? "done" : x.index === rec.current ? "cur" : "";
      return el("div", { class: `rec-step ${state}` },
        el("span", { class: "tr" }, span(x.start, x.end)),
        el("span", { class: "hero-icons" }, x.victims.map((v) => heroIcon(v, true))),
        el("span", { class: "st" }, t(state === "done" ? "rec.st.done" : state === "cur" ? "rec.st.cur" : waiting)),
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
