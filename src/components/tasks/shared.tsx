import type { ReactNode } from "react";
import type {
  AgentManifest,
  AgentRole,
  ProjectIssue,
  TaskDetail,
  TaskSummary,
} from "../../lib/api";

export type ContextOption = {
  id: string;
  kind: "note" | "doc";
  title: string;
};

export const POLL_INTERVAL_MS = 2000;
// start_task 在后台线程推进，queued 也可能由队列 worker 取走；继续轮询才能看到状态推进。
export const POLLING_STATUSES: Record<string, true> = {
  queued: true,
  running: true,
  verifying: true,
};

export type BoardColumn = {
  key: string;
  title: string;
  statuses: string[];
};

// 看板四阶段；未列出的状态（failed / interrupted）归入“异常/中断”列。
export const BOARD_COLUMNS: BoardColumn[] = [
  { key: "backlog", title: "准备开发", statuses: ["draft", "ready"] },
  {
    key: "active",
    title: "排队/开发中",
    statuses: ["queued", "running", "verifying"],
  },
  { key: "review", title: "待合并", statuses: ["needs_human"] },
  { key: "done", title: "已完成", statuses: ["merged"] },
];

// 已同步但还没转成任务的 open issue；ghost（远端已删除）和已关闭的不再提供“创建任务”。
export function issueNeedsTask(issue: ProjectIssue): boolean {
  return (
    issue.task_id === null &&
    !issue.ghost &&
    issue.github_state !== "closed"
  );
}

export const ADAPTER_DEFAULT_KEY = "__default__";
export const ADAPTER_FOLLOW_KEY = "__follow__";

const AGENT_ROLE_LABEL: Record<AgentRole, string> = {
  developer: "开发",
  reviewer: "审查",
};
const DEFAULT_AGENT_ROLES: AgentRole[] = ["developer", "reviewer"];

// 未声明 roles 的旧清单按“开发 + 审查”处理，避免选择器出现空列表。
export function adapterRoles(adapter: AgentManifest): AgentRole[] {
  return adapter.roles && adapter.roles.length > 0
    ? adapter.roles
    : DEFAULT_AGENT_ROLES;
}

type TaskAgentInfo = {
  developer: string | null;
  reviewer: string | null;
};

// list_tasks / get_task 都返回任务级适配器字段；null 表示回退默认。
export function taskAgentInfo(
  task: TaskSummary | TaskDetail["task"],
): TaskAgentInfo {
  return {
    developer: task.developer_adapter,
    reviewer: task.reviewer_adapter,
  };
}

// nextActor 由状态推导：需要人处理的状态标「待你」，Agent 驱动的状态标「Agent」。
const NEXT_ACTOR_TAGS: Record<
  string,
  { label: string; className: string; title: string }
> = {
  needs_human: {
    label: "待你",
    className:
      "bg-[var(--warning-soft)] text-[var(--warning-soft-foreground)]",
    title: "下一步需要你处理",
  },
  failed: {
    label: "待你",
    className:
      "bg-[var(--warning-soft)] text-[var(--warning-soft-foreground)]",
    title: "下一步需要你处理",
  },
  interrupted: {
    label: "待你",
    className:
      "bg-[var(--warning-soft)] text-[var(--warning-soft-foreground)]",
    title: "下一步需要你处理",
  },
  queued: {
    label: "Agent",
    className:
      "bg-[var(--accent-soft)] text-[var(--accent-soft-foreground)]",
    title: "下一步由 Agent 推进",
  },
  running: {
    label: "Agent",
    className:
      "bg-[var(--accent-soft)] text-[var(--accent-soft-foreground)]",
    title: "下一步由 Agent 推进",
  },
};

export function NextActorTag({ status }: { status: string }) {
  const next = NEXT_ACTOR_TAGS[status];
  if (!next) return null;
  return (
    <span
      className={`rounded px-1.5 py-0.5 text-label leading-none ${next.className}`}
      title={next.title}
    >
      {next.label}
    </span>
  );
}

export function adapterLabel(
  adapters: AgentManifest[],
  id: string | null,
): string {
  if (!id) return "默认适配器";
  const name = adapters.find((adapter) => adapter.id === id)?.name;
  return name && name.trim() ? name : id;
}

export function AgentRoleTags({ adapter }: { adapter: AgentManifest }) {
  return (
    <>
      {adapterRoles(adapter).map((role) => (
        <span
          key={role}
          className="shrink-0 rounded border border-[var(--border)] px-1 text-label text-[var(--muted)]"
        >
          {AGENT_ROLE_LABEL[role]}
        </span>
      ))}
    </>
  );
}

export function Section({
  title,
  icon,
  count,
  children,
}: {
  title: string;
  icon: ReactNode;
  count?: number;
  children: ReactNode;
}) {
  return (
    <section className="grid gap-2">
      <div className="flex items-center justify-between gap-2">
        <h3 className="flex items-center gap-1.5 text-label font-medium uppercase tracking-[0.06em] text-[var(--muted)]">
          {icon}
          {title}
        </h3>
        {count !== undefined && (
          <span className="text-label text-[var(--muted)]">{count}</span>
        )}
      </div>
      {children}
    </section>
  );
}

export function Muted({ children }: { children: ReactNode }) {
  return <p className="text-ui-sm text-[var(--muted)]">{children}</p>;
}
