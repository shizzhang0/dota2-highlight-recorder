# 技术验证 3：读取回放状态

结论：**可行**。看录像时 GSI 照常推送，能确认跳转后的游戏时钟、是否暂停，以及当前追踪的是哪个玩家。
代码在 [`crates/game/src/gsi.rs`](../../crates/game/src/gsi.rs)。

## 怎么跑

```bash
# 监听 GSI，每包打印一行摘要，完整内容按行写进文件
cargo run --release -p game --example gsi_watch -- [--addr 127.0.0.1:53000] [--out gsi.jsonl] [--secs 秒]
```

这次借用了 dota2-game-helper2 已经装好的 GSI 配置（推送到 `127.0.0.1:53000`），所以没有往游戏目录写任何文件。
验证时 helper2 没在运行，端口是空的。

## 结果

| 要确认的 | GSI 字段 | 实测 |
|---|---|---|
| 游戏时钟 | `map.clock_time`（秒，号角前为负） | `demo_gototick 11800` 后显示 248，即 4:08，和解析结果、画面时钟一致 |
| 是否暂停 | `map.paused` | `demo_pause` 后变为 `true` |
| 比赛阶段 | `map.game_state` | 例如 `DOTA_GAMERULES_STATE_GAME_IN_PROGRESS` |
| 是哪场比赛 | `map.matchid` | `9046032624` |
| 当前追踪的玩家 | `hero.teamN.playerK.selected_unit` | 刚加载时默认是 player0；按 3 后变成 player2（Juggernaut），按 5 后变成 player4（骷髅王） |

- GSI 里的 `playerK` 编号就是我们解析出的玩家序号（0–9）；按键编号是这个序号加 1。
- `player.*` 里还有每名玩家的实时数据（击杀、死亡、连杀、金钱等）。比如 4:08 时 Juggernaut `kills=3`，和解析结果一致。
- 稳定播放时大约每秒一个包。跳转过程中会先出现中间值（比如先报 220，再报 248），要等数值稳定后才算到位。

## 要注意的

- **`clock_time` 只精确到秒**。精确到 tick 的位置以 netcon 的 `Demo Skipping finished at tick N` 为准，GSI 用来做秒级确认。
- **GSI 请求是从 steam.exe 发出的，不是 dota2.exe**：连到 53000 端口的进程是 Steam，游戏应该是借 Steam 的 HTTP 接口发送的。
  所以不能靠"连接来自 dota2 进程"来校验数据来源，还是要用配置文件里的 `auth.token`。
- **配置只在游戏启动时读取**。正式版要在启动游戏前写好我们自己的配置文件（`gamestate_integration_<名字>.cfg`，用自己的端口），录完删掉，以符合"改动在录制后恢复"的原则。
- **镜头模式**（英雄追踪、玩家视角等）在 GSI 里没有对应字段，只能靠截图确认，或者假设设置后一直保持。
