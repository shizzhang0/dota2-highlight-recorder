// 首页：本地录像（游戏 replays 目录里的 .dem），顶上可以按比赛 ID 下载。不查 OpenDota 的玩家比赛列表：
// 它会落后几个月（2026-10-27 实测），按比赛查才可靠
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

export async function showHome(view, dialog, { openMatch }) {
  const idInput = el("input", { type: "text", placeholder: t("list.idPlaceholder"), inputmode: "numeric" });
  const idBtn = el("button", { type: "button" }, t("home.openId"));
  const idMsg = el("span", { class: "muted" });
  const tableBox = el("div", {}, el("div", { class: "loading" }, "…"));
  let rows = [];

  const say = (cls, text) => { idMsg.className = cls; idMsg.textContent = text; };

  /** 本地有就打开；没有就下载（经 OpenDota 拿录像地址），下完打开 */
  const openTyped = async () => {
    const id = idInput.value.trim();
    if (!/^\d+$/.test(id)) return say("error", t("home.badId"));
    if (rows.some((r) => String(r.match_id) === id && !r.error)) return openMatch(Number(id));
    idInput.disabled = idBtn.disabled = true;
    say("muted", t("home.downloading", { id }));
    try {
      await api.downloadReplay(Number(id));
      openMatch(Number(id));
    } catch (e) {
      say("error", downloadError(e, id));
    } finally {
      idInput.disabled = idBtn.disabled = false;
    }
  };
  idBtn.addEventListener("click", openTyped);
  idInput.addEventListener("keydown", (e) => e.key === "Enter" && openTyped());

  view.replaceChildren(el("div", { class: "card" },
    el("div", { class: "row", style: "margin-bottom:6px" },
      el("h2", { style: "margin:0" }, t("list.title")),
      el("div", { class: "spacer" }),
      el("button", { type: "button", onclick: () => showHome(view, dialog, { openMatch }) }, t("list.refresh")),
    ),
    el("div", { class: "row" }, idInput, idBtn, idMsg),
    el("p", { class: "muted" }, t("home.where")),
    tableBox,
  ));

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
