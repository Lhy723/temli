import { invoke } from "@tauri-apps/api/core";

export type TaskSummary = {
  id: string;
  title: string;
  status: string;
  attempts: number;
  criterion_count: number;
  created_at: string;
};

export type Criterion = {
  id: string;
  cmd: string;
  expect_exit: number;
  expect_match: string | null;
};

export type TaskRow = {
  id: string;
  project_id: string;
  title: string;
  status: string;
  prompt: string;
  attempts: number;
  created_at: string;
};

export type RunRow = {
  id: string;
  status: string;
  adapter_id: string;
  branch: string | null;
  worktree_path: string | null;
  started_at: string | null;
  ended_at: string | null;
  error: string | null;
};

export type EvidenceRow = {
  id: string;
  criterion_id: string | null;
  kind: string;
  cmd: string | null;
  exit_code: number | null;
  duration_ms: number | null;
  output_path: string | null;
  collected_at: string;
};

export type ReviewRow = {
  verdict: string;
  summary: string | null;
  risk_flags: unknown;
  test_diff: unknown;
};

export type DecisionRow = {
  id: string;
  task_id: string | null;
  kind: string;
  prompt: string;
  status: string;
  created_at: string;
  resolution: string | null;
};

export type EventRow = {
  id: string;
  kind: string;
  payload_json: string;
  at: string;
};

export type TaskDetail = {
  task: TaskRow;
  criteria: Criterion[];
  runs: RunRow[];
  evidence: EvidenceRow[];
  review: ReviewRow | null;
  decisions: DecisionRow[];
  events: EventRow[];
};

export type CreateTaskInput = {
  project_id: string;
  title: string;
  prompt: string;
  acceptance?: Criterion[];
};

export type DependencyInput = {
  task_id: string;
  depends_on_task_id: string;
};

export type TaskDependencies = {
  blockers: TaskSummary[];
  blocks: TaskSummary[];
};

export type SetProjectConcurrencyInput = {
  project_id: string;
  limit: number;
};

export function listTasks(projectId: string): Promise<TaskSummary[]> {
  return invoke<TaskSummary[]>("list_tasks", { projectId });
}

export function createTask(input: CreateTaskInput): Promise<TaskDetail> {
  return invoke<TaskDetail>("create_task", { input });
}

export function getTask(taskId: string): Promise<TaskDetail> {
  return invoke<TaskDetail>("get_task", { taskId });
}

export function startTask(taskId: string): Promise<void> {
  return invoke<void>("start_task", { taskId });
}

export function cancelTask(taskId: string): Promise<void> {
  return invoke<void>("cancel_task", { taskId });
}

export function listDecisions(projectId: string): Promise<DecisionRow[]> {
  return invoke<DecisionRow[]>("list_decisions", { projectId });
}

export function resolveDecision(
  id: string,
  resolution: string,
): Promise<void> {
  return invoke<void>("resolve_decision", { input: { id, resolution } });
}

export function listEvents(taskId: string): Promise<EventRow[]> {
  return invoke<EventRow[]>("list_events", { taskId });
}

export function addDependency(input: DependencyInput): Promise<void> {
  return invoke<void>("add_dependency", { input });
}

export function removeDependency(input: DependencyInput): Promise<void> {
  return invoke<void>("remove_dependency", { input });
}

export function listDependencies(taskId: string): Promise<TaskDependencies> {
  return invoke<TaskDependencies>("list_dependencies", { taskId });
}

export function getProjectConcurrency(projectId: string): Promise<number> {
  return invoke<number>("get_project_concurrency", { projectId });
}

export function setProjectConcurrency(
  input: SetProjectConcurrencyInput,
): Promise<void> {
  return invoke<void>("set_project_concurrency", { input });
}

const STATUS_LABELS: Record<string, string> = {
  draft: "草稿",
  ready: "就绪",
  queued: "排队中",
  running: "执行中",
  verifying: "验证中",
  merged: "已合并",
  failed: "失败",
  needs_human: "待决策",
  interrupted: "已中断",
  prepared: "已准备",
  completed: "完成",
  cancelled: "已取消",
  canceled: "已取消",
  waiting_approval: "待审批",
};

export function statusLabel(status: string): string {
  return STATUS_LABELS[status] ?? status;
}

export type StatusTone = "accent" | "success" | "warning" | "danger" | "default";

export function statusTone(status: string): StatusTone {
  switch (status) {
    case "queued":
    case "running":
    case "verifying":
      return "accent";
    case "merged":
    case "completed":
    case "pass":
      return "success";
    case "needs_human":
    case "interrupted":
    case "waiting_approval":
    case "cancelled":
    case "canceled":
      return "warning";
    case "failed":
    case "fail":
      return "danger";
    default:
      return "default";
  }
}

export function verdictLabel(verdict: string): string {
  switch (verdict) {
    case "pass":
      return "通过";
    case "fail":
      return "失败";
    case "needs_human":
      return "待人工";
    default:
      return verdict;
  }
}

const DECISION_KIND_LABELS: Record<string, string> = {
  timeout: "执行超时",
  retry_exhausted: "重试耗尽",
  post_merge_failed: "合并后验证失败",
  rebase_failed: "变基失败",
  evidence_gate: "证据门未通过",
  merge_failed: "合并失败",
  review_failed: "审查失败",
  test_changes_unreviewed: "测试改动未审查",
  agent_approval: "等待授权",
};

export function decisionKindLabel(kind: string): string {
  return DECISION_KIND_LABELS[kind] ?? kind;
}

/** 把后端可能为数组 / 字符串 / 对象的字段归一成可展示的字符串数组。 */
export function textList(value: unknown): string[] {
  if (value == null) return [];
  if (Array.isArray(value)) {
    return value.map((item) =>
      typeof item === "string" ? item : JSON.stringify(item),
    );
  }
  if (typeof value === "string") return value ? [value] : [];
  return [JSON.stringify(value)];
}

export function compactJson(value: string, max = 160): string {
  const oneLine = value.replace(/\s+/g, " ").trim();
  return oneLine.length > max ? `${oneLine.slice(0, max)}…` : oneLine;
}

export function formatDate(value: string | null | undefined): string {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  const hour = String(date.getHours()).padStart(2, "0");
  const minute = String(date.getMinutes()).padStart(2, "0");
  return `${month}-${day} ${hour}:${minute}`;
}

export function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return String(error);
}
