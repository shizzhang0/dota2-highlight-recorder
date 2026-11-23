// 比赛页：选玩家（十个英雄卡片）→ 高光时间线 →「推荐」｜「自定义」→ 底部汇总。
// 2026-11-11 改版：英雄用大图，片段用小图标排开，多杀就多几个（docs/app-interaction.md 第 6 节）。
// 2026-11-18 用户定（参考完美时刻）：默认是「推荐」，直接列这局能生成的成品（击杀集锦、助攻集锦、三杀……），勾成品就行；
// 原来逐个勾片段的那套挪到「自定义」
import * as api from "./api.js";
import { resolve, t } from "./i18n.js";
import { clock, el, heroIcon, heroName, heroPic, length, productName, sceneTags, span } from "./format.js";
import { settings, persist } from "./state.js";
import { play, revealButton } from "./videos.js";
import { downloadError } from "./list.js";

const MODES = ["rec", "custom"];

/** 录像不在本地时的比赛页（2026-11-20 起；原来只有一行红字「本地没有这场录像：路径」）：
 * 说明一下、给「重新下载录像」，边下边显示进度（和本地录像页同一套下载），下好了 `reopen` 打开比赛页；下不了说清楚原因 */
function showMissing(view, matchId, { back, backLabel, reopen }) {
  const title = el("div", { class: "missing-title" }, t("match.missing.title"));
  const what = el("div", { class: "muted" }, t("match.missing.what"));
  const fill = el("div", { class: "bar-fill" });
  const bar = el("div", { class: "bar", hidden: true }, fill);
  const dl = el("button", { type: "button" }, t("match.missing.download"));
  const cancel = el("button", { type: "button", hidden: true, onclick: () => api.cancelDownload(matchId) }, t("home.dl.cancel"));
  const idle = (text) => {
    title.textContent = t("match.missing.title");
    what.className = text ? "error" : "muted";
    what.textContent = text ?? t("match.missing.what");
    dl.hidden = false;
    cancel.hidden = true;
    bar.hidden = true;
  };
  dl.addEventListener("click", async () => {
    dl.hidden = true;
    cancel.hidden = false;
    what.className = "muted";
    title.textContent = t("match.missing.downloading");
    what.textContent = t("home.dl.finding", { id: matchId });
    const off = await window.__TAURI__.event.listen("download-progress", ({ payload: p }) => {
      if (String(p.match_id) !== String(matchId)) return;
      const mb = (n) => (n / 1e6).toFixed(1);
      if (p.stage === "finding_url") what.textContent = t("home.dl.finding", { id: matchId });
      else if (p.stage === "waiting_for_url") what.textContent = t("home.dl.waiting", { secs: p.waited_secs, max: p.max_secs });
      else if (p.stage === "saving") what.textContent = t("home.dl.saving");
      else if (p.stage === "downloading") {
        what.textContent = t(p.total ? "match.missing.progress" : "match.missing.progressNoTotal", { done: mb(p.done), total: p.total ? mb(p.total) : "" });
        bar.hidden = !p.total;
        if (p.total) fill.style.width = `${Math.min(100, (p.done / p.total) * 100)}%`;
      }
    });
    try {
      await api.downloadReplay(matchId);
      off();
      reopen();
    } catch (e) {
      off();
      idle(e?.kind === "cancelled" ? undefined : downloadError(e, matchId));
    }
  });
  view.replaceChildren(
    el("div", { class: "back-bar" }, el("button", { type: "button", class: "back", onclick: back }, backLabel)),
    el("div", { class: "card missing" },
      el("div", { class: "missing-text" }, title, what, bar),
      el("div", { class: "missing-acts" }, dl, cancel)),
  );
}
/** 已生成卡片上的播放按钮图标（实心三角，往右挪一点看着才居中） */
const PLAY_ICON = '<svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true"><path d="M8 5.5v13a1 1 0 0 0 1.5.86l10.5-6.5a1 1 0 0 0 0-1.72L9.5 4.64A1 1 0 0 0 8 5.5z" fill="currentColor"/></svg>';

/** `player` / `preselect`（片段文件名）/ `preselectProducts`（「推荐」的成品标识）从录制结果页的「重新录」带过来：
 * 选好玩家，勾上上次录的片段（到「自定义」）或成品（到「推荐」）。镜头、拼成集锦本来就记着上次的 */
/** `back` / `backLabel`：左上角的返回，从哪进来回哪（本地录像或已录视频） */
export async function showMatch(view, matchId, opts) {
  const { back, backLabel, record, player: wanted, preselect, preselectProducts } = opts;
  // 录像不在本地了（已录视频、重新录点进来，录像已经删了）：说明情况，给「重新下载录像」，下好了再打开
  if (!(await api.hasReplay(matchId).catch(() => true))) {
    showMissing(view, matchId, { back, backLabel, reopen: () => showMatch(view, matchId, opts) });
    return;
  }
  view.replaceChildren(el("div", { class: "loading" }, t("match.parsing")));
  let info;
  try {
    info = await api.openMatch(matchId);
  } catch (e) {
    view.replaceChildren(el("button", { type: "button", class: "back", onclick: back }, backLabel), el("div", { class: "card error" }, e.message));
    return;
  }

  let player = info.players.some((p) => p.id === wanted) ? wanted : info.me ?? info.players[0].id;
  // 带过来的片段只在第一次算计划时用；规则改过的话可能对不上，提示一句
  let pending = preselect ? new Set(preselect) : null;
  let pendingProducts = preselectProducts?.length ? new Set(preselectProducts) : null;
  const preselectNote = el("p", { class: "warn", hidden: true });
  let clips = [];
  let picked = new Set();
  // 「推荐」：这名玩家能生成的成品、勾了哪些（成品标识）
  let products = [];
  let chosen = new Set();
  // 带着"继续录未完成的"进来时直接到「自定义」，那些片段在那里勾着
  let mode = preselect && !pendingProducts ? "custom" : "rec";
  const current = () => info.players.find((p) => p.id === player);
  /** 勾了的成品用到的片段序号（同一段只录一次） */
  const usedClips = () => new Set(products.filter((x) => chosen.has(x.id)).flatMap((x) => x.clips));

  const playersBox = el("div", { class: "teams" });
  const highlight = el("div", { class: "card" });
  const clipsBox = el("div");
  const allBtn = el("button", { type: "button" });
  const summary = el("span", { class: "muted mono" });
  const recordBtn = el("button", { type: "button", class: "primary" }, t("match.record"));
  // 快速勾选：一类一个按钮，点了勾上这一类的全部片段（都勾着时再点就取消）。数量为 0 的不显示
  const chipsBox = el("div", { class: "chips" });
  // 拼成集锦：选了 2 段以上才出现，后面是按内容起的名字；记住上次的选择（开始录制时存进设置）
  // 原来是设置页的"拼成合集"开关，2026-11-16 挪到这里（用户：和集锦重复）
  let compileValue = settings.compile !== false;
  const compileBox = el("input", { type: "checkbox" });
  compileBox.checked = compileValue;
  compileBox.addEventListener("change", () => { compileValue = compileBox.checked; });
  const compileName = el("span", { class: "name" });
  const compileLabel = el("label", { class: "compile" }, compileBox, el("span", {}, t("match.compile")), compileName);
  // 镜头：对这次录的所有片段生效，记住上次选的（开始录制时存进设置）。只有两种，用两段切换按钮
  const cameras = ["hero_chase", "player_view"];
  let cameraValue = settings.camera === "player_view" ? "player_view" : "hero_chase";
  const cameraButtons = cameras.map((v) => el("button", {
    type: "button",
    role: "radio",
    title: t(`match.camera.${v}.hint`),
    onclick: () => setCamera(v),
  }, t(`match.camera.${v}`)));
  const setCamera = (v) => {
    cameraValue = v;
    cameraButtons.forEach((b, i) => {
      b.classList.toggle("on", cameras[i] === v);
      b.setAttribute("aria-checked", String(cameras[i] === v));
    });
  };
  setCamera(cameraValue);
  const camera = el("div", { class: "segmented", role: "radiogroup", "aria-label": t("match.camera") }, cameraButtons);
  recordBtn.addEventListener("click", () => {
    settings.camera = cameraValue;
    const minutes = (list) => (list.reduce((a, c) => a + c.estimated_secs, 0) / 60).toFixed(1);
    if (mode === "rec") {
      const ids = products.filter((x) => chosen.has(x.id)).map((x) => x.id);
      const used = usedClips();
      const sel = clips.filter((c) => used.has(c.index));
      if (!ids.length) return;
      record({
        matchId, player, hero: current().hero, clips: sel, products: ids, camera: cameraValue, compile: false, overwrite: 0,
        secs: sel.reduce((a, c) => a + c.duration, 0), minutes: minutes(sel),
      });
      return;
    }
    const sel = clips.filter((c) => picked.has(c.index));
    if (!sel.length) return;
    settings.compile = compileValue;
    record({
      matchId,
      player,
      hero: current().hero,
      clips: sel,
      products: [],
      camera: cameraValue,
      compile: compileValue,
      overwrite: sel.filter((c) => c.recorded).length,
      secs: sel.reduce((a, c) => a + c.duration, 0),
      minutes: minutes(sel),
    });
  });
  allBtn.addEventListener("click", () => {
    picked = picked.size === clips.length ? new Set() : new Set(clips.map((c) => c.index));
    renderClips();
  });

  /** 十个英雄：天辉一行、夜魇一行，点卡片换人 */
  function renderPlayers() {
    const team = (id, cls, label) => [
      el("div", { class: `side ${cls}` }, label),
      ...info.players.filter((p) => p.team === id).map((p) =>
        el("button", { type: "button", class: `hero-card${p.id === player ? " selected" : ""}`, title: `${heroName(p.hero)} · ${p.name}`, onclick: () => pick(p.id) },
          p.is_me ? el("span", { class: "me-tag" }, t("match.you")) : null,
          heroPic(p.hero),
          el("div", { class: "who" }, el("span", {}, p.name), el("span", { class: "kd", title: t("home.kda") }, `${p.kills} / ${p.deaths} / ${p.assists}`)),
        )),
    ];
    playersBox.replaceChildren(...team(2, "radiant", t("match.radiant")), ...team(3, "dire", t("match.dire")));
  }

  function toggle(index) {
    picked.has(index) ? picked.delete(index) : picked.add(index);
    renderClips();
  }

  /** 快速勾选，分两组（2026-11-16 用户定）：
   * 左边是片段类别，每个片段只属于其中一类（击杀勾上所有拿到人头的片段，不管杀几个，所以不另设"普通击杀"）；
   * 右边是高光时刻，从片段里挑精彩的，一个片段可以同时属于几个。双杀、三杀、四杀、暴走（五杀及以上）各一个 */
  const kinds = [
    ["chip.kill", (c) => c.kind === "kill"],
    ["chip.assist", (c) => c.kind === "assist"],
  ];
  const moments = [
    ["tag.multi.2", (c) => c.multi_kill === 2],
    ["tag.multi.3", (c) => c.multi_kill === 3],
    ["tag.multi.4", (c) => c.multi_kill === 4],
    ["tag.multi.5", (c) => c.multi_kill >= 5],
    ["chip.lowHp", (c) => c.low_hp],
    ["chip.teamwork", (c) => c.teamwork],
    ["chip.aegis", (c) => c.aegis_steal],
  ];

  function renderChips() {
    const chips = (group) => group.map(([name, test]) => {
      const list = clips.filter(test);
      if (!list.length) return null;
      const all = list.every((c) => picked.has(c.index));
      return el("button", {
        type: "button",
        class: `chip${all ? " on" : ""}`,
        "aria-pressed": String(all),
        onclick: () => {
          for (const c of list) all ? picked.delete(c.index) : picked.add(c.index);
          renderClips();
        },
      }, t(name), el("b", {}, String(list.length)));
    }).filter(Boolean);
    const left = chips(kinds), right = chips(moments);
    // 两组都有按钮时中间加一条竖线
    const sep = left.length && right.length ? [el("span", { class: "chip-sep", "aria-hidden": "true" })] : [];
    chipsBox.replaceChildren(...left, ...sep, ...right);
  }

  /** 片段里的高光时刻（越少见越靠前）：时间线上有就在色块角上标一颗星，名字放在悬停提示里 */
  function momentNames(c) {
    return [
      c.aegis_steal && t("scene.aegis"),
      c.low_hp && t("scene.lowHp"),
      c.multi_kill >= 3 && t(`tag.multi.${Math.min(c.multi_kill, 5)}`),
      c.teamwork && t("scene.teamwork"),
    ].filter(Boolean);
  }

  /** 高光时间线：整场一条线，色块是片段（击杀红、助攻蓝，没勾的变淡），上面挂被杀英雄的小图标，点色块也能勾选。
   * 有高光时刻的色块加一圈金框，名字在悬停提示里。原来是在图标那排写「残血反杀」这类小字，字把一组撑宽、压住旁边片段的图标
   * （2026-11-22 用户截图）；试过色块右上角挂金星，用户觉得金框好看 */
  function renderTimeline() {
    const p = current();
    const kills = clips.filter((c) => c.kind === "kill").reduce((a, c) => a + c.victims.length, 0);
    const assists = clips.reduce((a, c) => a + (c.kind === "assist" ? c.victims.length : c.assists), 0);
    const aegis = clips.filter((c) => c.aegis_steal).length;
    const total = Math.max(info.duration, ...clips.map((c) => c.end), 1);
    const pct = (s) => `${(Math.max(0, s) / total) * 100}%`;
    const line = el("div", { class: "timeline" }, el("div", { class: "track" }));
    // 色块都一样宽（`SEG_PX`），放在片段中点，和上面的图标对齐。原来宽度按时长画（最少 18 像素），
    // 长片段（比如 41 秒的四杀）突然大一块，用户觉得怪（2026-11-22），时长看悬停提示。
    // 挨得很近的两段会叠在一起、编号压编号：按时间线大概的像素宽度算，和前一段挤到一起（留不出 2 像素）就放到下一排
    const SEG_PX = 18;
    const perPx = total / Math.max(400, window.innerWidth - 110);
    let lastEnd = -Infinity, lastLow = false;
    // 「推荐」里亮的是勾了的成品用到的片段，点色块不勾选；「自定义」里亮的是勾了的片段，点色块勾选
    const used = mode === "rec" ? usedClips() : picked;
    clips.forEach((c, i) => {
      const off = !used.has(c.index);
      const mid = (c.start + c.end) / 2;
      const low = !lastLow && mid - (SEG_PX / 2) * perPx - lastEnd < 2 * perPx;
      lastEnd = mid + (SEG_PX / 2) * perPx;
      lastLow = low;
      const names = momentNames(c);
      line.append(
        el("div", {
          class: `seg ${c.kind}${off ? " off" : ""}${low ? " low" : ""}${names.length ? " moment" : ""}`,
          style: `left:${pct(mid)}`,
          title: [...names, span(c.start, c.end)].join(" · "),
          onclick: () => mode === "custom" && toggle(c.index),
        }, String(c.index)),
        // 相邻片段的图标会叠在一起，上下两排交替放
        el("div", { class: `kills ${i % 2 ? "b" : "a"}${off ? " off" : ""}`, style: `left:${pct(mid)}` },
          c.victims.map((v) => heroIcon(v, true))),
      );
    });
    const ticks = el("div", { class: "ticks" });
    for (let m = 0; m * 60 <= total; m += 10) ticks.append(el("span", { style: `left:${pct(m * 60)}` }, `${m}:00`));
    line.append(ticks);
    // 颜色说明只写这场实际出现的类别
    const colors = ["kill", "assist"].filter((k) => clips.some((c) => c.kind === k)).map((k) => t(`match.legend.${k}`)).join(t("match.legend.sep"));
    highlight.replaceChildren(
      el("h2", {}, t("match.highlights", { hero: heroName(p.hero), name: p.name })),
      el("p", { class: "sub" }, clips.length
        ? t("match.highlightsSub", { kills, assists, n: clips.length }) + (aegis ? t("match.highlightsAegis", { n: aegis }) : "")
          // 说明按当前页写：「推荐」里点色块不勾选，亮的是勾了的成品要用的片段
          + t(mode === "rec" ? "match.highlightsHintRec" : "match.highlightsHint", { colors })
        : t("match.noClips")),
      clips.length ? line : "",
    );
  }

  function renderSummary() {
    if (mode === "rec") {
      // 「推荐」：勾了几个成品、要录几段（共用的只算一次）、预计多久
      const n = products.filter((x) => chosen.has(x.id)).length;
      const used = usedClips();
      const est = clips.filter((c) => used.has(c.index)).reduce((a, c) => a + c.estimated_secs, 0);
      summary.textContent = n ? t("match.genSummary", { n, clips: used.size, minutes: (est / 60).toFixed(1) }) : t("match.genNone");
      compileLabel.hidden = true;
      recordBtn.textContent = n ? t("match.generate", { n }) : t("match.generateNone");
      recordBtn.disabled = n === 0;
      return;
    }
    const sel = clips.filter((c) => picked.has(c.index));
    const secs = sel.reduce((a, c) => a + c.duration, 0);
    const est = sel.reduce((a, c) => a + c.estimated_secs, 0);
    const overwrite = sel.filter((c) => c.recorded).length;
    summary.textContent = t("match.summary", {
      n: sel.length, total: clips.length, secs: length(secs), minutes: (est / 60).toFixed(1),
    }) + (overwrite ? t("match.overwrite", { n: overwrite }) : "");
    // 「自定义」拼出来的统一叫自定义集锦（2026-11-18 起，「推荐」里才有击杀集锦、助攻集锦）
    compileLabel.hidden = sel.length < 2;
    compileName.textContent = t("match.compileName", { hero: heroName(current().hero), name: productName("custom") });
    recordBtn.textContent = t("match.record");
    recordBtn.disabled = sel.length === 0;
    allBtn.hidden = clips.length === 0;
    allBtn.textContent = t(picked.size === clips.length ? "match.selectNone" : "match.selectAll");
  }

  /** 「推荐」：成品卡片，只列这局有的。集锦在前，高光时刻按难得程度排。
   * 已生成的点卡片直接播放，没生成的点了勾选。
   * 卡片里不列选了哪几段（2026-11-18 用户：文字、小标签都不好看）：勾上后时间线上亮的就是它要用的片段 */
  function renderProducts() {
    const card = (x) => {
      // 击杀集锦、助攻集锦（都太少时合成的高光集锦）按击杀、助攻次数说；高光时刻（同一种合成一个，2026-11-18 用户定）按次数说，只有一次就写时间
      const units = { kills: "product.unitKills", assists: "product.unitAssists", highlights: "product.unitKillsAssists" };
      const unit = t(units[x.kind] ?? "product.unitMoments");
      const sub = x.start != null
        ? `${clock(x.start)} · ${length(x.duration)}`
        : t(x.picked < x.total ? "product.picked" : "product.all", { picked: x.picked, total: x.total, unit, len: length(x.duration) });
      if (x.generated) {
        // 已生成（2026-11-18 用户定）：名字后面绿色「✓ 已生成」；右上角（没生成时放勾选框的地方）两个圆形按钮，
        // 播放实心、在文件夹中显示描边，卡片和没生成的一样高。点卡片别处也是播放
        const title = `${matchId} · ${heroName(current().hero)} · ${productName(x.kind)}`;
        const playBtn = el("button", { type: "button", class: "round primary", title: t("product.play"), "aria-label": t("product.play"), onclick: (e) => { e.stopPropagation(); play(x.path, title); } });
        playBtn.innerHTML = PLAY_ICON;
        const folder = revealButton(x.path);
        folder.className = "round";
        folder.addEventListener("click", (e) => e.stopPropagation());
        return el("div", { class: "product done", title: x.file_name, onclick: () => play(x.path, title) },
          el("span", { class: "name" }, productName(x.kind), el("span", { class: "done-tag" }, "✓ ", t("product.generated"))),
          el("span", { class: "sub" }, sub),
          el("span", { class: "actions" }, playBtn, folder));
      }
      const on = chosen.has(x.id);
      const box = el("input", { type: "checkbox", tabindex: -1, "aria-hidden": "true" });
      box.checked = on;
      return el("button", { type: "button", class: `product${on ? " on" : ""}`, "aria-pressed": String(on), title: x.file_name, onclick: () => {
        on ? chosen.delete(x.id) : chosen.add(x.id);
        renderClips();
      } }, box, el("span", { class: "name" }, productName(x.kind)), el("span", { class: "sub" }, sub));
    };
    // 只列这局有的（2026-11-18 用户定：原来没有的时刻合成一张灰卡片「这局没有」，不要）
    productsBox.replaceChildren(products.length ? el("div", { class: "product-grid" }, products.map(card)) : el("p", { class: "muted", style: "margin:0" }, t("match.noClips")));
  }

  /** 「推荐」｜「自定义」切换：两边的勾选各自保留 */
  function setMode(m) {
    mode = m;
    modeButtons.forEach((b, i) => {
      b.classList.toggle("on", MODES[i] === m);
      b.setAttribute("aria-checked", String(MODES[i] === m));
    });
    recCard.hidden = m !== "rec";
    customCard.hidden = m !== "custom";
    renderClips();
  }

  /** 片段列表：两列，一行一个片段；被杀英雄用小图标排开，多杀就多几个 */
  function renderClips() {
    renderTimeline();
    renderChips();
    renderProducts();
    if (!clips.length) {
      clipsBox.replaceChildren();
      renderSummary();
      return;
    }
    clipsBox.replaceChildren(el("div", { class: "clip-list" }, clips.map((c) => {
      const on = picked.has(c.index);
      const box = el("input", { type: "checkbox" });
      box.checked = on;
      // 点框本身也是点这一行，不让它再触发一次
      box.addEventListener("click", (e) => { e.preventDefault(); });
      // 连杀只放在悬停提示里（2026-11-16 用户定）：它从上次死亡累计，和这段杀几个无关，挂在行上几乎每行都有、数还忽高忽低
      const streak = c.kill_streak >= 3
        ? "\n" + t("match.streakHint", { n: c.kill_streak, name: t(`tag.streak.${Math.min(c.kill_streak, 10)}`) })
        : "";
      return el("div", { class: `clip-row${on ? "" : " off"}`, title: c.file_name + streak, onclick: () => toggle(c.index) },
        box,
        el("span", { class: "no" }, String(c.index)),
        el("span", { class: "t" }, span(c.start, c.end), el("small", {}, length(c.duration))),
        // 抢盾片段没有被杀的英雄，写"肉山"
        el("span", { class: "hero-icons" }, c.victims.length ? c.victims.map((v) => heroIcon(v)) : el("span", { class: "muted" }, t("scene.roshan"))),
        el("span", { class: "end" },
          ...sceneTags(c),
          // 只有单段文件还在才标「已录」。拼进集锦的单段已经删了，在这里勾上照样要重录，不标
          // （2026-11-18 先做过「已录 · 在集锦里」，用户说不对：单段并没有保留）
          c.recorded ? el("span", { class: "ok", title: t("match.recordedHint") }, t("match.recorded")) : null),
      );
    })));
    renderSummary();
  }

  async function replan() {
    try {
      ({ clips, products } = await api.plan(matchId, player, settings.rules, resolve(settings.language)));
    } catch (e) {
      clipsBox.replaceChildren(el("p", { class: "error" }, e.message));
      clips = [];
      products = [];
    }
    // 默认一个都不勾，用户自己挑
    picked = new Set();
    chosen = new Set();
    if (pendingProducts) {
      const found = products.filter((x) => pendingProducts.has(x.id));
      chosen = new Set(found.map((x) => x.id));
      preselectNote.hidden = found.length === pendingProducts.size;
      preselectNote.textContent = t("match.preselectMissing", { n: pendingProducts.size - found.length });
      pendingProducts = null;
      pending = null;
    } else if (pending) {
      const found = clips.filter((c) => pending.has(c.file_name));
      picked = new Set(found.map((c) => c.index));
      preselectNote.hidden = found.length === pending.size;
      preselectNote.textContent = t("match.preselectMissing", { n: pending.size - found.length });
      pending = null;
    } else {
      preselectNote.hidden = true;
    }
    renderClips();
  }

  function pick(id) {
    player = id;
    renderPlayers();
    replan();
  }

  // 规则参数：改了立刻重算，并存进设置
  const rulesBox = el("div", { class: "rules-grid" });
  function renderRules() {
    const num = (key, step = 1) => {
      const input = el("input", { type: "number", min: 0, step, value: settings.rules[key] });
      input.addEventListener("change", () => {
        const v = Number(input.value);
        if (Number.isFinite(v) && v >= 0) {
          settings.rules[key] = v;
          persist();
          replan();
        }
      });
      return el("label", {}, t(`rules.${key.replace(/_(\w)/g, (_, c) => c.toUpperCase())}`), input);
    };
    const anchor = el("select", {}, ["a", "b", "c"].map((a) => el("option", { value: a }, t(`rules.anchor.${a}`))));
    anchor.value = settings.rules.anchor;
    anchor.addEventListener("change", () => { settings.rules.anchor = anchor.value; persist(); replan(); });
    rulesBox.replaceChildren(
      el("label", {}, t("rules.anchor"), anchor),
      num("damage_gap", 0.5), num("max_lead"), num("post", 0.5), num("merge_gap"), num("keep_gap"),
      el("div", {}, el("button", {
        type: "button",
        onclick: async () => { settings.rules = await api.defaultRules(); persist(); renderRules(); replan(); },
      }, t("rules.reset"))),
    );
  }

  const result = info.radiant_won == null ? "" : el("span", { class: info.radiant_won ? "win" : "lose" }, t(info.radiant_won ? "match.radiantWon" : "match.direWon"));
  // 返回固定在顶部，翻到下面也能点；鼠标侧键的"后退"也返回（2026-11-16 用户：不要滚回顶上再点）
  const backBar = el("div", { class: "back-bar" }, el("button", { type: "button", class: "back", onclick: back }, backLabel));
  const onMouseBack = (e) => {
    if (!backBar.isConnected) {
      document.removeEventListener("mouseup", onMouseBack);
      return;
    }
    if (e.button === 3) {
      e.preventDefault();
      document.removeEventListener("mouseup", onMouseBack);
      back();
    }
  };
  document.addEventListener("mouseup", onMouseBack);
  // 「推荐」｜「自定义」两段切换，和镜头、录制画面的切换一样
  const modeButtons = MODES.map((m) => el("button", { type: "button", role: "radio", onclick: () => setMode(m) }, t(`match.tab.${m}`)));
  const productsBox = el("div");
  const recCard = el("div", { class: "card" },
    el("div", { class: "clips-head" }, el("h2", {}, t("match.tab.rec")), el("span", { class: "muted", style: "font-size:13px" }, t("match.recHint"))),
    productsBox);
  const customCard = el("div", { class: "card" },
    el("div", { class: "clips-head" }, el("h2", {}, t("match.clips")), el("span", { class: "muted", style: "font-size:13px" }, t("match.clipsHint")), el("div", { class: "spacer" }), allBtn),
    chipsBox, preselectNote, clipsBox,
    el("details", { class: "rules", style: "margin-top:16px" }, el("summary", {}, t("rules.title")), rulesBox));
  view.replaceChildren(
    backBar,
    el("div", { class: "page-head" },
      el("h1", {}, t("match.title", { id: info.match_id })),
      // 时长 · 胜负。原来最后还有「版本 6972」（录像的构建号），用户看不懂，2026-11-19 去掉
      el("span", { class: "meta" }, clock(info.duration), result ? " · " : "", result),
    ),
    playersBox,
    highlight,
    el("div", { class: "segmented mode-tabs", role: "radiogroup", "aria-label": t("match.tab.label") }, modeButtons),
    recCard,
    customCard,
    el("div", { class: "footer-dock" }, el("div", { class: "footer-bar" }, summary, el("div", { class: "spacer" }), compileLabel, camera, recordBtn)),
  );
  renderPlayers();
  renderRules();
  setMode(mode);
  await replan();
}
