# temli

本地优先的 AI 原生项目工作空间：从想法生成 Issue 与任务，由 Agent 在独立 worktree 中执行，经证据门验证后合并。数据保存在本机 SQLite 与 Markdown 文件中。

## 功能

- **任务与调度**：多项目工作区；串行 / DAG 队列与按项目并发上限；任务可指定 developer / reviewer 适配器与模型。
- **证据门**：验收命令证据、变更 diff、独立审查与高风险人工决策；合并后复验，失败可回滚。
- **Agent 适配器**：OMP（`omp --mode rpc`）、Codex、Claude Code，以及测试用 `fake`。
- **GitHub**：Issue 轮询同步（ETag、乐观写回、ghost）与 PR 创建；自动合并受证据门控制，可按项目要求非空 checks（`require_checks`）。
- **文档与知识**：仓库 `docs/` 索引与 FTS 搜索；独立笔记库（文件夹、`[[双链]]`、反链、图谱）。
- **规划与自动化**：想法拆分为多 Issue 计划；合并后可自动创建文档任务。
- **桌面体验**：菜单栏常驻、关窗继续执行、任务与决策通知、⌘K 全局搜索。

## 安装

从源码构建（推荐，产物无 quarantine，直接可用）：

```bash
git clone https://github.com/Lhy723/temli.git
cd temli && npm install
npm run tauri build
# 产物：src-tauri/target/release/bundle/macos/temli.app 与 dmg/
```

应用为 ad-hoc 签名、未公证。若使用 Release 里下载的 DMG，首次打开被 Gatekeeper 拦截时任选其一：

- 终端执行 `xattr -cr /Applications/temli.app`；
- 或「系统设置 → 隐私与安全性 → 仍要打开」（输管理员密码，仅需一次）。

## 开发

前置：Node.js、Rust 与 Tauri 2 开发环境（主要目标 macOS arm64）。

```bash
npm install
npm run tauri dev   # 桌面应用
npm run build       # 前端类型检查 + 构建
cd src-tauri && cargo test
```

发布构建：

```bash
npm run tauri build
```

## 文档

- [产品需求](docs/PRD.md)
- [数据模型与状态机](docs/data-model.md)
- [UI 规范](docs/ui.md)
- [Agent 适配器契约](docs/agent-plugins.md)
- [变更记录](CHANGELOG.md)

## 许可证

[MIT](LICENSE)
