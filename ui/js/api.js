// 调用 Rust 侧的命令（src-tauri/src）。命令出错时 Rust 返回字符串，这里统一抛成 Error
// 等了超过 SLOW_MS 才回来的调用记进日志（write_log 自己除外）：查录完后页面和后台之间的消息卡住（2026-11-19）
const SLOW_MS = 3000;
const invoke = (cmd, args) => {
  const t0 = Date.now();
  return window.__TAURI__.core.invoke(cmd, args)
    .finally(() => {
      const ms = Date.now() - t0;
      if (ms > SLOW_MS && cmd !== "write_log") window.__TAURI__.core.invoke("write_log", { kind: "界面", text: `调用 ${cmd} 等了 ${(ms / 1000).toFixed(1)} 秒` }).catch(() => {});
    })
    .catch((e) => { throw new Error(e); });
};

export const listReplays = () => invoke("list_replays");
// 失败时抛的是 Rust 侧的 DownloadError（{kind, message?}），不包成 Error，界面按 kind 提示
export const downloadReplay = (matchId) => window.__TAURI__.core.invoke("download_replay", { matchId });
export const cancelDownload = (matchId) => invoke("cancel_download", { matchId });
export const deleteReplays = (matchIds) => invoke("delete_replays", { matchIds });
export const openMatch = (matchId) => invoke("open_match", { matchId });
export const plan = (matchId, player, rules) => invoke("plan", { matchId, player, rules });
export const defaultRules = () => invoke("default_rules");
export const heroNames = (language) => invoke("hero_names", { language });
export const reveal = (path) => invoke("reveal", { path });
export const videoThumb = (path) => invoke("video_thumb", { path });
export const openFile = (path) => invoke("open_file", { path });
export const videoUrl = (path) => invoke("video_url", { path });
// 应用日志（d2hr.log）：界面只能写「播放」「界面」两类
export const writeLog = (kind, text) => invoke("write_log", { kind, text });
export const openLogs = () => invoke("open_logs");
export const accounts = () => invoke("accounts");
export const setAccount = (id) => invoke("set_account", { id });
export const recordedMatches = () => invoke("recorded_matches");
export const deleteMatchRecord = (matchId, deleteFiles) => invoke("delete_match_record", { matchId, deleteFiles });
// `products`：「推荐」选的成品标识，不空时按成品录（`clips` 是它们用到的片段，进度页显示用）；空时录「自定义」勾的 `clips`
export const startRecording = (matchId, player, clips, products, camera, compile, lang) => invoke("start_recording", { matchId, player, clips, products, camera, compile, lang });
export const stopRecording = () => invoke("stop_recording");
export const answerConfirm = (id, yes) => invoke("answer_confirm", { id, yes });
export const isRecording = () => invoke("is_recording");
// 上一次录制的结果（录完没收到 record-finished 时用）；还在录为 null
export const recordResult = () => invoke("record_result");
export const gameOpen = () => invoke("game_open");
export const quitGame = () => invoke("quit_game");
export const getSettings = () => invoke("get_settings");
export const saveSettings = (settings) => invoke("save_settings", { settings });
