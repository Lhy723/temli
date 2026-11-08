# TASKS

当前里程碑：M1 证据门（待合入）

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
