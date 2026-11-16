import { t } from "./i18n.js";

/** 秒 → m:ss（号角前为负） */
export function clock(secs) {
  const sign = secs < 0 ? "-" : "";
  const s = Math.round(Math.abs(secs));
  return `${sign}${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** 秒 → "12.3 秒" / "1:05" 这类时长 */
export function length(secs) {
  return secs < 60 ? `${secs.toFixed(1)}s` : clock(secs);
}

let heroes = {};

/** 换语言时调用：英雄名表来自游戏自带的本地化文件 */
export function setHeroNames(map) {
  heroes = map || {};
}

/** 内部英雄名 → 显示名；游戏文件里读不到时把内部名整理成英文 */
export function heroName(id) {
  return heroes[id] ?? id.split("_").map((w) => w.charAt(0).toUpperCase() + w.slice(1)).join(" ");
}

// 英雄图片取自 Valve 自己的图片服务器，只用英雄名请求（docs/app-interaction.md 第 6 节）
const HERO_CDN = "https://cdn.cloudflare.steamstatic.com/apps/dota2/images/dota_react/heroes/";

/** 图片加载不了（没联网、被墙）就换成写着英雄名的方块，大小和图片一样 */
function heroImg(src, id, cls, short) {
  const name = heroName(id);
  const img = el("img", { class: cls, src, alt: name, title: name, loading: "lazy" });
  img.addEventListener("error", () => img.replaceWith(el("span", { class: `${cls} hero-fallback`, title: name }, short ? name.slice(0, 1) : name)), { once: true });
  return img;
}

/** 英雄立绘（16:9 大图） */
export function heroPic(id) {
  return heroImg(`${HERO_CDN}${id}.png`, id, "hero-pic", false);
}

/** 英雄小图标；`small` 用在时间线、缩略图下面这种挤的地方 */
export function heroIcon(id, small = false) {
  return heroImg(`${HERO_CDN}icons/${id}.png`, id, small ? "hero-icon sm" : "hero-icon", true);
}

/** Unix 秒 → 本地时间 "10-26 21:10" */
export function date(secs) {
  const d = new Date(secs * 1000);
  const p = (n) => String(n).padStart(2, "0");
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** 文件路径 → 文件名 */
export function baseName(path) {
  return path.split(/[\\/]/).pop();
}

/** 生成元素：el("td", { class: "num" }, "文字", 子元素…) */
export function el(tag, attrs = {}, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === undefined || v === null || v === false) continue;
    if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else if (k === "class") e.className = v;
    else e.setAttribute(k, v === true ? "" : v);
  }
  e.append(...children.flat().filter((c) => c !== null && c !== undefined));
  return e;
}

/** 游戏模式名；没有模式或词条里没有这个模式（教程等）时为 null，不显示 */
export function modeName(mode) {
  if (mode == null) return null;
  const key = `mode.${mode}`;
  const name = t(key);
  return name === key ? null : name;
}

/** 游戏模式标签（本地录像）：加速模式琥珀色，全英雄选择蓝色，其他灰色 */
export function modeTag(mode) {
  const name = modeName(mode);
  const color = mode === 23 ? " turbo" : mode === 22 || mode === 1 ? " all-pick" : "";
  return name && el("span", { class: `mode${color}` }, name);
}

// 赛后评选的图标：MVP 皇冠，荣誉提名星星（2026-11-16 用户选的「图标加文字、不要底色」）
const AWARD_ICONS = {
  mvp: '<svg viewBox="0 0 24 22" width="14" height="13" fill="currentColor" aria-hidden="true"><path d="M2 6l5 5 5-9 5 9 5-5-2 14H4z"/></svg>',
  honorable: '<svg viewBox="0 0 24 24" width="13" height="13" fill="currentColor" aria-hidden="true"><path d="M12 2l3 7 7 .6-5.3 4.6L18.4 22 12 18l-6.4 4 1.7-7.8L2 9.6 9 9z"/></svg>',
};

/** 赛后评选标签："mvp" / "honorable"，其他（没拿到）为 null */
export function awardTag(award) {
  if (!AWARD_ICONS[award]) return null;
  const tag = el("span", { class: `award ${award}`, title: t(`award.${award}.title`) });
  tag.innerHTML = AWARD_ICONS[award];
  tag.append(t(`award.${award}`));
  return tag;
}
