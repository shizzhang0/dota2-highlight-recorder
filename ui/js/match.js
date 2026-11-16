// 比赛页：选玩家（十个英雄卡片）→ 高光时间线 + 片段列表（默认不勾，标出已录过的）→ 规则参数 → 底部汇总。
// 2026-11-11 改版：英雄用大图，片段用小图标排开，多杀就多几个（docs/app-interaction.md 第 6 节）
import * as api from "./api.js";
import { t } from "./i18n.js";
import { clock, collectionKey, collectionName, el, heroIcon, heroName, heroPic, length, sceneTags } from "./format.js";
import { settings, persist } from "./state.js";

/** `player` / `preselect`（片段文件名）从"已录视频"的"继续录未完成的"带过来：选好玩家、勾上这些片段 */
/** `back` / `backLabel`：左上角的返回，从哪进来回哪（本地录像或已录视频） */
export async function showMatch(view, matchId, { back, backLabel, record, player: wanted, preselect }) {
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
  const preselectNote = el("p", { class: "warn", hidden: true });
  let clips = [];
  let picked = new Set();
  const current = () => info.players.find((p) => p.id === player);

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
    const sel = clips.filter((c) => picked.has(c.index));
    if (!sel.length) return;
    settings.camera = cameraValue;
    settings.compile = compileValue;
    record({
      matchId,
      player,
      hero: current().hero,
      clips: sel,
      camera: cameraValue,
      compile: compileValue,
      overwrite: sel.filter((c) => c.recorded).length,
      secs: sel.reduce((a, c) => a + c.duration, 0),
      minutes: (sel.reduce((a, c) => a + c.estimated_secs, 0) / 60).toFixed(1),
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

  /** 时间线上片段的场景小字（只标一个，越少见越优先） */
  function sceneWord(c) {
    if (c.aegis_steal) return t("scene.aegis");
    if (c.low_hp) return t("scene.lowHp");
    if (c.multi_kill >= 3) return t(`tag.multi.${Math.min(c.multi_kill, 5)}`);
    if (c.teamwork) return t("scene.teamwork");
    return null;
  }

  /** 高光时间线：整场一条线，色块是片段（击杀红、助攻蓝、抢盾金，没勾的变淡），上面挂被杀英雄的小图标和场景小字，点色块也能勾选 */
  function renderTimeline() {
    const p = current();
    const kills = clips.filter((c) => c.kind === "kill").reduce((a, c) => a + c.victims.length, 0);
    const assists = clips.reduce((a, c) => a + (c.kind === "assist" ? c.victims.length : c.assists), 0);
    const aegis = clips.filter((c) => c.aegis_steal).length;
    const total = Math.max(info.duration, ...clips.map((c) => c.end), 1);
    const pct = (s) => `${(Math.max(0, s) / total) * 100}%`;
    const line = el("div", { class: "timeline" }, el("div", { class: "track" }));
    // 红块至少 18 像素宽，挨得很近的两段（比如 21:00–21:16 和 21:16–21:38）会叠在一起、编号压编号：
    // 按时间线大概的像素宽度算，和前一段挤到一起（留不出 2 像素）就放到下一排
    const perPx = total / Math.max(400, window.innerWidth - 110);
    let lastEnd = -Infinity, lastLow = false;
    clips.forEach((c, i) => {
      const off = !picked.has(c.index);
      const low = !lastLow && c.start - lastEnd < 2 * perPx;
      lastEnd = Math.max(c.end, c.start + 18 * perPx);
      lastLow = low;
      line.append(
        el("div", { class: `seg ${c.kind}${off ? " off" : ""}${low ? " low" : ""}`, style: `left:${pct(c.start)};width:max(18px,${pct(c.end - c.start)})`, title: `${clock(c.start)}–${clock(c.end)}`, onclick: () => toggle(c.index) }, String(c.index)),
        // 相邻片段的图标会叠在一起，上下两排交替放
        el("div", { class: `kills ${i % 2 ? "b" : "a"}${off ? " off" : ""}`, style: `left:${pct((c.start + c.end) / 2)}` },
          sceneWord(c) ? el("span", { class: "scene-word" }, sceneWord(c)) : null,
          c.victims.map((v) => heroIcon(v, true))),
      );
    });
    const ticks = el("div", { class: "ticks" });
    for (let m = 0; m * 60 <= total; m += 10) ticks.append(el("span", { style: `left:${pct(m * 60)}` }, `${m}:00`));
    line.append(ticks);
    // 颜色说明只写这场实际出现的类别
    const colors = ["kill", "assist", "aegis"].filter((k) => clips.some((c) => c.kind === k)).map((k) => t(`match.legend.${k}`)).join(t("match.legend.sep"));
    highlight.replaceChildren(
      el("h2", {}, t("match.highlights", { hero: heroName(p.hero), name: p.name })),
      el("p", { class: "sub" }, clips.length
        ? t("match.highlightsSub", { kills, assists, n: clips.length }) + (aegis ? t("match.highlightsAegis", { n: aegis }) : "") + t("match.highlightsHint", { colors })
        : t("match.noClips")),
      clips.length ? line : "",
    );
  }

  function renderSummary() {
    const sel = clips.filter((c) => picked.has(c.index));
    const secs = sel.reduce((a, c) => a + c.duration, 0);
    const est = sel.reduce((a, c) => a + c.estimated_secs, 0);
    const overwrite = sel.filter((c) => c.recorded).length;
    summary.textContent = t("match.summary", {
      n: sel.length, total: clips.length, secs: length(secs), minutes: (est / 60).toFixed(1),
    }) + (overwrite ? t("match.overwrite", { n: overwrite }) : "");
    compileLabel.hidden = sel.length < 2;
    compileName.textContent = t("match.compileName", { hero: heroName(current().hero), name: collectionName(collectionKey(sel)) });
    recordBtn.disabled = sel.length === 0;
    allBtn.hidden = clips.length === 0;
    allBtn.textContent = t(picked.size === clips.length ? "match.selectNone" : "match.selectAll");
  }

  /** 片段列表：两列，一行一个片段；被杀英雄用小图标排开，多杀就多几个 */
  function renderClips() {
    renderTimeline();
    renderChips();
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
        el("span", { class: "t" }, `${clock(c.start)}–${clock(c.end)}`, el("small", {}, length(c.duration))),
        // 抢盾片段没有被杀的英雄，写"肉山"
        el("span", { class: "hero-icons" }, c.victims.length ? c.victims.map((v) => heroIcon(v)) : el("span", { class: "muted" }, t("scene.roshan"))),
        el("span", { class: "end" },
          ...sceneTags(c),
          c.recorded ? el("span", { class: "ok", title: t("match.recordedHint") }, t("match.recorded")) : null),
      );
    })));
    renderSummary();
  }

  async function replan() {
    try {
      clips = (await api.plan(matchId, player, settings.rules)).clips;
    } catch (e) {
      clipsBox.replaceChildren(el("p", { class: "error" }, e.message));
      clips = [];
    }
    // 默认一个都不勾，用户自己挑
    picked = new Set();
    if (pending) {
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
  view.replaceChildren(
    el("button", { type: "button", class: "back", onclick: back }, backLabel),
    el("div", { class: "page-head" },
      el("h1", {}, t("match.title", { id: info.match_id })),
      el("span", { class: "meta" }, clock(info.duration), " · ", result, result ? " · " : "", t("match.build", { build: info.game_build })),
    ),
    playersBox,
    highlight,
    el("div", { class: "card" },
      el("div", { class: "clips-head" }, el("h2", {}, t("match.clips")), el("span", { class: "muted", style: "font-size:13px" }, t("match.clipsHint")), el("div", { class: "spacer" }), allBtn),
      chipsBox, preselectNote, clipsBox,
      el("details", { class: "rules", style: "margin-top:16px" }, el("summary", {}, t("rules.title")), rulesBox)),
    el("div", { class: "footer-bar" }, summary, el("div", { class: "spacer" }), compileLabel, camera, recordBtn),
  );
  renderPlayers();
  renderRules();
  await replan();
}
