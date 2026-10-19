# 技术验证 1：获取并解析录像

结论：**可行**。代码在 [`crates/replay`](../../crates/replay)。

## 怎么跑

```bash
# 解析本地录像，打印玩家、英雄、击杀、多杀、连杀（加 --json 输出 JSON）
cargo run --release -p replay --example dump -- "<录像.dem>"

# 按比赛 ID 下载录像并解析
cargo run --release -p replay --example fetch -- <比赛ID> <保存目录>
```

## 结果

- 用本机 `replays` 目录下的 11 场录像，和 OpenDota（即 Valve 官方数据）逐人核对：
  **110 条玩家记录的击杀数、死亡数全部一致**。
- 解析一场 20～40 分钟的录像约 0.3 秒（release 构建）。
- 按比赛 ID 下载的录像，和游戏客户端自己下载的那份 SHA-256 完全相同。

## 关键发现

### 玩家与英雄

录像开头的文件信息（`CDemoFileInfo`）里就有 10 名玩家的序号、昵称、Steam ID、队伍、英雄，
不用解析整场比赛。录像里的事件都用玩家序号 0–9 指代玩家。

### 时间

- 游戏时钟 = 战斗日志时间戳 − 开局时间（`CDOTAGamerulesProxy` 的 `m_pGameRules.m_flGameStartTime`）。
- 每个事件同时记下 tick（约 30 tick/秒），后面控制回放跳转时用得上。

### 击杀归属：以计分板为准

只看战斗日志里的"致死单位"会算错，击杀播报（`CDotaUserMsgChatEvent` 的 `HERO_KILL`）也不可靠：

- 英雄被塔或小兵打死时，游戏有时会把击杀记给附近的敌方英雄，战斗日志看不出来；
- 击杀播报里列出的英雄，不一定真记到了击杀。

最后采用的办法：在发生死亡的同一个 tick，看计分板（`CDOTA_PlayerResource` 的
`m_vecPlayerTeamData.NNNN.m_iKills` / `m_iAssists`）上谁的击杀数、助攻数加了 1。这和 Valve 的统计口径相同。

另外，骷髅王带盾复活、幻象死亡都不算死亡，已经过滤掉。

### 多杀、连杀

战斗日志里有游戏自己判定的多杀（`MULTIKILL`）和连杀（`KILLSTREAK`）事件，直接用就行，不用自己按时间窗口推算。

### 按比赛 ID 下载

- 录像地址是 `http://replay{cluster}.<域名>/570/{比赛ID}_{salt}.dem.bz2`，国服域名是 `dota2.com.cn`。
  其中的 salt 只有游戏协调服务器给得出。
- 目前借助 OpenDota 拿 salt：比赛详情里有 `replay_url` 就直接用；没有就调 `POST /api/request/{比赛ID}`，
  通常几十秒内就会给出地址。
- **文件名虽然是 `.bz2`，实际内容是 zstd 压缩**，所以要按文件头识别压缩格式（bzip2 也保留了兼容）。
- 录像在 Valve 服务器上只保留一段时间，过期后就下载不到了。

## 录像来源的安排

玩家、英雄、击杀等数据全部来自本地解析 `.dem`。录像文件本身来自 Valve 的录像服务器，OpenDota 只负责提供下载地址里的 salt。

1. **主路径**：读取游戏客户端已下载的录像（用户在游戏里点"下载"，我们读 `replays` 目录），不联网，最稳定。
2. **便捷路径**：输入比赛 ID，通过 OpenDota 拿到地址后下载。失败时提示用户去游戏里手动下载。
3. **待验证**：能否用控制台命令让客户端自己下载录像。如果可以，就不需要 OpenDota。放在"控制回放"验证里一起做。

## 遗留问题

- **助攻**：少数英雄会差 1–3 个，11 场里出现了 4 次，其中 3 次是 Pudge。推测这些助攻不是在死亡那个 tick 记上的。
  高光识别用不到助攻，暂时不处理。
- **自动找本地录像目录**：现在要手动给出路径。以后要从注册表和 `libraryfolders.vdf` 定位游戏安装目录，可以参考 dota2-game-helper2。
- **不依赖 OpenDota 获取 salt**：要直接问游戏协调服务器，需要登录 Steam，暂不考虑。
