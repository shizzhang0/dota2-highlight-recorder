// 高光视频：录好的视频（Rust 侧 history.rs 按比赛汇总的录制记录）。每个视频一张卡片，所有视频一整片网格（2026-11-25 起），
// 点缩略图在应用里播放，点英雄那一行去比赛页。
// 只放录好的视频（2026-11-20 用户定）：没录成的不记、也不提示「接着录」，要再录在录制结果页点「重新录」，或者去比赛页勾
import * as api from "./api.js";
import { t } from "./i18n.js";
import { clock, collectionName, el, heroIcon, heroName, modeName, productName, sceneTags, span } from "./format.js";
import { endPlayback, watchPlayback } from "./playlog.js";

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
// 视频从本机的播放服务读（src-tauri/src/video.rs），播放前 video_url 只放行这一个文件、给出带令牌的地址。
// 2026-11-18 前用 Tauri 的 asset 协议，用户反馈有时卡顿
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
    endPlayback();
    video.pause();
    video.removeAttribute("src");
    video.load();
  });
  video.addEventListener("error", () => { if (video.getAttribute("src")) error.hidden = false; });
  document.body.append(dialog);
  player = { dialog, title, video, error, reveal, external };
  return player;
}

export async function play(path, name) {
  const p = playerDialog();
  let url;
  try {
    url = await api.videoUrl(path);
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
  p.video.src = url;
  // 播放情况记进应用日志（playlog.js），查卡顿用
  watchPlayback(p.video, path.split(/[\\/]/).pop());
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
export const revealButton = (path) => iconButton(FOLDER_ICON, { class: "reveal", title: t("videos.reveal"), onclick: () => api.reveal(path) });

/** 一个视频一张卡片（2026-11-25 起高光视频页是一整片网格，不再按比赛分块）：
 *  缩略图（点了在应用里播放，右下角时长、左上角场景标签），下面是英雄头像、「英雄 · 内容」、玩家名
 *  （这一块点了去比赛页、选好这个英雄），最下面一行比赛 ID · 模式、「在文件夹中显示」和删除。
 *  2026-11-25 用户要去掉的：玩家名后面的段数和本机账号的「你」；最下面一行的录制时间（放在比赛 ID 后面像是比赛时间，有歧义）。
 *  页面照旧按录制时间排，最近的在前 */
function videoCard(v, nav) {
  const { m, p, file } = v;
  const img = el("img", { alt: "" });
  const th = el("div", { class: "th", title: file.path },
    img,
    file.exists ? el("div", { class: "play" }, el("span", {}, "▶")) : el("div", { class: "gone" }, t("videos.missingShort")),
    v.secs ? el("span", { class: "len" }, clock(v.secs)) : null,
    v.clip ? el("div", { class: "tags" }, sceneTags(v.clip)) : null,
  );
  const title = `${m.match_id} · ${heroName(p.hero)} · ${v.label}`;
  if (file.exists) {
    loadThumb(img, file.path);
    th.addEventListener("click", () => play(file.path, title));
  }
  const remove = async () => {
    if (!(await askDelete(nav.dialog, t("videos.deleteOne.title"), t("videos.deleteOne.what", { name: title })))) return;
    try {
      await api.deleteVideo(m.match_id, p.player, file.path);
      nav.videos();
    } catch (e) {
      alert(e.message);
    }
  };
  const sub = [String(m.match_id), modeName(m.game_mode)].filter(Boolean).join(" · ");
  // 缩略图下面两行（2026-11-25 用户定，原来三行、最下面一行空着一大段）：「英雄 · 内容」；玩家名 · 比赛 ID · 模式。
  // 玩家名太长截断加省略号（宽度有上限），比赛 ID 和模式照常显示。右边文件夹、删除两个按钮横着放、上下居中。
  // 整块点了去比赛页，悬停时整块铺浅底；按钮照旧单独点
  const open = () => nav.openMatch(m.match_id, { from: "videos", player: p.player });
  return el("div", { class: "vid" }, th,
    el("div", { class: "info", onclick: (e) => { if (!e.target.closest(".acts")) open(); } },
      el("button", { type: "button", class: "open" },
        heroIcon(p.hero),
        el("span", { class: "text" },
          el("span", { class: "t" }, `${heroName(p.hero)} · ${v.label}`),
          el("span", { class: "who" },
            p.player_name ? el("span", { class: "name", title: p.player_name }, p.player_name) : null,
            el("span", { class: "rest" }, `${p.player_name ? " · " : ""}${sub}`,
              // 录像不在本地了：视频照样能放，点进比赛页可以重新下载（2026-11-20 起）
              m.replay ? null : el("span", { class: "replay-gone" }, ` · ${t("videos.replayGone")}`))),
        ),
      ),
      el("span", { class: "acts" },
        file.exists ? revealButton(file.path) : null,
        iconButton(TRASH_ICON, { class: "reveal del", title: t("videos.deleteOne.button"), onclick: remove })),
    ));
}

/** 一场比赛里一个英雄的所有视频：单段（2026-11-18 前留下的）和成品、集锦 */
function heroVideos(m, p) {
  const out = [];
  for (const c of p.clips) {
    // 2026-11-16 之前的记录没有类别，当成击杀片段
    if (c.file) out.push({ m, p, file: c.file, time: c.time, secs: c.end - c.start, label: span(c.start, c.end), clip: { ...c, kind: c.kind ?? "kill" } });
  }
  // 成品和集锦（击杀集锦、三杀 · 16:40、自定义集锦）。单个时刻带起止时间；以前拼的集锦按以前的叫法（击杀集锦、高光集锦），更早的叫"合集"
  for (const comp of p.compilations) {
    const moment = comp.start != null;
    const label = moment ? `${productName(comp.kind)} · ${clock(comp.start)}`
      : comp.kind === "custom" || (comp.clips && comp.kind) ? productName(comp.kind) : collectionName(comp.kind);
    out.push({ m, p, file: comp.file, time: comp.time, secs: comp.secs ?? (moment ? comp.end - comp.start : 0), label });
  }
  return out;
}

/** 删除前问一句，同意返回 true。删除都是把视频移到回收站、记录里去掉：原来整场删除时可以只去掉记录、
 * 留着文件（确认框里的勾），2026-11-19 用户定不要这个选项。删错了还能从回收站找回 */
function askDelete(dialog, title, what) {
  return new Promise((resolve) => {
    const done = (ok) => { dialog.close(); resolve(ok); };
    dialog.replaceChildren(
      el("h2", {}, title),
      el("p", {}, what),
      el("div", { class: "dialog-actions" },
        el("button", { type: "button", onclick: () => done(false) }, t("settings.cancel")),
        el("button", { type: "button", class: "primary", onclick: () => done(true) }, t("videos.delete.ok")),
      ),
    );
    dialog.addEventListener("cancel", () => resolve(false), { once: true });
    dialog.showModal();
  });
}

/** 高光视频：所有视频一整片网格，最近录的在前（2026-11-25 用户定；原来一场比赛的一个英雄一块、右上角删整块，
 *  窗口宽的时候右边大半空着）。删视频只在卡片上一个一个删。录完的结果 2026-11-17 起在录制页上原地显示 */
export async function showVideos(view, nav) {
  view.replaceChildren(el("div", { class: "loading" }, "…"));
  const list = await api.recordedMatches().catch((e) => {
    view.replaceChildren(el("div", { class: "card error" }, e.message));
    return null;
  });
  if (!list) return;
  // 标题旁不写「5 场比赛 · 6 个视频」（2026-11-25 用户定，和比赛录像页一起去掉）
  const head = () => el("div", { class: "page-head" }, el("h1", {}, t("nav.videos")));
  const all = list.flatMap((m) => m.players.flatMap((p) => heroVideos(m, p)));
  if (!all.length) {
    view.replaceChildren(head(), el("div", { class: "card" }, el("p", { class: "muted", style: "margin:0" }, t("videos.empty"))));
    return;
  }
  all.sort((a, b) => b.time - a.time);
  view.replaceChildren(head(),
    el("div", { class: "vid-grid" }, all.map((v) => videoCard(v, nav))));
}
