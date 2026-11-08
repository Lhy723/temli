# Agent 适配器（插件）

## 1. 目标

调度器、证据门、UI 不感知具体 Agent。新增 Claude Code、Codex 等后端 = 实现一个适配器并注册，不改核心状态机。

## 2. 契约

Rust 侧 trait（Tauri 进程内）：

```rust
trait AgentAdapter {
    fn manifest(&self) -> AgentManifest;
    fn detect(&self) -> DetectResult;
    fn start(&self, run: RunSpec, sink: EventSink) -> Result<SessionHandle>;
    fn respond(&self, handle: &SessionHandle, request_id: &str, decision: Decision) -> Result<()>;
    fn cancel(&self, handle: &SessionHandle) -> Result<()>;
}
```

```rust
struct AgentManifest {
    id: String,            // "omp" | "fake" | "claude-code" ...
    name: String,
    version: String,       // semver
    capabilities: Capabilities,
}

struct Capabilities {
    streaming: bool,
    approvals: bool,       // 能上报审批请求并等待回答
    cancel: bool,
    usage: bool,           // 提供 token / 费用
    cost_cap: bool,        // 支持运行时设置费用上限
    resume: bool,          // 支持会话续跑
}
```

`RunSpec`：`run_id`、`worktree`、`prompt`、`model`（可选）、`max_minutes`、`max_cost`（可选）、`env_allowlist`。

`detect()` 返回 `{ available, version?, auth: unknown|ok|missing, detail? }`，只用于 UI 提示，不阻塞任务失败。

## 3. 归一化事件

适配器把原生事件映射为内部事件；UI 与状态机只消费内部事件。

| 内部事件 | 关键字段 | 去向 |
|---|---|---|
| `status` | `running` / `waiting_approval` / `done` / `failed` / `cancelled` | runs.status、UI |
| `text.delta` / `text.done` | message id、文本 | UI 流式渲染 |
| `tool.start` / `tool.end` | 工具名、参数摘要、成功与否 | UI、审查摘要 |
| `approval.request` | request id、标题、详情、风险、选项 | decisions |
| `usage` | tokens、cost | runs / UI |
| `artifact.diff` | base、head、文件清单 | evidence（artifact） |
| `error` | message、retryable | runs.error、重试判定 |

原始事件流必须完整落盘（`evidence.output_path`，JSONL）。归一化只用于展示与状态迁移；证据判定以原始输出和退出码为准。

## 4. 审批

- 适配器的审批请求写入 `decisions`，UI 回答后回传 `respond()`
- **fail closed**：超时、进程退出、无回答一律视为拒绝
- 高风险判定由审查阶段独立计算，不采信适配器给出的风险标签
- 能力缺失降级：`approvals=false` 的适配器一律要求人工审批任务启动

## 5. 注册与扩展

- **v1**：编译期注册表，内置 `omp` 与 `fake`（测试用，不调用模型）。`fake` 用于在 CI 中验证状态机与证据门
- **v1.1**：外置适配器目录 `~/Library/Application Support/temli/agents/<id>/`，`manifest.json` + 可执行文件，宿主以 stdio JSONL 协议通信（与 OMP RPC 同构）
- 安装外置适配器需显式确认；只传白名单环境变量；不向非 OMP 适配器传模型凭证，凭证由适配器自身登录解决
- 适配器崩溃不得影响其他任务；宿主动手标记 `failed` 并按重试策略处理

## 6. OMP 适配器（v1 唯一实现）

- 启动：`omp --mode rpc`，cwd 为任务 worktree；**不加 `--no-ui`**，否则工具审批直接 fail closed，拿不到审批帧
- 握手：读 `ready` 帧，发送 `negotiate_protocol` 协商 v2（大帧走 `rpc_chunk` 重组）
- 提示：`prompt` 的响应只表示“已接收”，不是完成；完成以 `agent_end` / `prompt_result` 判定（注意 yield 与 settled 的区别）
- 审批：`extension_ui_request` → decisions；回答用 `extension_ui_response`（`value` / `confirmed` / `cancelled` / `answers`）
- 取消：`abort`
- 费用：用 `session.maxCost` 设上限；达到上限时 OMP 返回 `error.code = "cost_cap"`
- 结束采集：`git status --porcelain`、`git diff base..head`、未提交文件清单、退出码，作为 artifact 证据
- 持久化：`get_state` 或 ready 后的会话信息中取 `sessionId` / `sessionFile`，写入 `runs.session_id`

## 7. 契约测试

- 用 `fake` 适配器跑通：启动 → 流式事件 → 审批 → 完成 → 证据落库
- 断言：适配器异常退出时 run 进入 `failed`；审批超时为拒绝；缺失 `approvals` 能力时任务必须人工放行
