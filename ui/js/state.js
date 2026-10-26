// 全局状态：设置在启动时读一次，改了就整份写回（Rust 侧 settings.rs）
import * as api from "./api.js";

export let settings = null;

export async function loadSettings() {
  settings = await api.getSettings();
  return settings;
}

export function persist() {
  return api.saveSettings(settings).catch((e) => console.error("保存设置失败", e));
}

export function replaceSettings(next) {
  settings = next;
  return persist();
}
