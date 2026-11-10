# TASKS

更新时间：2026-11-09　当前状态：M0–M22 完成，0.7.0 已发布（tag `v0.7.0`，tag 推送自动构建 Release）

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
- [x] M18 基础设施：设置存储（迁移 v10 `app_settings`）+ 设置页；单实例守卫；持久日志（轮换/panic hook）；默认 Agent 优先级 env > 设置 > omp；孤儿 worktree 窗口可配；CI 增加 clippy `-D warnings` 与 tag 自动发布 workflow（PR #67）
- [x] M19 Vault 对齐 Obsidian：标签（migration v11）、附件（选择/粘贴/拖入）、模板变量、日记、属性面板、大纲、⌘O 快速切换、`[[` 补全、别名解析、重命名同步改链接；对照矩阵见 `docs/vault-alignment.md`（PR #72；补丁 #75/#76）
- [x] M20 借鉴 Kitestring：计划（Plan）与阶段门（locked/active/review/passed，验收解锁下一阶段）；任务 `nextActor`（待你/Agent）；迁移 v12；对照与非目标见 `docs/kitestring-borrowing.md`（PR #78；自动入队补丁 #81）
- [x] M21 开发者模式 + 本地调试 API：设置页开关/端口/token；仅 127.0.0.1 的 HTTP（health/summary/tasks/plans 启停/日志），Bearer 认证；日志尾部与 IPC 控制台；`docs/dev-mode.md`（PR #84）
- [x] M22 Kitestring 证据与阶段意见：任务证据附件（截图/文件 ≤20 MB，迁移 v13 `task_attachments`）；阶段验收意见/小结（`plan_stages.summary/summary_at`）；对照文档更新（PR #87）

## 关键验证

- `cargo test --lib`：311 passed，1 ignored（需真实 OMP 的 live 规划测试）
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
- M18 设置写入链路（隔离浏览器 + mock IPC，真实点击）：retention 7→8 后保存按钮由 disabled 变 enabled，点击调用 `set_worktree_retention_days {days:8}` 并显示「已保存」；自动合并开关调用 `set_auto_merge {input:{project_id,enabled}}`；默认 Agent 选择调用 `set_default_agent {agent:"fake"}`；注入失败后 `role="alert"` 显示「模拟失败：写入被拒绝」
- 0.3.0 发布：tag `v0.3.0` 触发 release workflow 自动构建（DMG 3,758,232 字节 + `temli_aarch64.app.tar.gz`），发布说明已补；下载产物 `codesign -v` 通过并安装验证
- M19 Vault 对齐（浏览器 mock IPC）：标签筛选 `notes_by_tag`、属性保存 `set_note_properties`（引号值不二次转义、保存前先 `save_vault_note` 落盘草稿）、大纲跳过 frontmatter、⌘O 切换、`[[` 补全插入 `[[Inbox]]`、模板 `{{date}}`→当日日期
- 0.4.0 发布：tag 触发 workflow 构建（DMG 3,807,789 字节），`codesign -v` 通过并安装
- 0.4.1 补丁：浏览器 DOM drop 实测（`save_vault_attachment` 调用、插入 `![pic.png](attachments/…)`、预览读取）；`[[别名]]` 由 frontmatter `aliases/alias` 解析（flow/逗号/块列表，单测覆盖）；矩阵状态与实现对齐；Release DMG 3,811,144 字节，`codesign -v` 通过并安装
- M20 计划阶段门（浏览器 mock IPC）：计划时间线渲染、`accept_stage` 调用、新建计划 `create_plan {stages:[{title,task_ids}]}`；单测覆盖草稿不可验收、启动→待验收→通过解锁→完成；0.5.0 DMG 3,826,337 字节；0.5.1 修复启动/验收自动入队（单测：首阶段 draft→queued、次阶段验收后 draft→queued），DMG 3,827,159 字节，`codesign -v` 通过并安装
- M21 开发者 API（真实应用 + curl）：`/health` → `{ok:true,version:"0.5.1"}`；无 token 的 `/summary` 返回 401；带 Bearer 的 `/summary` 返回任务计数与默认 Agent；`/tasks?project_id=…` 返回任务；`/logs?lines=3` 返回日志尾部；关闭应用后端口 3789 无监听；0.6.0 DMG 3,914,613 字节，`codesign -v` 通过并安装
- M22 证据附件（浏览器 mock IPC）：上传 `add_task_attachment`（base64）、列表缩略图 + `read_task_attachment`、删除 `delete_task_attachment`；阶段验收意见随 `accept_stage` 传入并回显；0.7.0 DMG 3,930,729 字节，`codesign -v` 通过并安装

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
cd src-tauri && cargo test --lib          # 311 passed, 1 ignored
npm run build                             # 前端类型检查 + 构建
npm run tauri build                       # release .app + .dmg
cargo test --lib live_plan_generation_with_omp -- --ignored --nocapture
```
