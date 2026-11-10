# 借鉴 Kitestring 的项目管理设计

参考：<https://github.com/NekroAI/kitestring>（AGPL-3.0）。只借鉴产品概念，不复制代码。

## 概念对照

| Kitestring | temli 现状 | 采纳决定 |
|---|---|---|
| Plan（计划）→ Stage（阶段，locked/active/review/passed）→ 阶段验收后解锁下一阶段 | 只有「想法 → 多 Issue」，无阶段门 | **M20 采纳**：计划/阶段/验收门 |
| Requirement 七态（draft/ready/in_progress/blocked/review/done/shelved） | 任务态 queued/running/needs_human/merged/failed | 部分采纳：看板加 `nextActor`（等你/等 Agent），不重命名现有状态机 |
| `nextActor`（user/agent/none，推导） | 无 | **M20 采纳**：任务卡/表格显示「待你 / Agent」；终态沿用状态标签 |
| `blockedOn` + 指向需求/决策 + 自动解除 | 依赖 DAG 自动就绪；PR/CI 失败转人工 | 依赖已有 DAG，暂不新增 blocked 字段 |
| 证据带截图/录屏/原型、可作废留痕 | 证据=命令输出/diff/审查；无附件 | **M22 部分采纳**：任务证据附件（截图/文件，20 MB）；录屏/原型/作废留痕后续 |
| 阶段小结、退回意见 | 无 | **M22 部分采纳**：验收意见/阶段小结落库并展示；退回后续（temli 任务 merged 终态，无重开机制，退回语义未定） |
| Decision 选项+推荐+图片、答复注入 | 决策收件箱（approve/deny/input） | 已有；图片选项后续 |
| 角色（PM/aide/advisor/reviewer/developer） | developer / reviewer | 后续（角色预设 + 权限边界） |
| Session 车道/多机 Hub/手机/PWA | 单机桌面、任务队列 | 非目标（temli 定位本地单机） |
| 分享访客链接、留言队列 | 无 | 非目标 |
| 计划阶段小结、退回意见 | 无 | M20 先做验收通过；退回/小结后续 |

## M20 范围

1. `plans` / `plan_stages` / `plan_tasks`（迁移 v12）。
2. 计划：草稿 → 启动（第一阶段 active）→ 阶段任务全部 merged 进入 review → 用户验收 → passed 并解锁下一阶段；最后阶段通过则计划完成。
3. 校验：任务必须属于同一项目、一个任务最多属于一个阶段、阶段至少一条任务、至少一个阶段。
4. UI：新增「计划」视图（列表 + 阶段进度 + 新建向导 + 验收按钮）；任务卡显示 `nextActor`。

## 明确不做

- 多机 Hub、手机 App、访客分享、原型预览、Apple Pencil 标注。
- 由看板卡片直接驱动任意 Agent 会话（temli 已有调度器，避免双轨）。
