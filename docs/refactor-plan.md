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

## 契约与 E2E（2026-11-10，0.9.0）

- **契约防漂移**：`scripts/check-contracts.mjs` 校验「Rust 命令定义 = generate_handler 注册 = 前端 invoke 名称」，CI 强制执行；首次运行即发现并清除了 6 个 M6 遗留的 notes 命令调用（Workspace 852→304 行，改为纯 docs 模式）。
- **前端 E2E（mock IPC，仅 UI 回归）**：Playwright + vite + mock IPC，4 条用例（看板/计划验收/设置开发者模式/Vault），CI 运行 `npx playwright install chromium` + `npm run test:e2e`。不经过 Tauri/HTTP/DB，不能替代集成测试。
- **tauri-specta 评估（未实现）**：需要为 72 命令与全部 DTO 加 `Type`/`#[specta::specta]` 并接管 handler 注册，改动面与 Tauri 版本耦合较大；当前以契约 lint 覆盖「名称/注册漂移」，类型级 codegen 留作独立里程碑。
- **类型化错误（未实现）**：`Result<T, String>` 横跨 75 命令，替换会同时改动命令、前端 `errorText` 与既有错误匹配；未在本轮展开。

## 明确未覆盖

- 真实后端/集成 E2E：Playwright 尚未连开发者 HTTP API 或通过真实 Tauri IPC 驱动；当前 E2E 只验证渲染与前端调用参数。
- `tauri-specta` 类型生成与 `AppError` 类型化错误。


## 验收

- 无行为变化：命令/字段/文案保持，测试全绿。
- 单文件上限：Rust < 1200 行，前端组件 < 800 行。
- 新领域接入只需：迁移 + 领域模块 + 命令模块 + 页面，不再修改上帝文件。
