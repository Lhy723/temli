# 可维护性重构计划（M23）

目标：行为不变，把「大文件 + 手写契约 + 前端零测试」降到可扩展水平。每阶段结束必须通过 `cargo test`、`cargo clippy -D warnings`、`npm run build`。

## 阶段

1. `store.rs`（5.1k 行）拆为 `store/` 领域模块，`crate::store::*` 路径不变。
2. `lib.rs` 的 71 个命令拆到 `commands/{tasks,plans,vaults,settings,dev}.rs`；命令名与参数不变。
3. `TasksPanel.tsx`（4k）/ `Vaults.tsx`（2.5k）拆为子组件与 hooks。
4. 契约生成：评估 `tauri-specta`，从 Rust 生成 TS 类型与命令名，替代手写 `api.ts`。
5. 类型化错误：`thiserror` 定义 AppError，前端按类型提示。
6. 前端 E2E：Playwright 连开发者 API，覆盖看板、计划门、附件、设置。

## 结果（2026-11-10，0.8.0）

| 区域 | 之前 | 之后 |
|---|---|---|
| 后端 store | 单文件 5,099 行 | 7 个领域模块，最大 959 行 |
| 后端命令 | `lib.rs` 2,895 行 / 72 命令 | 8 个 `commands/*` 模块，最大 435 行 |
| 任务界面 | `TasksPanel.tsx` 4,015 行 | 主文件 760 行 + 15 个子组件/hooks |
| Vault 界面 | `Vaults.tsx` 2,542 行 | 主文件 559 行 + 14 个子组件/hooks |

验证：311 tests、clippy `-D warnings` 0、`npm run build`、fmt；浏览器 mock 冒烟（看板/详情/Vault 编辑器与右栏）通过。公开命令名、参数、`crate::store::*` 路径均未变。

## 验收

- 无行为变化：命令/字段/文案保持，测试全绿。
- 单文件上限：Rust < 1200 行，前端组件 < 800 行。
- 新领域接入只需：迁移 + 领域模块 + 命令模块 + 页面，不再修改上帝文件。
