// 已录视频：按比赛汇总的录制记录（Rust 侧 history.rs）。列表一行一场比赛，点进去看每个玩家录了哪些片段
import * as api from "./api.js";
import { t } from "./i18n.js";
import { baseName, clock, date, el, heroName } from "./format.js";

const playerLabel = (p) => `${heroName(p.hero)}${p.is_me ? t("match.me") : ""}`;
const unfinished = (p) => p.clips.filter((c) => !c.file);
const recorded = (p) => p.clips.filter((c) => c.file);

export async function showVideos(view, { openVideo }) {
  view.replaceChildren(el("div", { class: "loading" }, "…"));
  const list = await api.recordedMatches().catch((e) => {
    view.replaceChildren(el("div", { class: "card error" }, e.message));
    return null;
  });
  if (!list) return;
  if (!list.length) {
    view.replaceChildren(el("div", { class: "card" }, el("h2", {}, t("nav.videos")), el("p", { class: "muted" }, t("videos.empty"))));
    return;
  }
  view.replaceChildren(el("div", { class: "card" }, el("h2", {}, t("nav.videos")), el("table", {},
    el("thead", {}, el("tr", {},
      el("th", {}, t("videos.col.time")), el("th", {}, t("list.match")), el("th", {}, t("videos.col.players")),
      el("th", {}, t("match.col.status")), el("th", {}),
    )),
    el("tbody", {}, list.map((m) => {
      const done = m.players.reduce((a, p) => a + recorded(p).length, 0);
      const left = m.players.reduce((a, p) => a + unfinished(p).length, 0);
      const parts = [t("videos.done", { n: done })];
      if (m.players.some((p) => p.compilation?.exists)) parts.push(t("videos.compilation"));
      if (left) parts.push(t("videos.left", { n: left }));
      return el("tr", { class: "clickable", onclick: () => openVideo(m.match_id) },
        el("td", {}, date(m.last_time)),
        el("td", {}, String(m.match_id)),
        el("td", { class: "wrap" }, m.players.map(playerLabel).join("、")),
        el("td", { class: left ? "" : "muted" }, parts.join(" · ")),
        el("td", { class: "muted" }, "›"),
      );
    })),
  )));
}

/** 文件能做的两件事：播放、在文件夹中显示。文件不在了就没有操作（状态列已经写了） */
function fileActions(file) {
  if (!file.exists) return null;
  return el("span", { title: file.path },
    el("button", { type: "button", class: "link", onclick: () => api.openFile(file.path).catch((e) => alert(e.message)) }, t("videos.play")),
    "·",
    el("button", { type: "button", class: "link", onclick: () => api.reveal(file.path) }, t("videos.reveal")),
  );
}

/** 刚录完时页面顶上的结果：录完 / 停止 / 出错，游戏还开着没有 */
function latestBanner(f) {
  const kind = f.error ? "error" : f.cancelled ? "stopped" : "done";
  const title = {
    done: t("rec.result.done", { n: f.completed.length }),
    stopped: t("rec.result.stopped", { reason: f.cancelled }),
    error: t("rec.result.error"),
  }[kind];
  const sub = [t("rec.result.progress", { done: f.completed.length, total: f.selected.length, minutes: (f.elapsed_secs / 60).toFixed(1) })];
  if (kind !== "done") sub.push(t("rec.result.restored"));
  if (f.game_open) sub.push(t(kind === "done" ? "rec.result.gameOpen" : "rec.result.gameKept"));
  return el("div", { class: `card result ${kind}` },
    el("h2", {}, title),
    el("p", { class: "muted" }, sub.join(" ")),
    f.error ? el("p", { class: "error" }, f.error) : null,
    f.history_error ? el("p", { class: "error" }, t("rec.result.historyError", { error: f.history_error })) : null,
  );
}

function runLine(r) {
  const clips = r.selected.map((i) => `#${i}`).join("、");
  let result;
  if (r.error) result = t("videos.run.error", { error: r.error });
  else if (r.cancelled) result = t("videos.run.stopped", { done: r.completed.length, total: r.selected.length, reason: r.cancelled });
  else result = t("videos.run.done", { minutes: (r.elapsed_secs / 60).toFixed(1) });
  return el("li", {}, `${date(r.time)}  ${heroName(r.hero)}  ${t("videos.run.clips", { clips })}  ${result}`);
}

/** 删除这场的记录：确认框里可以勾"同时删除视频文件"（移到回收站），默认不勾 */
function confirmDelete(dialog, matchId) {
  return new Promise((resolve) => {
    const files = el("input", { type: "checkbox" });
    const done = (ok) => { dialog.close(); resolve(ok ? { deleteFiles: files.checked } : null); };
    dialog.replaceChildren(
      el("h2", {}, t("videos.delete.title", { id: matchId })),
      el("p", {}, t("videos.delete.what")),
      el("label", { class: "row" }, files, t("videos.delete.files")),
      el("div", { class: "dialog-actions" },
        el("button", { type: "button", onclick: () => done(false) }, t("settings.cancel")),
        el("button", { type: "button", class: "primary", onclick: () => done(true) }, t("videos.delete.ok")),
      ),
    );
    dialog.addEventListener("cancel", () => resolve(null), { once: true });
    dialog.showModal();
  });
}

/** 一场比赛的录制详情。`latest` 是刚录完的结果（从录制页跳过来时有） */
export async function showVideoMatch(view, matchId, dialog, { openMatch, videos, latest }) {
  view.replaceChildren(el("div", { class: "loading" }, "…"));
  let m;
  try {
    m = (await api.recordedMatches()).find((x) => x.match_id === matchId);
  } catch (e) {
    view.replaceChildren(el("div", { class: "card error" }, e.message));
    return;
  }
  const banner = latest ? latestBanner(latest) : null;
  if (!m) {
    // 记录写失败、或者已经删了
    view.replaceChildren(banner ?? "", el("div", { class: "card" }, el("p", { class: "muted" }, t("videos.noRecord"))));
    return;
  }

  const msg = el("span", { class: "muted" });
  const del = async () => {
    const answer = await confirmDelete(dialog, matchId);
    if (!answer) return;
    try {
      await api.deleteMatchRecord(matchId, answer.deleteFiles);
      videos();
    } catch (e) {
      msg.className = "error";
      msg.textContent = e.message;
    }
  };

  const sections = m.players.map((p) => {
    const left = unfinished(p);
    const rows = p.clips.map((c) => el("tr", { title: c.file_name },
      el("td", { class: "num" }, String(c.index)),
      el("td", {}, `${clock(c.start)} – ${clock(c.end)}`),
      el("td", { class: "wrap" }, c.victims.map(heroName).join("、"),
        c.multi_kill >= 2 ? el("span", { class: "tag", style: "margin-left:6px" }, t(`tag.multi.${Math.min(c.multi_kill, 5)}`)) : null),
      el("td", {}, c.file
        ? (c.file.exists ? el("span", { class: "ok" }, t("match.recorded")) : el("span", { class: "muted" }, t("videos.missingShort")))
        : el("span", { class: "muted" }, t("videos.unfinished"))),
      el("td", {}, c.file ? fileActions(c.file) : ""),
    ));
    if (p.compilation) {
      rows.push(el("tr", { title: p.compilation.path },
        el("td", {}), el("td", {}, t("videos.compilation")), el("td", { class: "muted" }, baseName(p.compilation.path)),
        el("td", {}, p.compilation.exists ? el("span", { class: "ok" }, t("match.recorded")) : el("span", { class: "muted" }, t("videos.missingShort"))),
        el("td", {}, fileActions(p.compilation)),
      ));
    }
    return el("div", { class: "card" },
      el("h2", {}, playerLabel(p), el("span", { class: "muted", style: "font-weight:400;margin-left:8px" }, p.player_name)),
      el("table", {},
        el("thead", {}, el("tr", {},
          el("th", { class: "num" }, "#"), el("th", {}, t("match.col.time")), el("th", {}, t("match.col.kills")),
          el("th", {}, t("match.col.status")), el("th", {}),
        )),
        el("tbody", {}, rows),
      ),
      left.length
        ? el("div", { class: "row", style: "margin-top:12px" }, el("button", {
          type: "button",
          class: "primary",
          // 走正常的录制流程：到比赛页、选好这名玩家、勾上没录完的片段，用户确认后开始
          onclick: () => openMatch(matchId, { player: p.player, preselect: left.map((c) => c.file_name) }),
        }, t("videos.resume", { n: left.length })))
        : null,
    );
  });

  view.replaceChildren(
    banner ?? "",
    el("div", { class: "row", style: "margin-bottom:12px" },
      el("h2", { style: "margin:0" }, t("match.title", { id: matchId })),
      el("span", { class: "muted" }, t("videos.lastTime", { time: date(m.last_time) })),
      el("div", { class: "spacer" }),
      msg,
      el("button", { type: "button", onclick: () => openMatch(matchId) }, t("videos.toMatch")),
      el("button", { type: "button", onclick: del }, t("videos.delete.button")),
    ),
    ...sections,
    el("details", { class: "card runs" },
      el("summary", {}, t("videos.runs", { n: m.runs.length })),
      el("ol", { class: "log" }, m.runs.map(runLine)),
    ),
  );
}
