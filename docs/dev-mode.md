# 开发者模式与本地调试 API

默认关闭。开启后应用在本机启动一个只监听 `127.0.0.1` 的 HTTP 服务，便于脚本、调试器和外部工具读取状态、驱动任务。

## 开启

设置 → 开发者 → 打开「开发者模式」→ 设置端口（默认 3789）→ 保存。

- 仅绑定 `127.0.0.1`，不对外网开放。
- 所有接口（除 `/health`）要求 `Authorization: Bearer <token>`；token 在设置页查看/复制。
- 关闭开关会停止服务；端口变更后自动重启。

## 接口

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/health` | 无需认证：`{ok, version}` |
| GET | `/summary` | 项目数、任务状态计数、默认 Agent |
| GET | `/tasks?project_id=…` | 任务摘要列表 |
| POST | `/tasks/{id}/start` | 入队（等价于界面的「开始」） |
| POST | `/tasks/{id}/cancel` | 取消 |
| GET | `/plans?project_id=…` | 计划与阶段 |
| POST | `/plans/{id}/start` | 启动计划（首阶段任务自动入队） |
| POST | `/stages/{id}/accept` | 验收阶段（下一阶段任务自动入队） |
| GET | `/logs?lines=200` | 应用日志尾部（上限 2000 行） |

## 示例

```bash
BASE=http://127.0.0.1:3789
DB="$HOME/Library/Application Support/com.lhy.temli/temli.db"
TOKEN=$(sqlite3 "$DB" "select value from app_settings where key='dev_api_token'")

curl -s $BASE/health
curl -s -H "Authorization: Bearer $TOKEN" "$BASE/summary" | jq
curl -s -H "Authorization: Bearer $TOKEN" "$BASE/tasks?project_id=<id>" | jq '.[0]'
curl -s -X POST -H "Authorization: Bearer $TOKEN" "$BASE/tasks/<task-id>/start"
curl -s -H "Authorization: Bearer $TOKEN" "$BASE/logs?lines=50" | jq -r '.[]'
```

token 以设置页展示为准（存放在 SQLite `app_settings.dev_api_token`）；不要把它提交到仓库或分享给他人。

## 界面内的调试工具

设置 → 开发者还提供：

- **日志尾部**：最近 200 行应用日志，刷新即可看到调度/错误。
- **IPC 控制台**：输入任意已注册命令（如 `list_projects`、`list_tasks`）与 JSON 参数，直接查看返回值与错误——等价于在开发者工具里执行 `invoke`，用于复现前端调用问题。

## 设计边界

- 不暴露任意 shell 执行；接口只能调用与界面同源的命令/调度器能力。
- 不监听 `0.0.0.0`，不做远程访问。
- 关闭开发者模式后端口立即释放。
