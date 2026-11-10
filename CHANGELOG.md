# Changelog

本文件记录 temli 的显著变更。格式参考 [Keep a Changelog](https://keepachangelog.com/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [0.9.0] - 2026-11-10

### Added

- **前端 E2E**：Playwright + Vite + mock IPC，覆盖看板、计划验收意见、设置开发者模式、Vault；CI 自动运行。
- **契约防漂移**：`npm run check:contracts` 校验 Rust 命令定义、`generate_handler!` 注册与前端 `invoke` 名称一致，CI 强制执行。
- `docs/refactor-plan.md` 补充契约/E2E 结果与 tauri-specta 评估。

### Changed

- 清理 M6 遗留的 notes 模式死代码：删除 6 个失效 API 与 Workspace 的笔记分支（852 → 304 行，改为纯 docs 模式）。

## [0.8.0] - 2026-11-10

### Changed（可维护性，无行为变化）

- 后端 `store.rs`（5,099 行）拆分为 7 个领域模块（tasks/scheduling/runs/plans/settings/attachments/test_support），`crate::store::*` 公开路径不变。
- `lib.rs` 的 72 个 Tauri 命令拆到 8 个 `commands/*` 领域模块；`AppState` 仅提升可见性，命令名与参数不变。
- `TasksPanel.tsx`（4,015 行）拆为主文件 760 行 + 15 个子组件/hooks；`Vaults.tsx`（2,542 行）拆为主文件 559 行 + 14 个子组件/hooks。
- 新增 `docs/refactor-plan.md` 记录拆分边界、验收与后续（契约生成、类型化错误、前端 E2E）。

## [0.7.0] - 2026-11-10

### Added

- **任务证据附件**：任务详情可上传截图/文件（≤20 MB），图片显示缩略图并可放大，支持删除；文件保存在数据目录 `attachments/<task>/`，迁移 v13 `task_attachments`。用于补充「证据，而非声称」的人工截图材料。
- **阶段验收意见/小结**：待验收阶段可填写可选意见随验收通过落库，已通过阶段展示验收意见与时间（`plan_stages.summary/summary_at`）。

### Changed

- `accept_stage` 增加可选 `note` 参数；空/空白意见不写入。

## [0.6.0] - 2026-11-10

### Added

- **开发者模式**（设置 → 开发者，默认关闭）：开关与端口配置；开启后显示本地 API 地址与 Bearer token。
- **本地调试 HTTP API**（仅 `127.0.0.1`）：`/health`、`/summary`、`/tasks`、`/plans`、任务启动/取消、计划启动/阶段验收、`/logs` 日志尾部；除 `/health` 外均需 Bearer token。
- **设置页调试工具**：日志尾部查看（200 行）、IPC 控制台（任意已注册命令 + JSON 参数，显示返回/错误）。
- `docs/dev-mode.md`：接口说明与 curl 示例。

### Changed

- 新增依赖 `tiny_http`（轻量同步 HTTP 服务）。

## [0.5.1] - 2026-11-10

### Fixed

- **启动计划会真正开始执行**：启动时把首阶段 `draft/ready` 任务自动入队；阶段验收通过后，下一阶段的 `draft/ready` 任务同样自动入队。已排队/执行/合并的任务跳过，失败/中断/待人工不自动重启；单个入队失败只记录日志、不影响计划状态。
- 计划界面注明启动与验收后的自动入队行为。

## [0.5.0] - 2026-11-09

### Added

- **计划（Plan）与阶段门**：把任务编排为有序阶段（locked / active / review / passed）；当前阶段任务全部合并后进入待验收，验收通过才解锁下一阶段，最后阶段通过后计划完成；支持草稿计划启动。
- **下一步（nextActor）**：任务卡/表格显示「待你 / Agent」，待人工决策与失败进入「待你」。
- 迁移 v12：`plans` / `plan_stages` / `plan_tasks`；任务最多属于一个阶段、同项目校验。
- `docs/kitestring-borrowing.md`：与 Kitestring 的概念对照与采纳/非目标记录。

### Changed

- 侧栏新增「计划」视图（位于「我的任务」之后）；阶段状态与任务状态分列展示。

## [0.4.1] - 2026-11-09

### Added

- 附件支持**拖入**编辑器（与选择/粘贴同一保存与插入路径，多文件按序处理）。
- `[[别名]]`：frontmatter `aliases` / `alias`（flow、逗号、块列表）参与链接解析与反链。

### Fixed

- `docs/vault-alignment.md` 与实现对齐：状态更新为实际完成项；`![[嵌入]]`、Callouts、数学、Mermaid 如实标记为后续，Canvas/插件/同步/移动端为非目标；脚注确认为 GFM 已支持。

## [0.4.0] - 2026-11-09

### Added

- **标签**：frontmatter `tags`（列表/逗号分隔）与行内 `#tag`（CJK、`_ - /`）；标签面板按引用次数排序，点击筛选笔记；迁移 v11 `vault_note_tags`。
- **附件**：编辑器选择文件或粘贴图片，存入 `attachments/`（≤25 MB，文件名清洗）；插入 `![](…)` 并在预览中解析为图片。
- **模板**：`templates/*.md` 出现在新建对话框，支持 `{{date}}` / `{{time}}` / `{{title}}` 替换。
- **日记**：一键创建/打开 `Daily/YYYY-MM-DD.md`，可套 `templates/Daily.md`。
- **属性面板**：编辑 frontmatter 键值（保留正文与其他键，空值删除）。
- **大纲**：解析标题树，点击定位到编辑器对应行。
- **快速切换**：`⌘O` 按标题/路径模糊跳转。
- **链接补全**：输入 `[[` 时提示笔记标题，↑↓/Enter 插入。
- **重命名**：重命名笔记文件并同步改写全库 `[[旧标题]]`（含别名/锚点）。

### Changed

- Vault 右栏改为「反链 / 大纲 / 属性」三个页签（宽 240）；`docs/vault-alignment.md` 记录与 Obsidian 的逐项对照。

## [0.3.0] - 2026-11-09

### Added

- **设置页**：项目（并发上限、自动合并、要求 CI checks、自动维护文档）、运行（默认 Agent、worktree 保留天数 1–90）、关于（版本、数据目录、许可证）；侧栏新增「设置」入口。
- **全局设置存储**：SQLite 迁移 v10 `app_settings`，键值持久化；默认 Agent 优先级 `TEMLI_AGENT` 环境变量 > 设置 > `omp`。
- **单实例守卫**：`tauri-plugin-single-instance`，重复启动只保留一个进程并激活已有窗口。
- **持久日志**：数据目录 `logs/temli.log`（1 MB 轮换），记录启动、退出、设置变更与 panic。
- CI：新增 `cargo clippy --all-targets -D warnings` 门禁；新增 tag 触发的自动构建与 Release 上传 workflow。

### Changed

- `cleanup_worktrees` 的孤儿回收窗口从硬编码 7 天改为读取设置。
- 清理 Rust clippy 29 条告警（等价重写，无行为变化）。

## [0.2.0] - 2026-11-09

### Added

- 审查页：按项目列出证据与独立审查报告（verdict、风险标记、证据通过率、可展开摘要与测试改动）。
- Agent 页：适配器能力与角色总览（developer/reviewer、审批/取消/成本能力），以及任务级适配器与模型选择说明。

### Changed

- 界面统一：字号/间距/控件高度/圆角规范，顶部 tab 与侧栏选中态一致，看板列宽与卡片密度对齐；过渡动画限定 120–160ms 并尊重 `prefers-reduced-motion`。

## [0.1.1] - 2026-11-09

### Added

- 应用图标：深炭底 + 琥珀色对勾（证据门/已验证语义），替换 Tauri 默认图标；菜单栏改用模板图标（随浅色/深色外观自动反色）。

### Fixed

- **签名修复**：Tauri 默认不对 `.app` 做 ad-hoc 签名，产出的包只有二进制 linker 签名、缺少 `_CodeSignature/CodeResources`，导致 macOS 自动安装（DMG 内双击 App）报「无法安装 / 没有更改任何内容」。现显式配置 `bundle.macOS.signingIdentity = "-"`，`codesign -v` 通过。

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
