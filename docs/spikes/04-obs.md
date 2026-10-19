# 技术验证 4：控制 OBS

结论：**可行**。能通过 obs-websocket 建专用场景、抓 Dota 窗口、开始 / 暂停 / 继续 / 停止录制，录完恢复原场景，不改动用户自己的场景。
代码在 [`crates/obs`](../../crates/obs)，用的是 [obws](https://crates.io/crates/obws) 0.15。

## 怎么跑

```bash
# 纯色画面：建专用场景 → 录 4 秒 → 暂停 3 秒 → 录 4 秒 → 停止 → 切回原场景、删掉专用场景
cargo run --release -p obs --example record_test

# 用游戏捕获抓 Dota 窗口（连同游戏声音），并让 OBS 对捕获源截一张图
cargo run --release -p obs --example record_test -- --window "Dota 2:SDL_app:dota2.exe" --shot <截图.png>
```

测试环境：OBS 32.2.2，obs-websocket 5.7.4。

## 连接

- WebSocket 的端口和密码直接读 OBS 的配置文件 `%APPDATA%\obs-studio\plugin_config\obs-websocket\config.json`，用户不用手动填。
- 这个文件要在 OBS 的设置窗口点"确定"后才会写入；`server_enabled` 是 false 时直接提示用户去开。

## 录制

- **暂停的时间不会进入视频**：录 4 秒、暂停 3 秒、再录 4 秒，输出时长 7.9 秒。
  所以"片段内击杀之间的空档自动剪掉"可以直接用 `PauseRecord` / `ResumeRecord` 实现，不用事后再剪。
- **`StopRecord` 返回时，文件还没写完**：停止后立刻读到的大小（188KB、9MB、11.6MB）明显小于最终大小（369KB、22.5MB、26.5MB）。
  正式版要等 OBS 的录制状态事件报告"已停止"，或者等文件大小稳定，再处理文件。
- 录制目录用 OBS 自己的设置（这台机器是 `C:\Users\timothy\Videos`），返回的是完整文件路径。
- 默认输出分辨率是 OBS 按屏幕比例算出来的 1724×1080、60 帧。正式版要设成 1920×1080 这类标准尺寸，录完再恢复用户原来的设置。
- **出错时必须停止录制**：第一次跑的时候，截图出错导致程序提前退出，OBS 一直在录，留下了一个 59 秒的文件。现在的测试程序已经改成出错时先停录制，启动时如果发现上次没停的录制也会先停掉。

## 抓 Dota 窗口

- 窗口标题 `Dota 2`，类名 `SDL_app`，OBS 里写作 `Dota 2:SDL_app:dota2.exe`。
- `Process.MainWindowHandle` 拿到的可能是输入法窗口（类名 `VALVEIME001`），不是游戏画面。要按类名 `SDL_app` 找窗口，发按键（技术验证 2）时也一样。

| | 游戏捕获 `game_capture` | 窗口捕获 `window_capture`（WGC） |
|---|---|---|
| 原理 | OBS 往游戏进程注入图形钩子 | Windows Graphics Capture，**不注入** |
| 本次结果 | 成功。第一次截图失败，因为钩子还没挂上；等待从 2 秒加到 4 秒后正常 | 成功 |
| 独占全屏 | 支持 | 不支持，游戏要用窗口化或无边框 |

**决定（2026-10-19）：只用游戏捕获**，和 CS2 参考项目一致。同时坚持两条：

1. **只在带 `-insecure` 的游戏会话里录。** 这时 VAC 不加载，注入的是 OBS 自己签名的模块。
2. **录完删掉专用场景和捕获源。** 不在 OBS 里留下一个对准 dota2.exe 的游戏捕获源，免得用户之后正常打天梯时 OBS 也往游戏里注入。

"不注入游戏进程"这条原则指的是**我们自己的程序**：我们只用 netcon、GSI 和窗口消息，不往游戏里放东西。注入画面钩子的是 OBS，这是业界通用做法。

做这个决定时查到的资料，都不是 Valve 的官方说明：

- OBS 论坛上，长期有人开着游戏捕获直播 VAC 保护的游戏，没听说因此被封，但也没人能打包票（[VAC safe?](https://obsproject.com/forum/threads/vac-safe.4494)）。
- CS 系列有"可信模式"，会阻止游戏捕获，需要加 `-allow_third_party_software`（[Dot Esports](https://dotesports.com/counter-strike/news/how-to-allow-third-party-software-in-cs2)）。Dota 2 没有这一层，实测不加参数就能挂上。
- [OBS 论坛：Dota 2 的窗口捕获](https://obsproject.com/forum/threads/window-capture-for-dota2.169669/)（2023）里，回帖的人也建议 Dota 用游戏捕获，理由是性能更好。

### 声音

游戏声音直接从游戏捕获源录（`capture_audio: true`），只录游戏，不录系统里的其他声音。

用 `audio_meter` 示例测量（OBS 的 `InputVolumeMeters` 事件，每秒取峰值）。测试时游戏窗口在后台，前台是 OBS；同一段录像（tick 11400 起）测了两次：

| `snd_mute_losefocus` | 峰值 | 结论 |
|---|---|---|
| `true`（默认，也是这台机器的原值） | 每秒都是 0（-188 dB） | **完全静音** |
| `false` | -17.8 到 -27.6 dB | 正常的游戏声音 |

- 关掉后录了 10 秒，视频里有双声道、48kHz、约 192kbps 的音轨。
- **录制前必须把 `snd_mute_losefocus` 设成 0，录完恢复原值。** 它带 `archive` 标记，改了会写进用户的配置文件。
  这次测完已经改回 `true`；用户配置文件里没有这一项，说明保持着默认值。
- 游戏刚启动时会抢走前台焦点。不过只要这个设置关掉了，焦点在不在游戏上都不影响录音。

```bash
cargo run --release -p obs --example audio_meter -- --window "Dota 2:SDL_app:dota2.exe" [--secs 10] [--record]
```

## 和回放控制配合时发现的

- **重新加载录像后，镜头模式会回到"自动视角"**。每次加载录像后都要重新设置镜头模式，按键切换玩家后也要用 GSI 的 `selected_unit` 和截图再确认一次。

## 测试留下的文件

`C:\Users\timothy\Videos` 里有 5 个测试录像（`2026-10-19 17-20-59.mp4` 到 `17-25-09.mp4`，以及测声音的 `19-08-25.mp4`），没有删，需要的话手动删掉。
