# TASKS

当前里程碑：M6 独立笔记库 + 任务级 Agent 分配（已合入 develop）

## 已完成

- [x] M0 应用骨架：Tauri 2 + React 19 + Tailwind v4 + HeroUI v3；Linear 式布局；SQLite 迁移；项目注册与 `.temli/` 校验（#4）
- [x] 产品规格：PRD、数据模型、UI 规范、Agent 适配器契约（`docs/`）
- [x] 验收目标仓库：`Lhy723/temli-demo`（6 个测试）

## 进行中：M1 证据门

- [x] Agent 适配器契约与注册表（`src-tauri/src/agent/mod.rs`）
- [x] `fake` 适配器与契约测试
- [x] OMP RPC 适配器（协议 v2、事件归一化、审批、取消）
- [x] 存储层与任务状态机（items/tasks/runs/evidence/reviews/decisions/events）
- [x] 证据执行器（命令发现、执行、diff、高风险与测试改动识别）
- [x] 编排器：worktree、单任务执行、证据门、审查、决策
- [x] 规划/审查 prompt 与 JSON 解析
- [x] 前端：任务列表/详情、决策收件箱
- [ ] 独立代码审查与修复

## 待办：M2–M4

- [ ] M2 队列与 worktree 生命周期、合并队列（#6）
- [ ] M3 DAG 调度与受控并发（#7）
- [ ] M4 GitHub 同步与自动合并（#8）

## 已知限制

- 自动合并默认关闭；证据门全绿前不得开启
- OMP 适配器 `cost_cap` 能力暂为 false（协议未暴露运行时费用上限命令）

## 审查修复（M1）

- [x] 测试/CI 改动必须由审查逐项确认（test_diff 覆盖，否则 needs_human）
- [x] reviewer 只接受 Completed 终态
- [x] 同一任务并发 start 的原子守卫
- [x] failed / interrupted 的重新入队路径
- [x] 合并前校验分支 SHA 与受审 commit 一致
- [x] rpc_chunk 重组、response.success=false 失败处理、session_settled 终态
- [x] 验收标准 expect_exit / expect_match 真实求值
- [x] 命令超时杀进程组
- [x] 决策收件箱轮询与刷新
- [x] 会话关闭与 sessionId 落库

## 冒烟结果

- 2026-11-08：OMP 真实任务（temli-demo 创建 hello.txt）→ npm test 证据 exit 0 → 审查 pass → 本地 fast-forward 合并成功；发现并修复 `setWidget` 被误判为审批的缺陷。

## 验证

- `cargo test --lib`：102 passed
- `npm run build`：通过
- 原生冒烟 2 次（OMP 真实执行，temli-demo）：均以 evidence exit 0 + 审查 pass 自动合并；第二次确认 `setWidget` 不再产生审批决策
- 审计完整性：artifact（diff.patch + files.json）与 review（review.json）均已作为 evidence 落库
- 规划路径测试：grounded 成功、解析失败回退、命令未发现回退、空命令跳过（5 个测试）

## 进行中：M2 队列与 worktree

- [x] 单 worker 串行队列与入队/取消 IPC
- [x] 合并前 rebase、合并锁、合并后重验与回滚
- [x] 超时决策、失败自动重试（3 次上限）
- [x] worktree 生命周期与启动清理
- [x] 前端排队状态与取消按钮
- [x] 调度串行/重试/取消、回滚、超时集成测试
- [x] 独立审查（2 P1 / 2 P2）
- [x] 审查修复：回滚外部提交保护、排队取消竞态、启动期取消意图、重试上限死路
- [x] 合入（PR #16）

## M2 冒烟

- 队列串行：两个 queued 任务，一个在运行中被强制重启 → `interrupted`，另一个自动执行并以 evidence + 审查通过合并
- 回滚/超时/重试/取消/串行队列用例由 `cargo test` 覆盖（121 tests）

## 进行中：M3 DAG 调度

- [x] 依赖边校验（同项目/自环/环）、就绪递归查询、并发配置 IPC
- [x] 并发 worker 池（按项目 limit、默认 1）
- [x] rebase 冲突阈值策略（≤3 文件且非高风险自动重排）
- [x] 前端依赖展示与并发设置
- [x] 独立审查（1 P1 / 2 P2 / 2 P3）
- [x] 审查修复：spawn 失败热循环、claimed 任务饿死、取消后重排、认领后依赖、空冲突误重试
- [x] 合入（PR #17）

## M3 冒烟

- 并发 2：gamma / delta 同时 running；delta 先合并，gamma 在合并时 rebase 到新 tip 后成功合并（修复前会 merge_failed）
- 依赖/并发/串行/环检测由 `cargo test` 覆盖（195 tests）

## 进行中：M4 GitHub 同步与自动合并

- [x] 迁移 v3：issue body/state/assignee/ghost、auto_merge、PR 字段
- [x] GhOps（gh token + curl）、60s ETag 轮询、乐观写回、ghost
- [x] PR 创建/checks 轮询/条件合并/本地拉取；auto_merge 默认关闭
- [x] 前端同步/发布/PR 链接/自动合并开关
- [x] 实时同步冒烟：远端新建 issue 1 分钟内入库；改标题后 1 个周期内本地更新
- [x] 独立审查（1 P1 / 7 P2 / 2 P3）
- [x] 审查修复：分页 ghost、合并 SHA 锁定、轮询取消、合并锁、curl 超时/令牌、发布幂等、pr_state 顺序、前端跨项目刷新
- [x] `require_checks` 语义：默认无 CI 视为通过，开启后空 checks 阻止自动合并（迁移 v4）
- [x] 合入（PR #18）

## 进行中：M5 知识/文档工作空间

- [x] 迁移 v5：search_fts 全文索引
- [x] 笔记 CRUD（`.temli/notes`，frontmatter + 文件权威）
- [x] 文档扫描与索引（仓库 docs/**/*.md，只读）
- [x] FTS5 搜索（笔记/文档，项目隔离 + snippet）
- [x] 笔记转任务（derived_from 关联）
- [x] 任务上下文关联（relates）与 prompt 注入（4k/12k 截断）
- [x] 前端：知识/文档双空间、Markdown 编辑与预览、搜索、上下文选择
- [x] 独立审查（1 P1 / 3 P2 / 多个 P3）
- [x] 修复：中文短词搜索（trigram + LIKE 回退）、缺失仓库不清索引、笔记转任务空验收拒绝、保存/删除竞态、上下文摘要、稳定 doc id、符号链接/大文件保护
- [x] 合入 develop（PR #23，Issue #22）

## 进行中：M6 笔记库解耦 + 任务级 Agent 分配

- [x] 迁移 v7：vaults / vault_notes / vault_links / notes_fts / 任务适配器字段
- [x] Vaults 后端：多库、文件夹树、Markdown 文件权威、索引
- [x] 双链/反链/图谱与 CJK 搜索
- [x] 笔记与项目解耦；任务上下文仅文档
- [x] 任务级 developer/reviewer 适配器解析与注册表校验
- [x] 前端 Vault 空间（树/编辑器/预览/反链/图谱）
- [x] 开发看板（四阶段）+ Agent 分配
- [x] 独立审查（1 blocker / 1 major / 3 minor）
- [x] 修复：上下文仍调用已删除的 list_notes、vault 符号链接逃逸、非原子写入、大小写重命名、混合短词搜索
- [x] 合入 develop（PR #26，Issue #25）
- [x] 看板展示同步 Issue 并支持一键转任务（PR #28，Issue #27）

## 进行中：M7 多项目 + 全局搜索

- [x] 后端 `global_search`（任务/文档/笔记，作用域 + 片段）
- [x] 项目切换器与选择持久化
- [x] ⌘K 命令面板与跨空间深链
- [x] 合入 develop（PR #31，Issue #30）
- 备注：M7 审查子 Agent 超时未产出结论，改为主会话自检（无 blocker 发现）
