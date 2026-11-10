// 首页：本地录像（游戏 replays 目录里的 .dem），顶上可以按比赛 ID 下载（分阶段显示进度，可以取消）。
// 不查 OpenDota 的玩家比赛列表：它会落后几个月（2026-10-27 实测），按比赛查才可靠
import * as api from "./api.js";
import { t } from "./i18n.js";
import { clock, date, el, heroName } from "./format.js";

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

  view.replaceChildren(el("div", { class: "card" },
    el("div", { class: "row", style: "margin-bottom:6px" },
      el("h2", { style: "margin:0" }, t("list.title")),
      el("div", { class: "spacer" }),
      el("button", { type: "button", onclick: () => showHome(view, dialog, { openMatch }) }, t("list.refresh")),
    ),
    el("div", { class: "row" }, idInput, idBtn, cancelBtn, idMsg, openDone),
    bar,
    el("p", { class: "muted" }, t("home.where")),
    tableBox,
  ));
  if (job) {
    idInput.value = job.id;
    render(false);
  }

  try {
    rows = await api.listReplays();
  } catch (e) {
    tableBox.replaceChildren(el("p", { class: "error" }, e.message));
    return;
  }
  // 比赛开始时间新的在前；读不了的录像按下载时间排
  rows.sort((a, b) => (b.start_time ?? b.modified) - (a.start_time ?? a.modified));
  let picked = new Set();

  /** 删掉这些录像（移到回收站），成功后从表里去掉 */
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

  function renderTable() {
    if (!rows.length) {
      tableBox.replaceChildren(el("p", { class: "muted" }, t("list.empty")));
      return;
    }
    const batchBtn = el("button", { type: "button", class: "danger", onclick: () => remove([...picked]) });
    const all = el("input", { type: "checkbox", title: t("match.selectAll") });
    const sync = () => {
      batchBtn.disabled = picked.size === 0;
      batchBtn.textContent = t("home.delete.picked", { n: picked.size });
      all.checked = picked.size === rows.length;
      all.indeterminate = picked.size > 0 && picked.size < rows.length;
    };
    all.addEventListener("change", () => {
      picked = all.checked ? new Set(rows.map((r) => r.match_id)) : new Set();
      for (const b of boxes) b.checked = all.checked;
      sync();
    });
    const boxes = [];
    const body = rows.map((r) => {
      const mine = !r.error && r.my_hero;
      // 勾选框那一格点了不打开比赛
      const box = el("input", { type: "checkbox" });
      box.checked = picked.has(r.match_id);
      box.addEventListener("change", () => { box.checked ? picked.add(r.match_id) : picked.delete(r.match_id); sync(); });
      boxes.push(box);
      return el("tr", { class: "clickable", onclick: () => openMatch(r.match_id) },
        el("td", { onclick: (e) => e.stopPropagation() }, box),
        el("td", {}, String(r.match_id)),
        el("td", {}, r.start_time ? date(r.start_time) : "—"),
        el("td", {}, r.error
          ? el("span", { class: "error", title: r.error }, t("list.unreadable"))
          : mine ? heroName(r.my_hero) : el("span", { class: "muted" }, t("home.notMe"))),
        el("td", {}, mine ? el("span", { class: r.my_won ? "win" : "lose" }, t(r.my_won ? "list.win" : "list.lose")) : ""),
        el("td", { class: "num" }, r.duration == null ? "—" : clock(r.duration)),
        el("td", { class: "muted" }, "›"),
      );
    });
    tableBox.replaceChildren(
      el("div", { class: "row", style: "margin-bottom:8px" }, batchBtn),
      el("table", {},
        el("thead", {}, el("tr", {},
          el("th", {}, all), el("th", {}, t("list.match")), el("th", {}, t("home.col.time")), el("th", {}, t("list.myHero")),
          el("th", {}, t("home.col.result")), el("th", { class: "num" }, t("list.duration")), el("th", {}),
        )),
        el("tbody", {}, body),
      ),
    );
    sync();
  }
  renderTable();
}
