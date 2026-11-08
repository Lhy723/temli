# temli

AI-native project workspace. 产品与架构文档见 [docs/PRD.md](docs/PRD.md)、[docs/data-model.md](docs/data-model.md)、[docs/ui.md](docs/ui.md)、[docs/agent-plugins.md](docs/agent-plugins.md)。

## 开发

```bash
npm install
npm run tauri dev   # 桌面应用
npm run build       # 前端类型检查 + 构建
cd src-tauri && cargo test
```
