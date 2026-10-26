// 界面语言：词条在 ui/lang/*.json，缺键回退中文，再缺才显示键名。做法同 dota2-game-helper2
const FALLBACK = "zh-CN";
const cache = {};
let dict = {}, fb = {};

function load(lang) {
  cache[lang] ??= fetch(`lang/${lang}.json`).then((r) => r.json());
  return cache[lang];
}

/** "auto" 时跟随系统：中文系统用中文，其他用英文 */
export function resolve(setting) {
  if (setting && setting !== "auto") return setting;
  return navigator.language?.toLowerCase().startsWith("zh") ? "zh-CN" : "en";
}

export async function loadLang(lang) {
  fb = await load(FALLBACK).catch(() => ({}));
  dict = lang === FALLBACK ? fb : await load(lang).catch(() => ({}));
  document.documentElement.lang = lang;
}

/** 取词条，`{name}` 换成 vars 里的值 */
export function t(key, vars = {}) {
  const s = dict[key] ?? fb[key] ?? key;
  return s.replace(/\{(\w+)\}/g, (_, k) => vars[k] ?? "");
}

/** 给带 data-t 的静态元素填字 */
export function applyStatic(root = document) {
  for (const el of root.querySelectorAll("[data-t]")) el.textContent = t(el.dataset.t);
  document.title = t("app.title");
}

// 语言名用它自己的语言写
export const LANGS = [["zh-CN", "简体中文"], ["en", "English"]];
