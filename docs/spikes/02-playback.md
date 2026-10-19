# 技术验证 2：控制回放

结论：**可行**。加载、跳转、暂停用 netcon 控制台命令；切换要看的玩家、选镜头模式走游戏界面，用窗口消息发按键和点击，不需要游戏在前台。
代码在 [`crates/game`](../../crates/game)。

## 怎么跑

```bash
# 1. Steam 已登录的前提下，直接运行 dota2.exe（参数只对这一次有效）
#    工作目录：<Dota 2>/game/bin/win64；环境变量：SteamAppId=570、SteamGameId=570
dota2.exe -netconport 2121 -insecure -perfectworld -novid -console

# 2. 通过 netcon 依次执行命令，打印游戏输出
cargo run --release -p game --example console -- [--wait 秒] "<命令>" "<命令>" ...
```

## 启动

- 直接运行 `dota2.exe`，设好 `SteamAppId=570` 就能启动，不经过 Steam 的 `-applaunch`，也不会弹确认框。前提是 Steam 已登录。
- **`-insecure` 是必需的**：不带它时，`-netconport` 不会开端口；带上以后，`status` 里会显示 `insecure`。
  这意味着我们控制的游戏会话一定关闭了 VAC，进不了匹配，和正常游戏天然隔开。
  反过来说，如果用户已经以普通模式开着游戏，就必须先重启游戏。
- 端口监听在 **`0.0.0.0`，也就是所有网卡**，不只是本机。防火墙要拦住入站连接，否则局域网里的其他设备也能给游戏发命令。正式版需要提醒用户。
- 通过 netcon 发 `quit`，游戏会正常退出，约 2 秒。

## 回放控制（netcon 命令）

| 目的 | 命令 | 说明 |
|---|---|---|
| 加载录像 | `playdemo replays/<比赛ID>` | 路径相对于 `game/dota`，从主界面开始加载约十几秒 |
| 跳转 | `demo_gototick <tick>` | 完成后输出 `Demo Skipping finished at tick N`，可作为确认信号 |
| 暂停 / 继续 | `demo_pause` / `demo_resume` | 画面上会显示"回放已暂停" |
| 其他 | `demo_timescale`、`demo_pauseatservertick`、`demo_step_tick`、`demo_quitafterplayback` | 暂未测试 |

- 跳转用的 tick 和 `crates/replay` 解析出来的 tick 是同一套编号：跳到 11800 后画面时钟显示 4:08，和解析结果一致。
  日志里另有一个"game tick"，比录像 tick 多一个固定偏移，等于录像头里的 `server_start_tick`（这场是 1456）。
- `demo_info` 只打印录像文件信息，不显示当前播放到哪个 tick。
- **没有下载录像的控制台命令**，只有一个设置项 `dota_replay_manager_download_chunk_size`。

## 切换要看的玩家、镜头模式（游戏界面）

引擎通用的 `spec_player`、`spec_mode` 在 Dota 2 里**不起作用**：发了以后，镜头模式和选中的玩家都没变。Dota 的观战镜头由它自己的界面控制。

### 切换玩家：热键

- 热键表 `userdata/<账号>/570/remote/cfg/dotakeys_personal.lst` 里有 `SpectatorCameraFocusPlayer1` 到 `SpectatorCameraFocusPlayer10`，默认绑在数字键 1–0 上。
- **编号从 1 开始**：按 3 切到玩家序号 2（Juggernaut）。也就是"按键编号 = 我们解析出的玩家序号 + 1"。
- 用 `PostMessage` 给游戏窗口发 `WM_KEYDOWN` / `WM_KEYUP` 就能触发，**不用切到前台，也不抢用户的键盘**。
- 从"自动视角"按数字键，会切到"英雄追踪"并锁定该玩家。
- 用户可能改过热键，正式版要从热键表里读出实际按键，不能写死。

### 镜头模式：右上角下拉菜单

- 有四种：**自动视角、自由视角、玩家视角、英雄追踪**。没有对应的控制台命令或热键。
  - **英雄追踪**：镜头始终以该英雄为中心。
  - **玩家视角**：还原这名玩家当时自己的镜头，画面里还会出现他的鼠标指针。
- 用 `PostMessage` 发鼠标点击，可以在后台点开菜单并选中模式。点击坐标依赖分辨率和界面布局，这次是 2560×1600。
- **选好模式后，用数字键切换玩家，模式会保持**。比如处于玩家视角时按 5，就变成"玩家视角 · 辣酒煮雞子"。

## 验证用的截图

用 `PrintWindow`（`PW_RENDERFULLCONTENT`）可以直接抓游戏窗口内容，游戏不在前台也行。只用于验证，正式录制用 OBS 的游戏捕获。

## 待确认

- 镜头模式在游戏重启后、换一场录像后会不会保留。如果会保留，只需要首次设置一次。
- 其他分辨率、界面缩放下的点击坐标，以及能否改用更稳的定位方式。
- 独占全屏模式下 `PostMessage` 发按键、点击是否还有效。
- 玩家视角下"战争迷雾"该怎么设：选"双方队伍"，还是只显示该玩家所在一方的视野。

## 和 CS2 参考项目的区别

参考项目直接运行 `cs2.exe` 并带 `-insecure`，启动时用 `+exec` 执行生成的 cfg。运行中发命令靠模拟键盘（SendInput）：绑定 F10 打开控制台，再往里输入命令。

我们的做法：
- 控制台命令走 netcon，能拿到命令的输出。
- 只有观战界面的操作（切玩家、选镜头模式）才需要发按键和点击，而且用 `PostMessage` 直接发给游戏窗口，不需要游戏在前台，也不影响用户的键盘鼠标。
