// 首页：本地录像（游戏 replays 目录里的 .dem），顶上可以按比赛 ID 下载（分阶段显示进度，可以取消）。
// 不查 OpenDota 的玩家比赛列表：它会落后几个月（2026-10-27 实测），按比赛查才可靠
import * as api from "./api.js";
import { t } from "./i18n.js";
import { clock, date, el, heroIcon, heroName, heroPic } from "./format.js";

// 卡片还是列表：记在本机浏览器存储里（只是个人习惯，丢了就回到卡片）
const VIEW_KEY = "d2hr.homeView";
function savedView() {
  try { return localStorage.getItem(VIEW_KEY) === "list" ? "list" : "cards"; } catch { return "cards"; }
}
function saveView(v) {
  try { localStorage.setItem(VIEW_KEY, v); } catch { /* 存不了就算了 */ }
}

/** 下载失败（Rust 侧 DownloadError）→ 一句话 */
function downloadError(e, id) {
  switch (e?.kind) {
    case "no_match": return t("home.err.noMatch", { id });
    case "no_url": return t("home.err.noUrl");
    case "expired": return t("home.err.expired");
    case "busy": return t("home.err.busy");
    case "cancelled": return t("home.dl.cancelled");
    default: return t("home.err.other", { error: e?.message ?? String(e) });
  }
}

/** 删除录像前确认（一场或多场），返回是否点了删除 */
function confirmDelete(dialog, ids) {
  return new Promise((resolve) => {
    const done = (ok) => { dialog.close(); resolve(ok); };
    dialog.replaceChildren(
      el("h2", {}, ids.length === 1 ? t("home.delete.title", { id: ids[0] }) : t("home.delete.titleMany", { n: ids.length })),
      el("p", {}, t("home.delete.what")),
      el("p", { class: "muted" }, t("home.delete.keep")),
      el("div", { class: "dialog-actions" },
        el("button", { type: "button", onclick: () => done(false) }, t("settings.cancel")),
        el("button", { type: "button", class: "primary", onclick: () => done(true) }, t("home.delete.ok")),
      ),
    );
    dialog.addEventListener("cancel", () => resolve(false), { once: true });
    dialog.showModal();
  });
}

// 正在下载（或者下完了、用户切走了还没回来看）的那一场。下载不跟着页面走：切到别的页面照常下，
// 回到首页接着显示进度；切走期间下完或失败了，回来显示结果（2026-11-10 实测：切走再回来"下载就消失了"，
// 那次其实是连 OpenDota 断了线、下载失败，错误没地方显示）。
// { id, progress（Rust 侧 DownloadProgress）, speed, last, outcome（undefined = 还在下；{ ok } 或 { error }） }
let job = null;
/** 当前首页的刷新函数（切走后它的元素已经不在文档里，调了也不显示） */
let page = null;
let listening = null;

function listenProgress() {
  listening ??= window.__TAURI__.event.listen("download-progress", (e) => {
    const p = e.payload;
    if (!job || job.outcome || String(p.match_id) !== job.id) return;
    // 速度按最近的平均算，免得跳得太厉害
    if (p.stage === "downloading") {
      const now = Date.now();
      if (job.last && now > job.last.at) job.speed = job.speed * 0.7 + 0.3 * ((p.done - job.last.done) / ((now - job.last.at) / 1000));
      job.last = { at: now, done: p.done };
    }
    job.progress = p;
    page?.(false);
  });
  return listening;
}

async function startDownload(id) {
  await listenProgress();
  const mine = { id, progress: { stage: "finding_url", match_id: id }, speed: 0, last: null };
  job = mine;
  page?.(false);
  try {
    await api.downloadReplay(Number(id));
    mine.outcome = { ok: true };
  } catch (e) {
    mine.outcome = { error: e };
  }
  page?.(true);
}

export async function showHome(view, dialog, { openMatch }) {
  const idInput = el("input", { type: "text", placeholder: t("list.idPlaceholder"), inputmode: "numeric" });
  const idBtn = el("button", { type: "button" }, t("home.openId"));
  const idMsg = el("span", { class: "muted" });
  const cancelBtn = el("button", { type: "button", hidden: true }, t("home.dl.cancel"));
  const openDone = el("button", { type: "button", class: "primary", hidden: true }, t("home.dl.open"));
  const fill = el("div", { class: "bar-fill" });
  const bar = el("div", { class: "bar", hidden: true, style: "max-width:520px" }, fill);
  const tableBox = el("div", {}, el("div", { class: "loading" }, "…"));
  // 刷新：列表留在原地变淡，按钮上转圈，读完再换
  const refreshBtn = el("button", { type: "button" }, el("span", { class: "spin", "aria-hidden": "true" }), t("list.refresh"));
  let rows = [];

  const say = (cls, text) => { idMsg.className = cls; idMsg.textContent = text; };

  /** 按 job 显示下载状态。`justFinished`：下载正好在这时结束、用户就在首页上——录像下好了就直接打开比赛页；
   * 否则（用户是切回来才看到结果）只显示"下好了"和「打开」，不自动跳页 */
  const render = (justFinished) => {
    if (!idInput.isConnected) return;
    const running = job && !job.outcome;
    idInput.disabled = idBtn.disabled = Boolean(running);
    cancelBtn.hidden = !running;
    openDone.hidden = true;
    bar.hidden = true;
    if (!job) return;
    const id = job.id;
    if (running) {
      const p = job.progress;
      if (p.stage === "finding_url") say("muted", t("home.dl.finding", { id }));
      else if (p.stage === "waiting_for_url") say("muted", t("home.dl.waiting", { secs: p.waited_secs, max: p.max_secs }));
      else if (p.stage === "saving") say("muted", t("home.dl.saving"));
      else if (p.stage === "downloading") {
        const mb = (n) => (n / 1e6).toFixed(1);
        const vars = { done: mb(p.done), total: p.total ? mb(p.total) : "", speed: mb(Math.max(0, job.speed)) };
        say("muted", t(p.total ? "home.dl.progress" : "home.dl.progressNoTotal", vars));
        bar.hidden = !p.total;
        if (p.total) fill.style.width = `${Math.min(100, (p.done / p.total) * 100)}%`;
      }
      return;
    }
    // 下完了或失败了：显示一次就清掉
    const { outcome } = job;
    job = null;
    if (outcome.ok && justFinished) return openMatch(Number(id));
    if (outcome.ok) {
      say("muted", t("home.dl.done", { id }));
      openDone.hidden = false;
      openDone.onclick = () => openMatch(Number(id));
    } else {
      say(outcome.error?.kind === "cancelled" ? "muted" : "error", downloadError(outcome.error, id));
    }
  };
  page = render;
  cancelBtn.addEventListener("click", () => {
    if (!job) return;
    cancelBtn.disabled = true;
    api.cancelDownload(Number(job.id));
  });
  cancelBtn.disabled = false;

  /** 本地有就打开；没有就下载（经 OpenDota 拿录像地址），下完打开 */
  const openTyped = () => {
    const id = idInput.value.trim();
    if (!/^\d+$/.test(id)) return say("error", t("home.badId"));
    if (rows.some((r) => String(r.match_id) === id && !r.error)) return openMatch(Number(id));
    cancelBtn.disabled = false;
    fill.style.width = "0%";
    startDownload(id);
  };
  idBtn.addEventListener("click", openTyped);
  idInput.addEventListener("keydown", (e) => e.key === "Enter" && openTyped());

  // 录像变成卡片（2026-11-11 改版）：大图是你玩的英雄，角上标胜负、已录几段
  const countLine = el("span", { class: "meta" });
  const batchBtn = el("button", { type: "button", class: "danger", hidden: true });
  // 全选 / 全不选（2026-11-12 用户要的，批量删除用），放在工具栏最左边：全勾上了就变成「全不选」，和比赛页的片段一样
  const allBtn = el("button", { type: "button", hidden: true });
  let viewMode = savedView();
  const views = ["cards", "list"];
  const viewButtons = views.map((v) => el("button", { type: "button", role: "radio", onclick: () => setView(v) }, t(`home.view.${v}`)));
  const setView = (v) => {
    viewMode = v;
    saveView(v);
    viewButtons.forEach((b, i) => { b.classList.toggle("on", views[i] === v); b.setAttribute("aria-checked", String(views[i] === v)); });
    if (rows.length) renderTable();
  };
  viewButtons.forEach((b, i) => b.classList.toggle("on", views[i] === viewMode));
  view.replaceChildren(
    el("div", { class: "page-head" }, el("h1", {}, t("list.title")), countLine),
    el("div", { class: "row", style: "margin-bottom:6px" }, idInput, idBtn, cancelBtn, idMsg, openDone),
    bar,
    el("div", { class: "row", style: "margin:16px 0 12px" },
      allBtn,
      el("div", { class: "spacer" }),
      batchBtn,
      el("div", { class: "segmented", role: "radiogroup", "aria-label": t("home.view") }, viewButtons),
      refreshBtn,
    ),
    tableBox,
    el("p", { class: "muted", style: "font-size:12.5px;margin-top:18px" }, t("home.where")),
  );
  if (job) {
    idInput.value = job.id;
    render(false);
  }

  let recorded = new Map();
  let picked = new Set();

  /** 读录像列表和已录段数，读不了就在列表位置显示错误、返回 false。打开页面和点「刷新」都走这里 */
  async function load() {
    try {
      const [list, history] = await Promise.all([api.listReplays(), api.recordedMatches().catch(() => [])]);
      rows = list;
      // 已录几段：从"已录视频"的记录里数（文件还在的才算）
      recorded = new Map();
      for (const m of history) {
        const n = m.players.reduce((a, p) => a + p.clips.filter((c) => c.file?.exists).length, 0);
        if (n) recorded.set(m.match_id, n);
      }
    } catch (e) {
      rows = [];
      tableBox.replaceChildren(el("p", { class: "error" }, e.message));
      return false;
    }
    // 比赛开始时间新的在前；读不了的录像按下载时间排
    rows.sort((a, b) => (b.start_time ?? b.modified) - (a.start_time ?? a.modified));
    // 勾着的录像要是已经不在了（在游戏里删了），去掉
    for (const id of picked) if (!rows.some((r) => r.match_id === id)) picked.delete(id);
    return true;
  }

  refreshBtn.addEventListener("click", async () => {
    refreshBtn.disabled = true;
    refreshBtn.classList.add("busy");
    tableBox.classList.add("stale");
    // 读过的录像有缓存，刷新很快；至少转 0.4 秒，看得出刷新过
    const [ok] = await Promise.all([load(), new Promise((r) => setTimeout(r, 400))]);
    refreshBtn.disabled = false;
    refreshBtn.classList.remove("busy");
    tableBox.classList.remove("stale");
    if (ok && tableBox.isConnected) renderTable();
  });

  /** 删掉这些录像（移到回收站），成功后从列表里去掉 */
  const remove = async (ids) => {
    if (!ids.length || !(await confirmDelete(dialog, ids))) return;
    try {
      await api.deleteReplays(ids);
      rows = rows.filter((r) => !ids.includes(r.match_id));
      for (const id of ids) picked.delete(id);
      say("muted", ids.length === 1 ? t("home.deleted", { id: ids[0] }) : t("home.deletedMany", { n: ids.length }));
      renderTable();
    } catch (err) {
      say("error", err.message);
    }
  };
  batchBtn.addEventListener("click", () => remove([...picked]));
  allBtn.addEventListener("click", () => {
    if (rows.every((r) => picked.has(r.match_id))) picked.clear();
    else for (const r of rows) picked.add(r.match_id);
    renderTable();
  });

  function renderTable() {
    countLine.textContent = t("home.count", { n: rows.length });
    if (!rows.length) {
      allBtn.hidden = batchBtn.hidden = true;
      tableBox.replaceChildren(el("p", { class: "muted" }, t("list.empty")));
      return;
    }
    const grid = el("div", { class: viewMode === "list" ? "match-rows" : "match-grid" });
    const sync = () => {
      batchBtn.hidden = picked.size === 0;
      batchBtn.textContent = t("home.delete.picked", { n: picked.size });
      allBtn.hidden = rows.length === 0;
      allBtn.textContent = t(rows.every((r) => picked.has(r.match_id)) ? "match.selectNone" : "match.selectAll");
      grid.classList.toggle("picking", picked.size > 0);
    };
    for (const r of rows) {
      const mine = !r.error && r.my_hero;
      // 勾选框悬停才出现，点它不打开比赛；勾了任意一个之后全都显示，方便连着勾
      const box = el("input", { type: "checkbox", class: "pick", title: t("home.pick") });
      box.checked = picked.has(r.match_id);
      const result = mine ? el("span", { class: `res ${r.my_won ? "win" : "lose"}` }, t(r.my_won ? "list.win" : "list.lose")) : null;
      const heroLine = mine ? heroName(r.my_hero) : r.error ? el("span", { class: "error", title: r.error }, t("list.unreadable")) : t("home.notMe");
      // 我的击杀 / 死亡 / 助攻（终场计分板）；不在这场就空着
      const kda = mine && r.my_score ? `${r.my_score.kills} / ${r.my_score.deaths} / ${r.my_score.assists}` : "";
      // 列表：一行一场，小图标
      const card = viewMode === "list"
        ? el("div", { class: `match-row${box.checked ? " picked" : ""}`, onclick: () => openMatch(r.match_id) },
          box,
          mine ? heroIcon(r.my_hero) : el("span", { class: "hero-icon hero-fallback" }, "?"),
          el("span", { class: "id" }, String(r.match_id)),
          el("span", { class: "hero" }, heroLine),
          el("span", { class: "num kda", title: kda && t("home.kda") }, kda),
          el("span", {}, result),
          el("span", { class: "num muted" }, r.duration == null ? "—" : clock(r.duration)),
          el("span", { class: "num muted" }, r.start_time ? date(r.start_time) : ""),
          el("span", {}, recorded.has(r.match_id) ? el("span", { class: "rec" }, t("home.recorded", { n: recorded.get(r.match_id) })) : ""))
        : el("div", { class: `match-card${box.checked ? " picked" : ""}`, onclick: () => openMatch(r.match_id) },
        el("div", { class: "pic" },
          mine ? heroPic(r.my_hero) : r.error ? el("div", { class: "hero-pic hero-fallback" }, t("list.unreadable")) : el("div", { class: "hero-pic hero-fallback unknown" }, "?"),
          mine ? el("span", { class: `res ${r.my_won ? "win" : "lose"}` }, t(r.my_won ? "list.win" : "list.lose")) : null,
          recorded.has(r.match_id) ? el("span", { class: "rec" }, t("home.recorded", { n: recorded.get(r.match_id) })) : null,
        ),
        el("div", { class: "info" },
          el("div", {},
            el("div", { class: "id" }, String(r.match_id)),
            el("div", { class: "hero" }, mine ? heroName(r.my_hero) : r.error ? el("span", { class: "error", title: r.error }, t("list.unreadable")) : t("home.notMe"),
              kda ? el("span", { class: "kda", title: t("home.kda") }, kda) : null),
          ),
          el("div", { class: "stats" }, r.duration == null ? "—" : clock(r.duration), el("br"), r.start_time ? date(r.start_time) : ""),
        ),
        box,
      );
      if (viewMode === "list") card.prepend(box);
      box.addEventListener("click", (e) => e.stopPropagation());
      box.addEventListener("change", () => {
        box.checked ? picked.add(r.match_id) : picked.delete(r.match_id);
        card.classList.toggle("picked", box.checked);
        sync();
      });
      grid.append(card);
    }
    tableBox.replaceChildren(grid);
    sync();
  }
  if (await load()) renderTable();
}
