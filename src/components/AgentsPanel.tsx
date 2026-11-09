import { useEffect, useState } from "react";
import { Button } from "@heroui/react";
import { Bot, RefreshCw, Terminal } from "lucide-react";
import {
  errorText,
  listAdapters,
  type AgentCapabilities,
  type AgentManifest,
  type AgentRole,
} from "../lib/api";

const AGENT_ROLE_LABEL: Record<AgentRole, string> = {
  developer: "开发",
  reviewer: "审查",
};
const DEFAULT_AGENT_ROLES: AgentRole[] = ["developer", "reviewer"];

const CAPABILITY_LABEL: Record<keyof AgentCapabilities, string> = {
  streaming: "流式输出",
  approvals: "审批",
  cancel: "取消",
  usage: "用量",
  cost_cap: "费用上限",
  resume: "续跑",
};

const CAPABILITY_KEYS: (keyof AgentCapabilities)[] = [
  "streaming",
  "approvals",
  "cancel",
  "usage",
  "cost_cap",
  "resume",
];

// 各适配器的补一句说明；未知适配器回退到通用文案。
const ADAPTER_NOTES: Record<string, string> = {
  omp: "原生 RPC 通道，支持流式输出、审批、取消与用量上报。",
  codex: "通过 Codex CLI 执行任务，支持流式输出与取消。",
  claude: "通过 Claude Code CLI 执行任务，支持流式输出与取消。",
  fake: "内置演示适配器，不依赖外部 CLI，用于本地联调。",
};

export function AgentsPanel({ projectId: _projectId }: { projectId: string }) {
  // 适配器清单是全局资源，与项目无关；projectId 仅保持页面组件签名统一。
  const [adapters, setAdapters] = useState<AgentManifest[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);
    listAdapters()
      .then((rows) => {
        if (!active) return;
        setAdapters(rows);
      })
      .catch((e) => {
        if (!active) return;
        setAdapters([]);
        setError(errorText(e));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [revision]);

  return (
    <div className="flex flex-col">
      <header className="flex items-start justify-between gap-3 pb-4">
        <div className="min-w-0">
          <h1 className="page-title">Agent</h1>
          <p className="mt-1 text-ui text-[var(--muted)]">
            可用的执行与审查适配器，以及它们的角色与能力
          </p>
        </div>
        <Button
          variant="ghost"
          size="sm"
          className="h-7 shrink-0 gap-1 px-2 text-ui"
          onPress={() => setRevision((value) => value + 1)}
          isDisabled={loading}
        >
          <RefreshCw size={14} className={loading ? "animate-spin" : undefined} />
          刷新
        </Button>
      </header>

      {error && (
        <div
          role="alert"
          className="mb-3 flex items-center justify-between gap-2 rounded-[10px] border border-[var(--border)] bg-[var(--background-secondary)] px-3 py-2"
        >
          <p className="min-w-0 break-words text-ui-sm text-[var(--danger)]">
            {error}
          </p>
          <Button
            variant="ghost"
            size="sm"
            className="h-7 shrink-0 px-2 text-ui"
            onPress={() => setRevision((value) => value + 1)}
          >
            重试
          </Button>
        </div>
      )}

      {loading && (
        <p className="py-6 text-center text-ui-sm text-[var(--muted)]">
          正在检测适配器…
        </p>
      )}

      {!loading && !error && adapters.length === 0 && (
        <div className="flex flex-col items-center gap-2 rounded-[10px] border border-dashed border-[var(--border)] px-4 py-10 text-center">
          <Terminal size={20} className="text-[var(--muted)] opacity-70" />
          <p className="text-ui font-medium">未检测到可用的适配器</p>
          <p className="max-w-[360px] text-ui-sm text-[var(--muted)]">
            请确认已安装 OMP / Codex / Claude CLI，并在启动应用前加入 PATH，然后重新检测。
          </p>
          <Button
            variant="secondary"
            size="sm"
            className="mt-1 h-7 px-2.5 text-ui"
            onPress={() => setRevision((value) => value + 1)}
          >
            重新检测
          </Button>
        </div>
      )}

      {!loading && adapters.length > 0 && (
        <div className="grid grid-cols-1 gap-3 lg:grid-cols-2">
          {adapters.map((adapter) => {
            const roles =
              adapter.roles && adapter.roles.length > 0
                ? adapter.roles
                : DEFAULT_AGENT_ROLES;
            return (
              <article
                key={adapter.id}
                aria-label={`${adapter.name} 适配器`}
                className="flex min-w-0 flex-col gap-2.5 rounded-[10px] border border-[var(--border)] bg-[var(--background-secondary)] p-3"
              >
                <div className="flex items-start justify-between gap-2">
                  <div className="min-w-0">
                    <h2 className="flex items-center gap-1.5 text-ui font-semibold">
                      <Bot size={14} className="shrink-0 text-[var(--muted)]" />
                      <span className="truncate">{adapter.name}</span>
                    </h2>
                    <p className="mt-0.5 truncate font-mono text-label text-[var(--muted)]">
                      {adapter.id} · v{adapter.version}
                    </p>
                  </div>
                  <div className="flex shrink-0 flex-wrap justify-end gap-1">
                    {roles.map((role) => (
                      <span
                        key={role}
                        title={role}
                        className="rounded border border-[var(--border)] bg-[var(--background)] px-1.5 py-0.5 text-label text-[var(--muted)]"
                      >
                        {AGENT_ROLE_LABEL[role] ?? role}
                      </span>
                    ))}
                  </div>
                </div>

                <div className="flex flex-wrap gap-1">
                  {CAPABILITY_KEYS.map((key) => {
                    const supported = adapter.capabilities[key];
                    return (
                      <span
                        key={key}
                        title={key}
                        className={`rounded-full border px-1.5 py-0.5 text-label ${
                          supported
                            ? "border-[var(--border)] bg-[var(--background)] text-[var(--foreground)]"
                            : "border-dashed border-[var(--border)] text-[var(--muted)] opacity-60"
                        }`}
                      >
                        {CAPABILITY_LABEL[key]}
                        {!supported && " · 不支持"}
                      </span>
                    );
                  })}
                </div>

                <p className="mt-auto text-ui-sm text-[var(--muted)]">
                  {ADAPTER_NOTES[adapter.id] ?? "外部 CLI 适配器，能力以清单为准。"}
                </p>
              </article>
            );
          })}
        </div>
      )}

      <section className="mt-4 rounded-[10px] border border-[var(--border)] bg-[var(--background-secondary)] p-3">
        <h2 className="text-ui font-medium">如何选择</h2>
        <ul className="mt-1.5 list-disc space-y-1 pl-4 text-ui-sm text-[var(--muted)]">
          <li>任务级可分别指定 developer 与 reviewer；developer 留空用默认适配器，reviewer 留空沿用 developer。</li>
          <li>每个任务可单独覆盖 model（provider/model 形式），不影响其它任务。</li>
          <li>roles 决定适配器能否担任开发或审查；能力标签表示其可用的交互。</li>
        </ul>
      </section>
    </div>
  );
}
