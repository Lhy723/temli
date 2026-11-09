# Changelog

本文件记录 temli 的显著变更。格式参考 [Keep a Changelog](https://keepachangelog.com/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

## [0.1.0] - 2026-11-09

### Added

- 本地优先的 Tauri 2 + React 19 桌面工作区：Linear 风格布局、SQLite + FTS5 存储、项目注册与 `.temli/` 忽略校验。
- 证据门：验收命令的真实输出、退出码与耗时，变更文件与 diff，独立审查报告，高风险变更人工决策；放行前逐条校验验收标准。
- Agent 适配器：OMP（`omp --mode rpc`）、Codex、Claude Code，以及测试用 `fake`；支持任务级 developer/reviewer 适配器与 model。
- 任务队列：串行与 DAG 调度、依赖关系、按项目并发上限、独立 worktree、自动 rebase，以及取消、超时和有限重试。
- 合并后复验与回滚：合并队列串行执行，合并后重跑证据，失败可回滚到合并前状态。
- GitHub 同步：Issue 每 60s 条件轮询（ETag）、ghost 删除标记、带版本号的乐观写回；PR 创建、checks 展示与受证据门控制的自动合并，`require_checks` 可要求非空检查。
- 文档与知识空间：仓库 `docs/**/*.md` 只读索引与 FTS 搜索；任务上下文注入项目文档。
- 独立笔记库（Vault）：多库注册、文件夹树、Markdown 文件权威、`[[双链]]`/别名、反链、图谱与 CJK 搜索。
- 多项目界面与 ⌘K 全局搜索：跨项目检索任务、文档与笔记并深链；项目选择持久化。
- 想法到计划：规划 Agent 将一段想法拆成带验收标准与依赖关系的多个 Issue，预览后批量落库。
- 合并后自动文档：可按项目开关，合并成功后创建受限文档任务，并走同一证据门。
- 菜单栏常驻与通知：托盘显示/退出、关窗隐藏并继续执行，任务终态与待决策通知。
- SQLite 迁移 v1–v9（`schema_migrations`），覆盖证据、同步、`require_checks`、搜索、笔记库、任务适配器、model 与自动文档字段。
- GitHub Issue 写回闭环：标题、正文、指派人、关闭/重开经乐观并发（`expected_updated_at`）写回远端；远端关闭可同步回本地 `github_state`。
- 任务看板与详情：已发布 issue 支持编辑标题/正文、设置指派人、关闭/重开；冲突提示「远端已变更，请先同步」。
- 通知点击深链：点击任务通知恢复主窗口并打开对应任务；Dock 图标在无可见窗口时恢复窗口。

### Changed

- 自动合并默认关闭；开启后仍须证据门全绿。
- 笔记库与项目解耦：笔记独立存储与检索，任务上下文只接受项目文档。
- 搜索索引改用 trigram FTS；少于 3 字符及混合短词查询走 LIKE 回退。
- 文档任务识别改用 `tasks.auto_docs` 结构化字段（不再依赖标题前缀）；父任务合并后立即刷新列表，消除一个轮询周期的延迟。
- 看板窄窗口改为横向滚动，任务卡标题按行截断。

### Fixed

- GitHub checks 瞬时读取失败自动重试，持续失败在超时后转人工决策。
- 全局搜索隐藏未转任务且已关闭或 ghost 的同步 Issue。
- 笔记库阻止符号链接逃逸，笔记写入改为原子替换；修复大小写重命名与中文短词搜索。
- OMP 审批帧误判（如 `setWidget`）、合并前分支 SHA 校验、取消竞态、进程组清理与命令超时。
- 启动清理按 7 天时间窗回收「登记仍在但 run 行已消失」的孤儿 worktree；失败/需决策任务的 run 行存在时永不回收，保留调试现场。
- 清理适配器层死代码（`RunSpec.run_id`/`max_cost_usd`、`detect` 桩等测试专用符号改为 `#[cfg(test)]` 门控），`cargo build` 恢复 0 warning。
