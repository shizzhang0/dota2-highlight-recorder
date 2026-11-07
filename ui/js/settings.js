// 设置对话框
import { LANGS, t } from "./i18n.js";
import { el } from "./format.js";
import { reveal } from "./api.js";
import { replaceSettings, settings } from "./state.js";

export function openSettings(dialog, { onSaved }) {
  const out = el("input", { type: "text", value: settings.out_dir });
  // 录制画面大小：1080p（较快）或跟游戏窗口一致
  const capture = el("select", {},
    el("option", { value: "1080p" }, t("settings.capture.1080p")),
    el("option", { value: "native" }, t("settings.capture.native")));
  capture.value = settings.capture === "native" ? "native" : "1080p";
  const compile = el("input", { type: "checkbox" });
  compile.checked = settings.compile;
  const quit = el("input", { type: "checkbox" });
  quit.checked = settings.quit_when_done;
  // 1080p 录完一定退出游戏（要把显示设置改回去），"录完退出游戏"只在保持原尺寸时有用
  const quitLabel = el("label", {}, t("settings.quitAfter"));
  const quitRow = el("label", {}, quit, " ", t("settings.quitAfter.hint"));
  const syncQuit = () => { quitLabel.hidden = quitRow.hidden = capture.value !== "native"; };
  capture.addEventListener("change", syncQuit);
  syncQuit();
  const lang = el("select", {},
    el("option", { value: "auto" }, t("settings.language.auto")),
    LANGS.map(([v, name]) => el("option", { value: v }, name)));
  lang.value = settings.language;
  const ffmpeg = el("input", { type: "text", value: settings.ffmpeg, placeholder: t("settings.ffmpeg.auto") });

  const save = async () => {
    await replaceSettings({
      ...settings,
      out_dir: out.value.trim() || settings.out_dir,
      capture: capture.value,
      compile: compile.checked,
      quit_when_done: quit.checked,
      language: lang.value,
      ffmpeg: ffmpeg.value.trim(),
    });
    dialog.close();
    onSaved();
  };

  dialog.replaceChildren(
    el("h2", {}, t("settings.title")),
    el("div", { class: "form" },
      el("label", {}, t("settings.outDir")),
      el("div", { class: "row", style: "flex-wrap:nowrap" }, out,
        el("button", { type: "button", onclick: () => reveal(out.value.trim() || settings.out_dir) }, t("settings.outDir.open"))),
      el("label", {}, t("settings.capture")), capture,
      el("div", { class: "hint" }, t("settings.capture.hint")),
      el("label", {}, t("settings.compile")), el("label", {}, compile, " ", t("settings.compile.hint")),
      quitLabel, quitRow,
      el("label", {}, t("settings.language")), lang,
      el("label", {}, "ffmpeg"), ffmpeg,
      el("div", { class: "hint" }, t("settings.ffmpeg.hint")),
    ),
    el("div", { class: "dialog-actions" },
      el("button", { type: "button", onclick: () => dialog.close() }, t("settings.cancel")),
      el("button", { type: "button", class: "primary", onclick: save }, t("settings.save")),
    ),
  );
  dialog.showModal();
}
