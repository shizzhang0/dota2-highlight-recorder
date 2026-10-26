// 比赛页：选玩家 → 片段列表（可勾选）→ 规则参数 → 底部汇总
import * as api from "./api.js";
import { t } from "./i18n.js";
import { clock, el, heroName, length } from "./format.js";
import { settings, persist } from "./state.js";

export async function showMatch(view, matchId, { back, record }) {
  view.replaceChildren(el("div", { class: "loading" }, t("match.parsing")));
  let info;
  try {
    info = await api.openMatch(matchId);
  } catch (e) {
    view.replaceChildren(el("button", { type: "button", onclick: back }, t("nav.back")), el("div", { class: "card error" }, e.message));
    return;
  }

  let player = info.me ?? info.players[0].id;
  let clips = [];
  let picked = new Set();

  const playersBox = el("div", { class: "teams" });
  const clipsBox = el("div");
  const summary = el("span", { class: "muted" });
  const recordBtn = el("button", { type: "button", class: "primary" }, t("match.record"));
  recordBtn.addEventListener("click", () => {
    const sel = clips.filter((c) => picked.has(c.index));
    if (!sel.length) return;
    record({
      matchId,
      player,
      clips: sel,
      secs: sel.reduce((a, c) => a + c.duration, 0),
      minutes: (sel.reduce((a, c) => a + c.estimated_secs, 0) / 60).toFixed(1),
    });
  });

  function renderPlayers() {
    const team = (id, label, won) => el("div", { class: `team ${id === 2 ? "radiant" : "dire"}` },
      el("h3", {}, label, won ? ` · ${t("match.won")}` : ""),
      info.players.filter((p) => p.team === id).map((p) =>
        el("button", { type: "button", class: `player${p.id === player ? " selected" : ""}`, onclick: () => pick(p.id) },
          el("span", { class: "hero" }, heroName(p.hero), p.is_me ? el("span", { class: "me" }, t("match.me")) : null),
          el("span", { class: "name", title: p.name }, p.name),
          el("span", { class: "kd" }, `${p.kills} / ${p.deaths}`),
        )),
    );
    playersBox.replaceChildren(
      team(2, t("match.radiant"), info.radiant_won === true),
      team(3, t("match.dire"), info.radiant_won === false),
    );
  }

  function renderSummary() {
    const sel = clips.filter((c) => picked.has(c.index));
    const secs = sel.reduce((a, c) => a + c.duration, 0);
    const est = sel.reduce((a, c) => a + c.estimated_secs, 0);
    summary.textContent = t("match.summary", {
      n: sel.length, total: clips.length, secs: length(secs), minutes: (est / 60).toFixed(1),
    });
    recordBtn.disabled = sel.length === 0;
  }

  function tags(c) {
    const out = [];
    if (c.multi_kill >= 2) out.push(el("span", { class: "tag" }, t(`tag.multi.${Math.min(c.multi_kill, 5)}`)));
    if (c.kill_streak >= 3) out.push(el("span", { class: "tag" }, t("tag.streak", { n: c.kill_streak })));
    return out;
  }

  function renderClips() {
    if (!clips.length) {
      clipsBox.replaceChildren(el("p", { class: "muted" }, t("match.noClips")));
      renderSummary();
      return;
    }
    const all = el("input", { type: "checkbox", title: t("match.selectAll") });
    all.checked = picked.size === clips.length;
    all.addEventListener("change", () => {
      picked = all.checked ? new Set(clips.map((c) => c.index)) : new Set();
      renderClips();
    });
    const rows = clips.map((c) => {
      const box = el("input", { type: "checkbox" });
      box.checked = picked.has(c.index);
      box.addEventListener("change", () => {
        box.checked ? picked.add(c.index) : picked.delete(c.index);
        all.checked = picked.size === clips.length;
        renderSummary();
      });
      return el("tr", { title: c.file_name },
        el("td", {}, box),
        el("td", { class: "num" }, String(c.index)),
        el("td", {}, `${clock(c.start)} – ${clock(c.end)}`),
        el("td", { class: "num" }, length(c.duration)),
        el("td", { class: "wrap" }, c.victims.map(heroName).join(", ")),
        el("td", {}, tags(c)),
        el("td", { class: "num" }, String(c.exports)),
      );
    });
    clipsBox.replaceChildren(el("table", {},
      el("thead", {}, el("tr", {},
        el("th", {}, all), el("th", { class: "num" }, "#"),
        el("th", {}, t("match.col.time")), el("th", { class: "num" }, t("match.col.length")),
        el("th", {}, t("match.col.kills")), el("th", {}, t("match.col.tags")),
        el("th", { class: "num" }, t("match.col.exports")),
      )),
      el("tbody", {}, rows),
    ));
    renderSummary();
  }

  async function replan() {
    try {
      clips = (await api.plan(matchId, player, settings.rules)).clips;
    } catch (e) {
      clipsBox.replaceChildren(el("p", { class: "error" }, e.message));
      clips = [];
    }
    picked = new Set(clips.map((c) => c.index));
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

  view.replaceChildren(
    el("div", { class: "row", style: "margin-bottom:12px" },
      el("button", { type: "button", onclick: back }, t("nav.back")),
      el("h2", { style: "margin:0" }, t("match.title", { id: info.match_id })),
      el("span", { class: "muted" }, t("match.meta", { duration: clock(info.duration), build: info.game_build })),
    ),
    el("div", { class: "card" }, el("h2", {}, t("match.pickPlayer")), playersBox),
    el("div", { class: "card" }, el("h2", {}, t("match.clips")), clipsBox,
      el("details", { class: "rules", style: "margin-top:14px" }, el("summary", {}, t("rules.title")), rulesBox)),
    el("div", { class: "footer-bar" }, summary, el("div", { class: "spacer" }), recordBtn),
  );
  renderPlayers();
  renderRules();
  await replan();
}
