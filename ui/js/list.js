// 首页：本地录像列表
import * as api from "./api.js";
import { t } from "./i18n.js";
import { ago, clock, el, heroName } from "./format.js";

export async function showList(view, { openMatch }) {
  view.replaceChildren(el("div", { class: "loading" }, "…"));
  let rows, now;
  try {
    [rows, now] = await Promise.all([api.listReplays(), api.now()]);
  } catch (e) {
    view.replaceChildren(el("div", { class: "card error" }, e.message));
    return;
  }

  const idInput = el("input", { type: "text", placeholder: t("list.idPlaceholder"), inputmode: "numeric" });
  const idError = el("span", { class: "error" });
  const openTyped = () => {
    const id = idInput.value.trim();
    if (!/^\d+$/.test(id)) return;
    if (!rows.some((r) => String(r.match_id) === id)) {
      // 按比赛 ID 下载在里程碑 4 接入，现在只能打开本地已有的
      idError.textContent = t("list.empty");
      return;
    }
    openMatch(Number(id));
  };
  idInput.addEventListener("keydown", (e) => e.key === "Enter" && openTyped());

  const header = el("div", { class: "row" },
    el("h2", { style: "margin:0" }, t("list.title")),
    el("div", { class: "spacer" }),
    idInput,
    el("button", { type: "button", onclick: openTyped }, t("list.openId")),
    el("button", { type: "button", onclick: () => showList(view, { openMatch }) }, t("list.refresh")),
  );

  if (!rows.length) {
    view.replaceChildren(el("div", { class: "card" }, header, el("p", { class: "muted" }, t("list.empty"))));
    return;
  }

  const body = rows.map((r) => {
    let mine = el("span", { class: "muted" }, "—");
    if (r.error) mine = el("span", { class: "error", title: r.error }, t("list.unreadable"));
    else if (r.my_hero) {
      mine = el("span", {}, heroName(r.my_hero), " ",
        el("span", { class: r.my_won ? "win" : "lose" }, t(r.my_won ? "list.win" : "list.lose")));
    }
    return el("tr", { class: "clickable", onclick: () => openMatch(r.match_id) },
      el("td", {}, String(r.match_id)),
      el("td", {}, ago(r.modified, now)),
      el("td", { class: "num" }, r.duration == null ? "—" : clock(r.duration)),
      el("td", {}, mine),
    );
  });

  view.replaceChildren(el("div", { class: "card" },
    header,
    el("p", { class: "muted" }, t("list.hint")),
    idError,
    el("table", {},
      el("thead", {}, el("tr", {},
        el("th", {}, t("list.match")),
        el("th", {}, t("list.downloaded")),
        el("th", { class: "num" }, t("list.duration")),
        el("th", {}, t("list.myHero")),
      )),
      el("tbody", {}, body),
    ),
  ));
}
