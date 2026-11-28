# 归档文档

早期的计划、验证记录和决定，2026-11-28 从 `docs/` 挪到这里。内容停在当时，不再更新，和现在的代码可能对不上；
现在的做法以 [../app-interaction.md](../app-interaction.md)（界面和交互）、[../clip-rules.md](../clip-rules.md)（片段规则）和代码注释为准。

| 文档 | 内容 | 最后更新 |
|---|---|---|
| [phase1-plan.md](phase1-plan.md) | 阶段 1 计划：片段规则、录制流程（当时用游戏自带的剪辑工具导出） | 2026-10-23 |
| [phase2-plan.md](phase2-plan.md) | 阶段 2 计划：桌面应用 | 2026-11-11 |
| [prototype-walkthrough.md](prototype-walkthrough.md) | 录制流程的决定（D1–D10：控制台录制、拦截输入……） | 2026-11-10 |
| [recording-review.md](recording-review.md) | 录制实测记录和当时的待定问题 | 2026-11-10 |
| [spikes/](spikes/) | 验证 1–6：解析录像、控制回放、读游戏状态、OBS 录屏、端到端、剪辑工具导出 | 2026-11-04 |

验证 4（OBS 录屏）和验证 6（剪辑工具导出）的代码（`crates/obs`、`crates/recorder`、`game::clip_builder`）2026-11-28 已删，要看翻 git 历史。
