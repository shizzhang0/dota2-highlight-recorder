// 录制事件（Rust 侧 pipeline::Event）→ 一行文字，显示在主窗口的进度页
import { t } from "./i18n.js";
import { clock } from "./format.js";

/** 事件 → 一行文字；不值得显示的返回 null（每半秒一次的帧数进度只用来走进度条） */
export function describe(e) {
  switch (e.kind) {
    case "waiting_for_dota_to_quit": return t("ev.waitQuit");
    case "launching_game": return t("ev.launch", { args: e.args.join(" ") });
    case "replay_loaded": return t("ev.loaded");
    case "recording": return t("ev.recording", { encoder: e.encoder });
    case "clip_started": return t("ev.clip", { n: e.n, total: e.total, name: e.name });
    case "segment_started": return t("ev.segment", { from: clock(e.from), to: clock(e.to) });
    case "segment_done": return t("ev.segmentDone", { from: clock(e.from), to: clock(e.to), secs: e.secs.toFixed(1), wall: Math.round(e.wall_secs) });
    case "retrying": return t("ev.retry", { reason: e.reason });
    case "clip_done": return t("ev.clipDone", { path: e.path, secs: e.secs.toFixed(1) });
    case "compilation_done": return t("ev.compiled", { path: e.path, secs: e.secs.toFixed(1) });
    case "restoring_window_size": return t("ev.restoringSize", { w: e.width, h: e.height });
    case "window_size_restored": return t("ev.sizeRestored", { w: e.width, h: e.height });
    case "window_size_not_restored": return t("ev.sizeNotRestored", { w: e.width, h: e.height, error: e.error });
    case "restore_failed": return t("ev.restoreFailed", { error: e.error });
    case "diagnostics_kept": return t("ev.diag", { dir: e.dir });
    default: return null;
  }
}
