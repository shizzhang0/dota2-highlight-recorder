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

/** 多久以前 */
export function ago(thenSecs, nowSecs) {
  const d = Math.max(0, nowSecs - thenSecs);
  if (d < 3600) return t("ago.minutes", { n: Math.floor(d / 60) });
  if (d < 86400) return t("ago.hours", { n: Math.floor(d / 3600) });
  return t("ago.days", { n: Math.floor(d / 86400) });
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
