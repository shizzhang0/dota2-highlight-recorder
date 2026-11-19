// 播放日志（2026-11-19 起）：应用里放视频的情况记进应用日志（d2hr.log，类别「播放」），查"有时卡顿"用。
// 每次播放记一行开始（首帧用了多久）、一行结束（放了多久、丢了几帧、等过几次数据、界面最长卡了多久），
// 中间只记出问题的那一秒和拖动，正常放的时候不写。读日志：
// - 缓冲剩 0、在等数据：读数据慢（本机播放服务 video.rs）；
// - 缓冲还有、丢帧：解码或画面跟不上；
// - 界面卡住：应用别处在忙（生成缩略图、刷新页面……），拖慢了播放。
// 日志是给开发者看的，固定写中文，不走 i18n
import * as api from "./api.js";
import { clock } from "./format.js";

/** 界面一帧超过这么久没画就算卡住（60Hz 下正常 17ms） */
const UI_STALL_MS = 100;

const log = (text) => api.writeLog("播放", text).catch(() => {});
const sec = (n) => `${n.toFixed(1)}s`;

let session = null;

/** 开始盯一次播放：设好 `video.src` 后调。`name` 是视频的文件名 */
export function watchPlayback(video, name) {
  endPlayback();
  const off = new AbortController();
  const on = (type, fn) => video.addEventListener(type, fn, { signal: off.signal });
  const s = {
    video, name, off,
    opened: performance.now(),
    started: false,
    playedSecs: 0,
    waits: 0,
    maxStall: 0,
    // 这一秒里的情况
    waited: false, stall: 0, seeked: false,
    // 上一次采样：{ at, frames, dropped }；暂停、切到后台时清掉，回来重新开始算
    prev: null,
    // 拖动：从哪儿拖、什么时候开始拖
    lastTime: 0, seekFrom: null, seekAt: 0,
  };
  session = s;

  on("playing", () => {
    if (s.started) return;
    s.started = true;
    const first = (performance.now() - s.opened) / 1000;
    log(`开始  ${name}  时长 ${clock(video.duration || 0)}  首帧 ${sec(first)}`);
  });
  on("waiting", () => {
    if (!s.started) return;
    s.waits++;
    s.waited = true;
  });
  on("timeupdate", () => { if (!video.seeking) s.lastTime = video.currentTime; });
  on("seeking", () => {
    if (s.seekFrom === null) s.seekFrom = s.lastTime;
    s.seekAt = performance.now();
  });
  on("seeked", () => {
    if (s.seekFrom === null) return;
    log(`拖动  ${sec(s.seekFrom)} → ${sec(video.currentTime)}  用了 ${((performance.now() - s.seekAt) / 1000).toFixed(2)}s`);
    s.seekFrom = null;
    s.seeked = true;
    s.lastTime = video.currentTime;
  });
  on("error", () => {
    const e = video.error;
    if (video.getAttribute("src")) log(`出错  ${name}  ${e ? `code ${e.code} ${e.message}` : ""}`);
  });

  // 界面有没有卡：看两次画面之间隔了多久。窗口最小化或切到后台时浏览器本来就不画，不算
  let lastFrame = performance.now();
  const frame = (now) => {
    if (session !== s) return;
    if (!document.hidden) {
      const gap = now - lastFrame;
      s.stall = Math.max(s.stall, gap);
    }
    lastFrame = now;
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);

  s.timer = setInterval(() => sample(s), 1000);
}

/** 每秒看一次：这一秒有丢帧、等数据或界面卡住，就记一行 */
function sample(s) {
  const v = s.video;
  const q = v.getVideoPlaybackQuality();
  const now = performance.now();
  if (v.paused || v.ended || document.hidden) {
    s.prev = null;
    s.waited = s.seeked = false;
    s.stall = 0;
    return;
  }
  const cur = { at: now, frames: q.totalVideoFrames, dropped: q.droppedVideoFrames };
  const p = s.prev;
  s.prev = cur;
  if (!p) {
    s.waited = s.seeked = false;
    s.stall = 0;
    return;
  }
  const secs = (now - p.at) / 1000;
  s.playedSecs += secs;
  const frames = cur.frames - p.frames;
  const dropped = cur.dropped - p.dropped;
  const stall = Math.round(s.stall);
  s.maxStall = Math.max(s.maxStall, stall);
  if (dropped > 0 || s.waited || stall > UI_STALL_MS) {
    const b = v.buffered;
    const ahead = b.length ? Math.max(0, b.end(b.length - 1) - v.currentTime) : 0;
    const span = Math.abs(secs - 1) < 0.2 ? "这一秒" : `这 ${secs.toFixed(1)} 秒`;
    const parts = [`卡顿  进度 ${sec(v.currentTime)}`, `${span} ${frames} 帧（丢 ${dropped}）`, `缓冲剩 ${sec(ahead)}`];
    if (s.waited) parts.push("等数据");
    if (stall > UI_STALL_MS) parts.push(`界面卡住 ${stall}ms`);
    if (s.seeked) parts.push("刚拖动");
    log(parts.join("  "));
  }
  s.waited = s.seeked = false;
  s.stall = 0;
}

/** 这次播放结束（关了播放器、换了视频）：记一行汇总 */
export function endPlayback() {
  const s = session;
  if (!s) return;
  session = null;
  clearInterval(s.timer);
  s.off.abort();
  if (!s.started) {
    log(`没放起来  ${s.name}  等了 ${sec((performance.now() - s.opened) / 1000)}`);
    return;
  }
  const q = s.video.getVideoPlaybackQuality();
  log(`结束  ${s.name}  放了 ${clock(s.playedSecs)}  共 ${q.totalVideoFrames} 帧  丢 ${q.droppedVideoFrames}  等数据 ${s.waits} 次  界面最长卡 ${Math.round(s.maxStall)}ms`);
}
