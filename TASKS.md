# TASKS

更新时间：2026-11-09　当前状态：M0–M17 完成，0.2.0 已发布（tag `v0.2.0`，GitHub Release 挂 DMG）

## 里程碑

- [x] M0 应用骨架：Tauri 2 + React 19 + Tailwind v4 + HeroUI v3；Linear 式布局；SQLite 迁移；项目注册与 `.temli/` 校验（#4）
- [x] M1 证据门：适配器契约（OMP RPC / fake）、任务状态机、证据执行器、worktree、审查、决策收件箱、规划 prompt
- [x] M2 队列与 worktree：串行队列、取消、超时、失败重试（3 次）、合并锁、合并后复验与回滚（PR #16）
- [x] M3 DAG 调度：依赖校验、就绪递归、按项目并发池、rebase 冲突阈值（PR #17）
- [x] M4 GitHub 同步与自动合并：迁移 v3、ETag 轮询、ghost、乐观写回、PR/checks/条件合并、`require_checks`（迁移 v4，PR #18）
- [x] M5 文档与知识空间：FTS5、笔记 CRUD、`docs/` 只读索引、搜索、任务上下文注入（PR #23，Issue #22）
- [x] M6 笔记库解耦 + 任务级 Agent：迁移 v7 vaults / notes_fts / 适配器字段、双链反链图谱、看板转任务（PR #26/#28，Issue #25/#27）
- [x] M7 多项目 + 全局搜索：`global_search`、项目切换持久化、⌘K 深链（PR #31/#32，Issue #30）
- [x] M8 多 Agent 后端：Codex、Claude Code 适配器真实 CLI 冒烟；roles + 任务级 model（PR #35，Issue #34）
- [x] M9 想法 → 多 Issue 计划：规划、校验、批量落库、依赖与 UI 预览；真实 OMP 冒烟（PR #38，Issue #37）
- [x] M10 真实 PR + CI + 自动合并：temli-demo PR #3 checks 全绿自动合并；checks 瞬时失败重试
- [x] M11 合并后自动文档：迁移 v9、受限文档任务、防递归、统一证据门；真实闭环 PR #4 → 文档任务 → PR #5（PR #43，Issue #42）
- [x] M12 菜单栏常驻 + 通知：关窗隐藏、托盘显示/退出、任务终态与待决策通知（PR #45，Issue #44）
- [x] M13 GitHub 写回闭环 + 文档任务结构化：issue 标题/正文/指派人/关闭重开写回（乐观并发）、远端关闭同步、auto_docs 字段驱动 chip、merged 即时刷新、通知点击深链、孤儿 worktree 时间窗回收、死代码清零（PR #51）
- [x] M14 发布 0.1.0：CI workflow（PR #52）、CHANGELOG 定稿、`release/0.1.0` → `main`（PR #53）、tag `v0.1.0`、GitHub Release 挂 DMG、回合并 develop（PR #55）
- [x] M15 应用图标：深炭底 + 琥珀对勾（证据门/已验证语义），全平台图标集重新生成；菜单栏用模板图标（随浅色/深色反色）（PR #57）
- [x] M16 0.1.1 补丁发布：修复 .app 未被 ad-hoc 签名导致 DMG 自动安装报「无法安装」（`signingIdentity = "-"`，`codesign -v` 通过）；CI 时序测试稳定化（PR #61）；`release/0.1.1` → main（PR #62）→ tag `v0.1.1` + GitHub Release（PR #59/#61 后基于 main 重建）
- [x] M17 真实页面 + 界面统一：审查页（证据/审查报告列表，`list_reviews` 后端）、Agent 页（适配器能力/角色总览）；字号/间距/控件高度/圆角统一、tab 与侧栏选中态一致、看板列宽与卡片密度、120–160ms 动效 + reduced-motion（PR #64）
- [x] M18 基础设施：设置存储（迁移 v10 `app_settings`）+ 设置页；单实例守卫；持久日志（轮换/panic hook）；默认 Agent 优先级 env > 设置 > omp；孤儿 worktree 窗口可配；CI 增加 clippy `-D warnings` 与 tag 自动发布 workflow（PR #TBD）

## 关键验证

- `cargo test --lib`：282 passed，1 ignored（需真实 OMP 的 live 规划测试）
- `npm run build`：通过
- `npm run tauri build`：通过，产物 `temli.app`（7.05 MiB）与 `temli_0.1.0_aarch64.dmg`（3.54 MiB）
- `cargo build`：0 warning（M13 死代码清零）
- M13 写回载荷：以与 `issue_patch_payload` 相同的 JSON（`title` + `assignees` 数组 + `state`）对真实 GitHub issue 执行 PATCH，验证 title/assignees/state 均生效，随后已还原
- M13 独立审查：16 文件 +761/−102，P0–P2 全 0；通知 `extra()` 取回、乐观并发冲突路径、SELECT 列序、孤儿 worktree 命名、轮询快照、前后端契约均源码级核对通过
- M10 真实链路：OMP 任务新增 `greet` + 测试 → PR #3 → CI `npm test` 通过 → 自动合并
- M11 真实链路：父任务合并（PR #4）→ 自动生成 `文档：add farewell` → PR #5 合并 → `docs/guide.md` 更新；仅 1 条文档任务，无递归
- M12 运行时（release bundle + fake 适配器）：托盘左键恢复窗口、右键菜单「显示 temli / 退出 temli」；关闭窗口后进程存活；通知授权后横幅显示「需要决策 / 任务标题 / 状态：等待人工决策」
- 0.1.0 发布：CI 绿后 PR #53 合入 `main`，tag `v0.1.0`，GitHub Release 附件 `temli_0.1.0_aarch64.dmg`（3,703,023 字节）；DMG 已挂载验证包含 `temli.app`；develop 回合并 PR #55
- 0.1.1 发布：`codesign -v` 通过（修复前因缺 `_CodeSignature/CodeResources` 失败）；Release 附件下载后 SHA-256 与本地构建一致（07d0f135…）；安装到 `/Applications` 后签名校验通过并可启动；CI 绿（PR #62）
- M18 基础设施：设置页渲染并读取 `app_settings`（并发/开关/Agent/保留天数/数据目录）；重复启动第二实例自动退出且日志记录「检测到第二个实例，激活主窗口」（仅 1 个进程）；`logs/temli.log` 写入启动/退出/设置变更；`cargo clippy --all-targets -- -D warnings` 0 warning

## 基础设施缺口（M18 起补齐）

- [x] 配置：`app_settings`（迁移 v10）+ 设置页；默认 Agent/保留天数可配置
- [x] 单实例：`tauri-plugin-single-instance`，第二实例激活已有窗口后退出
- [x] 日志：`logs/temli.log`（1 MB 轮换）+ panic hook + 启动/退出/设置变更埋点
- [x] CI：fmt + clippy `-D warnings` 门禁；tag 触发自动构建与 Release 上传
- [ ] 测试：无前端 E2E；托盘/通知无自动化回归
- [ ] 分发：ad-hoc 签名，无公证/自动更新

## 已知遗留

- M13 按验证范围：写回按钮的屏上点击（编辑/关闭/重开）与通知点击深链未做手工冒烟——验证期间用户占用机器、应用窗口位于其他 Space；后端载荷已验证真实 GitHub，通知链路经单测 + 插件源码核对。空闲时补一次点击冒烟
- `tauri dev` 下偶发窗口不在当前 Space（直接运行 `target/debug/temli` 正常）；疑与全屏 Space 切换有关，未定位到代码回归
- 通知在 `tauri dev` 下由插件回退为 Terminal 名义发送；release bundle 使用 `com.lhy.temli`，首次发送会请求系统授权
- 文档工作区只读；GitHub Projects V2 未接入；指派人移除链路未打通（IPC 输入是 `Option<String>`，传不出 `Some(None)`，只有内部 `IssuePatch` 能表达移除；需扩展输入类型或显式 remove 标志 + UI）
- Vault 附件、标签、模板未做；团队抽象、预算/成本上限未做
- 无真实 E2E 前端测试；Developer ID 签名与公证未做（当前 ad-hoc 签名 + `xattr` 提示，安装链路已验证）
- 前端看板窄窗口已改横向滚动（M13），仍需 E2E 回归覆盖

## 下一步

- [ ] 空闲时补 M13 屏上冒烟：issue 编辑/关闭/重开、通知点击深链
- [ ] 文档工作区可编辑；Projects V2；Vault 附件/标签/模板
- [ ] 前端 E2E（Tauri WebDriver 或 mock IPC 全流程）；通知/托盘自动化回归
- [ ] 签名/公证（Developer ID）与 `release/*` 后续版本流程，需要时再启用

## 验证命令

```bash
cd src-tauri && cargo test --lib          # 282 passed, 1 ignored
npm run build                             # 前端类型检查 + 构建
npm run tauri build                       # release .app + .dmg
cargo test --lib live_plan_generation_with_omp -- --ignored --nocapture
```
