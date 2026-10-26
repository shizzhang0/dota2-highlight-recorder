// 设置对话框
import { LANGS, t } from "./i18n.js";
import { el } from "./format.js";
import { replaceSettings, settings } from "./state.js";

const RESOLUTIONS = ["", "1920x1080", "2560x1440", "2560x1600", "1600x900", "1280x720"];

export function openSettings(dialog, { onSaved }) {
  const out = el("input", { type: "text", value: settings.out_dir });
  const res = el("select", {}, RESOLUTIONS.map((r) => el("option", { value: r }, r || t("settings.resolution.window"))));
  if (!RESOLUTIONS.includes(settings.resolution)) res.append(el("option", { value: settings.resolution }, settings.resolution));
  res.value = settings.resolution;
  const compile = el("input", { type: "checkbox" });
  compile.checked = settings.compile;
  const quit = el("input", { type: "checkbox" });
  quit.checked = settings.quit_after;
  const lang = el("select", {},
    el("option", { value: "auto" }, t("settings.language.auto")),
    LANGS.map(([v, name]) => el("option", { value: v }, name)));
  lang.value = settings.language;

  const save = async () => {
    await replaceSettings({
      ...settings,
      out_dir: out.value.trim() || settings.out_dir,
      resolution: res.value,
      compile: compile.checked,
      quit_after: quit.checked,
      language: lang.value,
    });
    dialog.close();
    onSaved();
  };

  dialog.replaceChildren(
    el("h2", {}, t("settings.title")),
    el("div", { class: "form" },
      el("label", {}, t("settings.outDir")), out,
      el("label", {}, t("settings.resolution")), res,
      el("div", { class: "hint" }, t("settings.resolution.hint")),
      el("label", {}, t("settings.compile")), el("label", {}, compile, " ", t("settings.compile.hint")),
      el("label", {}, t("settings.quitAfter")), el("label", {}, quit),
      el("label", {}, t("settings.language")), lang,
    ),
    el("div", { class: "dialog-actions" },
      el("button", { type: "button", onclick: () => dialog.close() }, t("settings.cancel")),
      el("button", { type: "button", class: "primary", onclick: save }, t("settings.save")),
    ),
  );
  dialog.showModal();
}
