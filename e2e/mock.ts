import { expect, test as base, type Page } from "@playwright/test";

export type MockCall = {
  cmd: string;
  args: Record<string, unknown>;
};

type MockController = {
  /** 所有 IPC 调用；传入命令名时只返回该命令。 */
  calls: (cmd?: string) => Promise<MockCall[]>;
};

declare global {
  interface Window {
    __TAURI_INTERNALS__: {
      invoke: (cmd: string, args?: unknown) => Promise<unknown>;
      transformCallback: (callback: (...args: unknown[]) => void, once: boolean) => number;
      unregisterCallback: (id: number) => void;
      convertFileSrc: (path: string) => string;
    };
    __TAURI_EVENT_PLUGIN_INTERNALS__: {
      registerListener: (event: string, id: number, handler: unknown) => void;
      unregisterListener: (event: string, id: number) => void;
    };
    __temliMock: {
      calls: MockCall[];
      devStatus: {
        enabled: boolean;
        port: number;
        token: string | null;
        base_url: string | null;
      };
    };
  }
}

/**
 * 在页面所有脚本之前安装 Tauri IPC mock。
 *
 * 前端通过 @tauri-apps/api 的 invoke 调用 window.__TAURI_INTERNALS__.invoke，
 * 因此这里只需实现一个与真实命令同名的路由表即可在浏览器里跑通 SPA。
 * 未识别的命令返回 null，避免页面在开发态因缺少命令而崩溃。
 */
export function installMockIpc(): void {
  const now = "2026-11-10T08:00:00Z";
  const project = {
    id: "proj-e2e",
    name: "temli-e2e",
    repo_path: "/tmp/temli-e2e",
    github_owner: "lhy",
    github_repo: "temli-e2e",
    default_branch: "main",
    created_at: now,
  };
  const tasks = [
    {
      id: "task-1",
      title: "接入 mock IPC",
      status: "needs_human",
      attempts: 1,
      criterion_count: 1,
      created_at: now,
      developer_adapter: "claude-code",
      reviewer_adapter: null,
      model: "gpt-5",
      auto_docs: false,
    },
    {
      id: "task-2",
      title: "已有合并任务",
      status: "merged",
      attempts: 2,
      criterion_count: 2,
      created_at: now,
      developer_adapter: null,
      reviewer_adapter: null,
      model: null,
      auto_docs: true,
    },
  ];
  const taskDetail = {
    task: {
      id: "task-1",
      project_id: project.id,
      title: "接入 mock IPC",
      status: "needs_human",
      prompt: "在浏览器中用 mock IPC 跑通看板、计划、设置与笔记库。",
      attempts: 1,
      created_at: now,
      developer_adapter: "claude-code",
      reviewer_adapter: null,
      model: "gpt-5",
    },
    criteria: [
      {
        id: "crit-1",
        cmd: "pnpm test",
        expect_exit: 0,
        expect_match: "passed",
      },
    ],
    runs: [],
    evidence: [
      {
        id: "evidence-1",
        criterion_id: "crit-1",
        kind: "test",
        cmd: "pnpm test",
        exit_code: 0,
        duration_ms: 1200,
        output_path: null,
        collected_at: now,
      },
    ],
    review: null,
    decisions: [],
    events: [{ id: "event-1", kind: "task.created", payload_json: "{}", at: now }],
  };
  const adapters = [
    {
      id: "claude-code",
      name: "Claude Code",
      version: "1.0.0",
      capabilities: {
        streaming: true,
        approvals: true,
        cancel: true,
        usage: true,
        cost_cap: false,
        resume: true,
      },
      roles: ["developer", "reviewer"],
    },
    {
      id: "codex",
      name: "Codex",
      version: "1.0.0",
      capabilities: {
        streaming: true,
        approvals: false,
        cancel: true,
        usage: true,
        cost_cap: true,
        resume: false,
      },
      roles: ["developer"],
    },
  ];
  const attachmentsByTask: Record<string, unknown[]> = {
    "task-1": [
      {
        id: "attachment-1",
        task_id: "task-1",
        file_name: "vitest-report.xml",
        mime: "text/plain",
        size: 2048,
        created_at: now,
      },
    ],
  };
  const plans = [
    {
      id: "plan-1",
      project_id: project.id,
      title: "计划：打通 E2E",
      goal: "先在浏览器里验证核心界面，再补齐回归用例。",
      status: "active",
      stages: [
        {
          id: "stage-1",
          seq: 1,
          title: "基础开发",
          status: "passed",
          tasks: [{ id: "task-1", title: "接入 mock IPC", status: "merged" }],
          passed_at: now,
          summary: null,
          summary_at: null,
        },
        {
          id: "stage-2",
          seq: 2,
          title: "计划验收",
          status: "review",
          tasks: [{ id: "task-2", title: "已有合并任务", status: "merged" }],
          passed_at: null,
          summary: null,
          summary_at: null,
        },
      ],
      created_at: now,
      updated_at: now,
    },
  ];
  const vault = {
    id: "vault-1",
    name: "E2E 笔记库",
    root_path: "/tmp/temli-e2e-vault",
    created_at: now,
  };
  const vaultEntry = {
    id: "note-1",
    path: "欢迎.md",
    title: "欢迎",
    is_dir: false,
    updated_at: now,
  };
  const tagEntry = {
    id: "note-3",
    path: "按标签命中.md",
    title: "按标签命中",
    is_dir: false,
    updated_at: now,
  };
  const vaultNotes: Record<string, unknown> = {
    "note-1": {
      id: "note-1",
      vault_id: vault.id,
      path: "欢迎.md",
      title: "欢迎",
      body: [
        "---",
        "status: draft",
        "owner: lhy",
        "tags: [工作]",
        "---",
        "# 欢迎",
        "",
        "这是 E2E mock 笔记正文。",
      ].join("\n"),
      updated_at: now,
    },
    "note-3": {
      id: "note-3",
      vault_id: vault.id,
      path: "按标签命中.md",
      title: "按标签命中",
      body: "# 按标签命中\n\n通过标签筛选打开的笔记。",
      updated_at: now,
    },
  };
  const appSettings = {
    default_agent: null,
    worktree_retention_days: 7,
    data_dir: "/tmp/temli-e2e-data",
    version: "0.8.0",
  };

  const bridge: Window["__temliMock"] = {
    calls: [],
    devStatus: {
      enabled: false,
      port: 1421,
      token: null,
      base_url: null,
    },
  };
  window.__temliMock = bridge;

  let callbackId = 0;
  const callbacks = new Map<number, (...args: unknown[]) => void>();

  function findStage(stageId: string) {
    for (const plan of plans) {
      for (const stage of plan.stages) {
        if (stage.id === stageId) return { plan, stage };
      }
    }
    return null;
  }

  function handle(cmd: string, args: Record<string, unknown>): unknown {
    switch (cmd) {
      case "list_projects":
        return [project];
      case "register_project":
        return project;
      case "list_tasks":
        return tasks;
      case "get_task":
        return args.taskId === "task-1" ? taskDetail : null;
      case "list_events":
        return taskDetail.events;
      case "list_dependencies":
        return { blockers: [], blocks: [] };
      case "list_context":
        return [];
      case "list_decisions":
        return [];
      case "list_reviews":
        return [];
      case "list_project_issues":
      case "get_project_issues":
        return [];
      case "list_task_attachments":
        return attachmentsByTask[String(args.taskId ?? "")] ?? [];
      case "read_task_attachment":
        return "ZmlsZQ==";
      case "list_adapters":
        return adapters;
      case "get_project_concurrency":
        return 2;
      case "get_auto_merge":
        return false;
      case "get_require_checks":
        return false;
      case "get_auto_docs":
        return false;
      case "reindex_workspace":
        return 0;
      case "list_docs":
        return [];
      case "list_plans":
        return plans;
      case "get_plan":
        return plans.find((plan) => plan.id === args.planId) ?? null;
      case "accept_stage": {
        const found = findStage(String(args.stageId ?? ""));
        if (found) {
          found.stage.status = "passed";
          found.stage.passed_at = now;
          found.stage.summary =
            typeof args.note === "string" ? args.note : null;
          found.stage.summary_at = now;
          found.plan.updated_at = now;
          if (found.plan.stages.every((stage) => stage.status === "passed")) {
            found.plan.status = "done";
          }
        }
        return null;
      }
      case "get_app_settings":
        return appSettings;
      case "set_default_agent":
      case "set_worktree_retention_days":
      case "set_project_concurrency":
      case "set_auto_merge":
      case "set_require_checks":
      case "set_auto_docs":
        return null;
      case "get_dev_status":
        return bridge.devStatus;
      case "set_dev_mode": {
        const enabled = args.enabled === true;
        const port = Number(args.port ?? 1421);
        bridge.devStatus = enabled
          ? {
              enabled: true,
              port,
              token: "temli-dev-token-e2e",
              base_url: `http://127.0.0.1:${port}`,
            }
          : { enabled: false, port, token: null, base_url: null };
        return bridge.devStatus;
      }
      case "read_app_log":
        return [
          "[2026-11-10T08:00:00Z] temli dev api listening on 127.0.0.1:1421",
          "[2026-11-10T08:00:01Z] served GET /health 200",
        ];
      case "list_vaults":
        return [vault];
      case "list_vault_templates":
        return [];
      case "index_vault":
        return 1;
      case "vault_tree":
        return [vaultEntry];
      case "vault_tags":
        return [{ tag: "工作", count: 2 }];
      case "notes_by_tag":
        return args.tag === "工作" ? [tagEntry] : [];
      case "get_vault_note":
        return vaultNotes[String(args.noteId ?? "")] ?? null;
      case "note_backlinks":
        return [];
      case "vault_graph":
        return { nodes: [], edges: [] };
      case "plugin:event|listen":
        return 1;
      case "plugin:event|unlisten":
        return null;
      default:
        return null;
    }
  }

  window.__TAURI_INTERNALS__ = {
    invoke: async (cmd, rawArgs) => {
      const args = (rawArgs ?? {}) as Record<string, unknown>;
      bridge.calls.push({ cmd, args });
      const result = handle(cmd, args);
      // 模拟 IPC 的序列化边界：每次调用都返回新对象，避免 React 因引用相同而跳过重渲染。
      if (result === null || result === undefined) return result;
      return JSON.parse(JSON.stringify(result));
    },
    transformCallback: (callback, once) => {
      callbackId += 1;
      const id = callbackId;
      callbacks.set(id, once
        ? (...args: unknown[]) => {
            callbacks.delete(id);
            callback(...args);
          }
        : callback);
      return id;
    },
    unregisterCallback: (id) => {
      callbacks.delete(id);
    },
    convertFileSrc: (path) => path,
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    registerListener: () => {},
    unregisterListener: () => {},
  };
}

export const test = base.extend<{ mock: MockController }>({
  mock: [
    async ({ page }, use) => {
      await page.addInitScript(installMockIpc);
      const controller: MockController = {
        calls: async (cmd?: string) =>
          page.evaluate((wanted) => {
            const all = window.__temliMock?.calls ?? [];
            return wanted ? all.filter((call) => call.cmd === wanted) : all;
          }, cmd),
      };
      await use(controller);
    },
    { auto: true },
  ],
});

export { expect };

/** 打开应用并等待项目列表就绪。 */
export async function openApp(page: Page): Promise<void> {
  await page.goto("/");
  await expect(
    page.getByRole("button", { name: "切换项目：temli-e2e" }),
  ).toBeVisible();
}

/** 点击左侧导航，打开对应视图。 */
export async function openView(page: Page, label: string): Promise<void> {
  await page.getByRole("button", { name: label, exact: true }).click();
}
