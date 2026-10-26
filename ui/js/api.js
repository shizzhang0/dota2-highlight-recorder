// 调用 Rust 侧的命令（src-tauri/src）。命令出错时 Rust 返回字符串，这里统一抛成 Error
const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args).catch((e) => { throw new Error(e); });

export const listReplays = () => invoke("list_replays");
export const openMatch = (matchId) => invoke("open_match", { matchId });
export const plan = (matchId, player, rules) => invoke("plan", { matchId, player, rules });
export const defaultRules = () => invoke("default_rules");
export const now = () => invoke("now");
export const heroNames = (language) => invoke("hero_names", { language });
export const reveal = (path) => invoke("reveal", { path });
export const getSettings = () => invoke("get_settings");
export const saveSettings = (settings) => invoke("save_settings", { settings });
