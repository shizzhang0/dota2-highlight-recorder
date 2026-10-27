// 调用 Rust 侧的命令（src-tauri/src）。命令出错时 Rust 返回字符串，这里统一抛成 Error
const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args).catch((e) => { throw new Error(e); });

export const listReplays = () => invoke("list_replays");
// 失败时抛的是 Rust 侧的 DownloadError（{kind, message?}），不包成 Error，界面按 kind 提示
export const downloadReplay = (matchId) => window.__TAURI__.core.invoke("download_replay", { matchId });
export const deleteReplays = (matchIds) => invoke("delete_replays", { matchIds });
export const openMatch = (matchId) => invoke("open_match", { matchId });
export const plan = (matchId, player, rules) => invoke("plan", { matchId, player, rules });
export const defaultRules = () => invoke("default_rules");
export const heroNames = (language) => invoke("hero_names", { language });
export const reveal = (path) => invoke("reveal", { path });
export const openFile = (path) => invoke("open_file", { path });
export const recordedMatches = () => invoke("recorded_matches");
export const deleteMatchRecord = (matchId, deleteFiles) => invoke("delete_match_record", { matchId, deleteFiles });
export const startRecording = (matchId, player, clips) => invoke("start_recording", { matchId, player, clips });
export const stopRecording = () => invoke("stop_recording");
export const resumeRecording = () => invoke("resume_recording");
export const answerConfirm = (id, yes) => invoke("answer_confirm", { id, yes });
export const isRecording = () => invoke("is_recording");
export const gameOpen = () => invoke("game_open");
export const quitGame = () => invoke("quit_game");
export const getSettings = () => invoke("get_settings");
export const saveSettings = (settings) => invoke("save_settings", { settings });
