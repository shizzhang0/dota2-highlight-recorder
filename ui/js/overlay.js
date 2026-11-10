// 录制时盖在 Dota 画面左上角的置顶进度窗：第几个片段、这一段的进度、用时和预计、拦下了几次操作，一个停止按钮。
// 位置和显示/收起由 Rust 侧管（跟着 Dota 窗口走），这里只听事件
import { loadLang, resolve, t } from "./i18n.js";
import { clock } from "./format.js";
import { describe } from "./events.js";

const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const $ = (id) => document.getElementById(id);
const box = document.querySelector(".box");
const listen = (name, fn) => window.__TAURI__.event.listen(name, (e) => fn(e.payload));

let estimated = 0, t0 = 0, timer = null, stopping = false;

function tick() {
  $("time").textContent = t("ov.time", { elapsed: clock((Date.now() - t0) / 1000), total: clock(estimated) });
}

/** 状态：recording（红）/ stopping（黄） */
function setState(state) {
  box.dataset.state = state;
}

function setBar(fraction) {
  $("fill").style.width = `${Math.min(100, fraction * 100)}%`;
}

async function start() {
  const settings = await invoke("get_settings");
  await loadLang(resolve(settings.language));
  $("stop").textContent = t("ov.stop");
  $("stop").addEventListener("click", () => {
    $("stop").disabled = true;
    invoke("stop_recording");
  });

  listen("record-started", (s) => {
    estimated = s.estimated_secs;
    t0 = 0;
    stopping = false;
    clearInterval(timer);
    setState("recording");
    $("title").textContent = t("ov.preparing");
    $("step").textContent = t("rec.starting");
    $("time").textContent = "";
    setBar(0);
    $("hint").textContent = t("ov.hint");
    $("hint").classList.remove("blocked");
    $("stop").disabled = false;
  });

  listen("record-event", (e) => {
    if (stopping) return;
    const text = describe(e);
    if (e.kind === "recording") {
      // 开始录才计时（启动游戏、加载录像的时间不算在预计里）
      t0 = Date.now();
      clearInterval(timer);
      timer = setInterval(tick, 500);
      tick();
    } else if (e.kind === "clip_started") {
      $("title").textContent = t("ov.clip", { n: e.n, total: e.total });
      setBar(0);
    } else if (e.kind === "segment_progress") {
      setBar(e.frames / e.total);
    } else if (e.kind === "segment_done") {
      setBar(1);
    }
    // 每半秒一次的帧数进度只走进度条，不换文字
    if (text) $("step").textContent = e.kind === "clip_started" ? e.name : text;
  });

  listen("record-blocked", (n) => {
    if (stopping) return;
    $("hint").textContent = t("ov.blocked", { n });
    $("hint").classList.add("blocked");
  });

  // 点了停止：流程还要做完手上这一步、恢复设置，先告诉用户"收到了"
  listen("record-stopping", (reason) => {
    stopping = true;
    clearInterval(timer);
    setState("stopping");
    $("title").textContent = t("ov.stopping");
    $("step").textContent = reason;
    $("stop").disabled = true;
  });

  // 录完、停下或出错时 Rust 侧立刻收起这个窗口，结果看主窗口和系统通知
  listen("record-finished", () => clearInterval(timer));
}

start();
