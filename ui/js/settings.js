// 设置页（导航第三项）。原先是对话框，2026-11-10 改成页面，和"比赛""已录视频"一样点了换页；
// 保留「保存」按钮（用户定的），改了没保存就切走时先问（`leaveSettings`）
import { LANGS, t } from "./i18n.js";
import { el } from "./format.js";
import { reveal } from "./api.js";
import { replaceSettings, settings } from "./state.js";
import { appVersion, checkUpdate, installUpdate, isInstalled, lastUpdate, onUpdate, openReleasePage } from "./update.js";
import { recordingMatch } from "./record.js";

/** 「关于」：版本号、检查更新、一键更新。不属于表单，不参与"有没有改过" */
function aboutRow(row) {
  const version = el("span", { class: "muted" });
  const status = el("span", { class: "muted" });
  const checkBtn = el("button", { type: "button" }, t("upd.check"));
  const actionBtn = el("button", { type: "button", class: "primary", hidden: true });
  const control = [status, actionBtn, checkBtn];
  const mb = (n) => (n / 1e6).toFixed(1);

  const show = (r) => {
    if (!checkBtn.isConnected && r) return false;
    actionBtn.hidden = true;
    checkBtn.disabled = r?.state === "checking";
    status.className = "muted";
    if (!r) status.textContent = "";
    else if (r.state === "checking") status.textContent = t("upd.checking");
    else if (r.state === "latest") status.textContent = t("upd.latest");
    else if (r.state === "fail") status.textContent = t("upd.fail");
    else if (r.state === "new") {
      status.className = "upd-new";
      status.textContent = t("upd.new", { version: r.version });
      isInstalled().then((installed) => {
        actionBtn.hidden = false;
        actionBtn.textContent = t(installed ? "upd.install" : "upd.download");
        // 录制中不装：装新版会结束程序，游戏设置来不及恢复
        const busy = installed && recordingMatch() !== null;
        actionBtn.disabled = busy;
        actionBtn.title = busy ? t("upd.recording") : "";
        actionBtn.onclick = installed ? install : openReleasePage;
      });
    }
    return true;
  };
  const install = async () => {
    actionBtn.disabled = checkBtn.disabled = true;
    status.className = "muted";
    status.textContent = t("upd.starting");
    const err = await installUpdate((done, total) => {
      status.textContent = total ? t("upd.downloading", { done: mb(done), total: mb(total) }) : t("upd.downloadingNoTotal", { done: mb(done) });
    });
    // 走到这里就是没装上
    checkBtn.disabled = false;
    actionBtn.disabled = false;
    status.className = "error";
    status.textContent = err === "recording" ? t("upd.recording") : err === "portable" ? t("upd.portable") : t("upd.err", { error: err });
  };
  checkBtn.addEventListener("click", () => checkUpdate());
  appVersion().then((v) => { version.textContent = `v${v}`; }).catch(() => {});
  const off = onUpdate((r) => { if (!show(r)) off(); });
  show(lastUpdate());
  return row(t("upd.version"), version, control);
}

/** 当前设置页表单上的值；不在设置页时为 null */
let form = null;

/** 表单上的值和已保存的不一样 */
export function settingsDirty() {
  if (!form) return false;
  const v = form();
  return Object.keys(v).some((k) => v[k] !== settings[k]);
}

/** 保存表单上的值 */
async function saveForm() {
  const v = form();
  await replaceSettings({ ...settings, ...v, out_dir: v.out_dir || settings.out_dir });
}

/** 要离开设置页时调：没改过直接返回 true；改了没保存就问保存、不保存还是留下，返回能不能离开 */
export function leaveSettings(dialog) {
  if (!settingsDirty()) {
    form = null;
    return Promise.resolve(true);
  }
  return new Promise((done) => {
    const close = async (choice) => {
      dialog.close();
      if (choice === "save") await saveForm();
      if (choice !== "stay") form = null;
      done(choice !== "stay");
    };
    dialog.replaceChildren(
      el("h2", {}, t("settings.unsaved.title")),
      el("p", {}, t("settings.unsaved.what")),
      el("div", { class: "dialog-actions" },
        el("button", { type: "button", onclick: () => close("stay") }, t("settings.unsaved.stay")),
        el("button", { type: "button", onclick: () => close("discard") }, t("settings.unsaved.discard")),
        el("button", { type: "button", class: "primary", onclick: () => close("save") }, t("settings.save")),
      ),
    );
    // 按 Esc 当作留下
    dialog.addEventListener("cancel", () => done(false), { once: true });
    dialog.showModal();
  });
}

/** 设置页：按用途分组（录制、输出、界面、高级），每项左边是名称和说明，右边是控件；底部固定一条保存栏。
 * 保存后调 `onSaved()`（语言可能改了，调用方重新加载词条、重画） */
export function showSettings(view, { onSaved }) {
  // 一项设置：名称 + 一句说明 + 控件。stack = 控件放到下面占满一行（路径之类长的输入框）
  const row = (label, desc, control, stack = false) => el("div", { class: stack ? "setting stack" : "setting" },
    el("div", {}, el("div", { class: "setting-name" }, label), desc ? el("div", { class: "setting-desc" }, desc) : ""),
    el("div", { class: "setting-control" }, control));
  const group = (title, ...rows) => el("section", { class: "settings-group" }, el("h3", {}, title), rows);
  const toggle = (checked, label) => {
    const box = el("input", { type: "checkbox", class: "switch", "aria-label": label });
    box.checked = checked;
    return box;
  };

  // 录制画面：1080p（较快）或跟游戏窗口一致，两段切换（和比赛页的镜头一样）
  const captures = ["1080p", "native"];
  let captureValue = settings.capture === "native" ? "native" : "1080p";
  const captureButtons = captures.map((v) => el("button", { type: "button", role: "radio", onclick: () => setCapture(v) }, t(`settings.capture.${v}`)));
  const compile = toggle(settings.compile, t("settings.compile"));
  const quit = toggle(settings.quit_when_done, t("settings.quitAfter"));
  // 1080p 录完一定退出游戏（要把显示设置改回去），"录完退出游戏"只在跟游戏窗口一致时有用
  const quitRow = row(t("settings.quitAfter"), t("settings.quitAfter.hint"), quit);
  const out = el("input", { type: "text", value: settings.out_dir, "aria-label": t("settings.outDir") });
  const lang = el("select", { "aria-label": t("settings.language") },
    el("option", { value: "auto" }, t("settings.language.auto")),
    LANGS.map(([v, n]) => el("option", { value: v }, n)));
  lang.value = settings.language;
  const ffmpeg = el("input", { type: "text", value: settings.ffmpeg, placeholder: t("settings.ffmpeg.auto"), "aria-label": "ffmpeg" });

  form = () => ({
    out_dir: out.value.trim(),
    capture: captureValue,
    compile: compile.checked,
    quit_when_done: quit.checked,
    language: lang.value,
    ffmpeg: ffmpeg.value.trim(),
  });

  // 底部保存栏：有没保存的修改时按钮亮起、显示提示；保存后显示"已保存"
  const status = el("span", { class: "muted" });
  const saveBtn = el("button", { type: "button", class: "primary" }, t("settings.save"));
  const revertBtn = el("button", { type: "button" }, t("settings.revert"));
  const sync = () => {
    const dirty = settingsDirty();
    saveBtn.disabled = revertBtn.disabled = !dirty;
    if (dirty) status.textContent = t("settings.unsaved.hint");
    else if (status.textContent === t("settings.unsaved.hint")) status.textContent = "";
  };
  const setCapture = (v) => {
    captureValue = v;
    captureButtons.forEach((b, i) => {
      b.classList.toggle("on", captures[i] === v);
      b.setAttribute("aria-checked", String(captures[i] === v));
    });
    quitRow.hidden = v !== "native";
    sync();
  };
  saveBtn.addEventListener("click", async () => {
    saveBtn.disabled = true;
    await saveForm();
    await onSaved();
  });
  revertBtn.addEventListener("click", () => showSettings(view, { onSaved }));

  const card = el("div", { class: "card settings" },
    el("h2", {}, t("settings.title")),
    group(t("settings.group.record"),
      row(t("settings.capture"), t("settings.capture.hint"),
        el("div", { class: "segmented", role: "radiogroup", "aria-label": t("settings.capture") }, captureButtons)),
      row(t("settings.compile"), t("settings.compile.hint"), compile),
      quitRow),
    group(t("settings.group.output"),
      row(t("settings.outDir"), t("settings.outDir.hint"), [out,
        el("button", { type: "button", onclick: () => reveal(out.value.trim() || settings.out_dir) }, t("settings.outDir.open"))], true)),
    group(t("settings.group.ui"),
      row(t("settings.language"), "", lang)),
    group(t("settings.group.advanced"),
      row("ffmpeg", t("settings.ffmpeg.hint"), ffmpeg, true)),
    group(t("settings.group.about"), aboutRow(row)),
  );
  card.addEventListener("input", sync);
  card.addEventListener("change", sync);

  view.replaceChildren(card,
    el("div", { class: "footer-bar settings-bar" }, status, el("div", { class: "spacer" }), revertBtn, saveBtn));
  setCapture(captureValue);
  return { saved: () => { status.textContent = t("settings.saved"); } };
}
