// 已录视频：按比赛汇总的录制记录（Rust 侧 history.rs）。2026-11-11 改版：每个视频一张缩略图，点缩略图在应用里播放；
// 没录完的片段显示成虚线框"接着录"。只有一层（2026-11-11 用户试用后去掉了详情页）：每场比赛一块，
// 比赛 ID 点了去比赛页，右上角「删除记录」，下面折叠的录制记录；刚录完回到这里，顶上显示这次的结果
import * as api from "./api.js";
import { t } from "./i18n.js";
import { clock, date, el, heroIcon, heroName } from "./format.js";

const playerLabel = (p) => `${heroName(p.hero)}${p.is_me ? t("match.me") : ""}`;
const unfinished = (p) => p.clips.filter((c) => !c.file);

// 缩略图一张一张取：第一次要 ffmpeg 现取一帧，同时开十几个 ffmpeg 会卡；取过的有缓存，很快
const thumbQueue = [];
let thumbBusy = false;
function loadThumb(img, path) {
  thumbQueue.push([img, path]);
  // 等这一轮把元素放进页面再开始，切走了的页面就不取了
  setTimeout(pumpThumbs, 0);
}
async function pumpThumbs() {
  if (thumbBusy) return;
  thumbBusy = true;
  while (thumbQueue.length) {
    const [img, path] = thumbQueue.shift();
    if (!img.isConnected) continue;
    try {
      img.src = await api.videoThumb(path);
    } catch {
      img.hidden = true;
    }
  }
  thumbBusy = false;
}

// 播放器：点缩略图在应用里放（2026-11-12 用户要的，原来是交给系统默认播放器）。
// 视频经 Tauri 的 asset 协议读，播放前 allow_video 只放行这一个文件
let player = null;
function playerDialog() {
  if (player) return player;
  const title = el("h2", {});
  const video = el("video", { controls: true, autoplay: true, preload: "auto" });
  const error = el("p", { class: "error", hidden: true }, t("videos.play.error"));
  const reveal = el("button", { type: "button" }, t("videos.reveal"));
  const external = el("button", { type: "button" }, t("videos.play.system"));
  const close = el("button", { type: "button", class: "close", "aria-label": t("videos.play.close"), title: t("videos.play.close") }, "×");
  const dialog = el("dialog", { class: "player" },
    el("div", { class: "player-head" }, title, close),
    video, error,
    el("div", { class: "dialog-actions" }, reveal, external));
  close.addEventListener("click", () => dialog.close());
  // 点遮罩（对话框外面）也关
  dialog.addEventListener("click", (e) => e.target === dialog && dialog.close());
  // 关了就停下、放掉文件，不然声音还在、文件也被占着
  dialog.addEventListener("close", () => {
    video.pause();
    video.removeAttribute("src");
    video.load();
  });
  video.addEventListener("error", () => { if (video.getAttribute("src")) error.hidden = false; });
  document.body.append(dialog);
  player = { dialog, title, video, error, reveal, external };
  return player;
}

async function play(path, name) {
  const p = playerDialog();
  try {
    await api.allowVideo(path);
  } catch (e) {
    alert(e.message);
    return;
  }
  p.title.textContent = name;
  p.error.hidden = true;
  p.reveal.onclick = () => api.reveal(path);
  p.external.onclick = () => {
    p.dialog.close();
    api.openFile(path).catch((e) => alert(e.message));
  };
  p.video.src = window.__TAURI__.core.convertFileSrc(path);
  p.dialog.showModal();
}

// 「在文件夹中显示」用文件夹图标（2026-11-12 用户要的，原来写着"文件夹"三个字）
const FOLDER_ICON = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" aria-hidden="true"><path d="M3 6.5A1.5 1.5 0 0 1 4.5 5h4.3l2 2.2h8.7A1.5 1.5 0 0 1 21 8.7v9.8a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 18.5z"/></svg>';
// 「删除记录」也换成图标（垃圾桶），平时灰色不抢眼，鼠标移上去变红；点了照样先弹确认框
const TRASH_ICON = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 7h16M9.5 7V4.8c0-.4.3-.8.8-.8h3.4c.5 0 .8.4.8.8V7M6.5 7l.9 12.2c.1.9.8 1.8 1.8 1.8h5.6c1 0 1.7-.9 1.8-1.8L17.5 7M10 11v6M14 11v6"/></svg>';
/** 只有图标的按钮：`title` 同时当悬停提示和读屏文字 */
function iconButton(svg, { title, ...attrs }) {
  const b = el("button", { type: "button", title, "aria-label": title, ...attrs });
  b.innerHTML = svg;
  return b;
}
const revealButton = (path) => iconButton(FOLDER_ICON, { class: "reveal", title: t("videos.reveal"), onclick: () => api.reveal(path) });

/** 一个视频：缩略图（点了在应用里播放）、时长、多杀，下面是时间和被杀英雄。`title` 是播放器上方的标题 */
function videoCard(file, { range, secs, victims = [], multi = 0, label, title }) {
  const img = el("img", { alt: "" });
  const th = el("div", { class: "th", title: file.path },
    img,
    file.exists ? el("div", { class: "play" }, el("span", {}, "▶")) : el("div", { class: "gone" }, t("videos.missingShort")),
    secs ? el("span", { class: "len" }, `${Math.round(secs)}s`) : null,
    multi >= 2 ? el("span", { class: "multi" }, t(`tag.multi.${Math.min(multi, 5)}`)) : null,
  );
  if (file.exists) {
    loadThumb(img, file.path);
    th.addEventListener("click", () => play(file.path, title ?? label ?? range));
  }
  return el("div", { class: "vid" }, th,
    el("div", { class: "meta" },
      el("span", { class: "tr" }, label ?? range),
      el("span", { class: "hero-icons" }, victims.map((v) => heroIcon(v, true))),
      file.exists ? revealButton(file.path) : null,
    ));
}

/** 一场比赛的所有视频（每个玩家的片段、合集），加上没录完的"接着录" */
function videoGrid(m, { openMatch }) {
  const cards = [];
  for (const p of m.players) {
    for (const c of p.clips) {
      const range = `${clock(c.start)}–${clock(c.end)}`;
      if (c.file) cards.push(videoCard(c.file, { range, secs: c.end - c.start, victims: c.victims, multi: c.multi_kill, title: `${m.match_id} · ${heroName(p.hero)} · ${range}` }));
    }
    if (p.compilation) {
      const label = t("videos.compilationOf", { hero: heroName(p.hero) });
      cards.push(videoCard(p.compilation, { label, title: `${m.match_id} · ${label}` }));
    }
    const left = unfinished(p);
    if (left.length) {
      cards.push(el("div", { class: "vid left" }, el("div", {},
        t("videos.leftN", { n: left.length, hero: heroName(p.hero) }), el("br"),
        // 走正常的录制流程：到比赛页、选好这名玩家、勾上没录完的片段，用户确认后开始
        el("button", { type: "button", style: "margin-top:10px", onclick: () => openMatch(m.match_id, { from: "videos", player: p.player, preselect: left.map((c) => c.file_name) }) }, t("videos.resumeShort")),
      )));
    }
  }
  return el("div", { class: "vid-grid" }, cards);
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

/** 一场比赛一块：标题（英雄图标、比赛 ID、谁、最近什么时候录的）（点比赛 ID 去比赛页）、「删除记录」、视频、折叠的录制记录 */
function matchSection(m, nav) {
  const lead = m.players.find((p) => p.is_me) ?? m.players[0];
  const msg = el("span", { class: "muted" });
  const del = async () => {
    const answer = await confirmDelete(nav.dialog, m.match_id);
    if (!answer) return;
    try {
      await api.deleteMatchRecord(m.match_id, answer.deleteFiles);
      nav.videos();
    } catch (e) {
      msg.className = "error";
      msg.textContent = e.message;
    }
  };
  return el("section", { class: "rec-match" },
    el("div", { class: "head" },
      heroIcon(lead.hero),
      el("div", {},
        // 点比赛 ID 去比赛页（2026-11-12 把原来的「详情」按钮并进来了）
        el("button", { type: "button", class: "t", title: t("videos.moreHint"), onclick: () => nav.openMatch(m.match_id, { from: "videos" }) },
          String(m.match_id), el("span", { class: "go", "aria-hidden": "true" }, "›")),
        el("div", { class: "who" }, m.players.map(playerLabel).join("、"), " · ", t("videos.lastTime", { time: date(m.last_time) })),
      ),
      el("div", { class: "act" },
        msg,
        iconButton(TRASH_ICON, { class: "icon-btn danger-icon", title: t("videos.delete.button"), onclick: del }),
      ),
    ),
    videoGrid(m, nav),
    el("details", { class: "runs" },
      el("summary", {}, t("videos.runs", { n: m.runs.length })),
      el("ol", { class: "log" }, m.runs.map(runLine)),
    ),
  );
}

/** `nav.latest`：刚录完的结果（从录制页回来时有），显示在顶上，刚录的那场排第一 */
export async function showVideos(view, nav) {
  view.replaceChildren(el("div", { class: "loading" }, "…"));
  let list = await api.recordedMatches().catch((e) => {
    view.replaceChildren(el("div", { class: "card error" }, e.message));
    return null;
  });
  if (!list) return;
  const banner = nav.latest ? latestBanner(nav.latest) : "";
  const head = (meta) => el("div", { class: "page-head" }, el("h1", {}, t("nav.videos")), el("span", { class: "meta" }, meta));
  if (!list.length) {
    view.replaceChildren(banner, head(""), el("div", { class: "card" }, el("p", { class: "muted", style: "margin:0" }, t("videos.empty"))));
    return;
  }
  if (nav.latest) list = [...list.filter((m) => m.match_id === nav.latest.match_id), ...list.filter((m) => m.match_id !== nav.latest.match_id)];
  const videos = list.reduce((a, m) => a + m.players.reduce((b, p) => b + p.clips.filter((c) => c.file).length + (p.compilation ? 1 : 0), 0), 0);
  view.replaceChildren(banner, head(t("videos.count", { matches: list.length, videos })), ...list.map((m) => matchSection(m, nav)));
}
