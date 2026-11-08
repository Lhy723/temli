import { invoke } from "@tauri-apps/api/core";

export type TaskSummary = {
  id: string;
  title: string;
  status: string;
  attempts: number;
  criterion_count: number;
  created_at: string;
  developer_adapter: string | null;
  reviewer_adapter: string | null;
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
  developer_adapter: string | null;
  reviewer_adapter: string | null;
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
  pr_number: number | null;
  pr_url: string | null;
  pr_state: string | null;
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

export type ProjectIssue = {
  id: string;
  title: string;
  status: string;
  github_state: string | null;
  github_number: number | null;
  ghost: boolean;
  task_id: string | null;
};

export type UpdateIssueInput = {
  item_id: string;
  title?: string;
  body?: string;
};

export type CreateTaskFromIssueInput = {
  item_id: string;
  title?: string;
  prompt?: string;
};

export type SetAutoMergeInput = {
  project_id: string;
  enabled: boolean;
};

export type SetRequireChecksInput = {
  project_id: string;
  enabled: boolean;
};

export type NoteMeta = {
  id: string;
  title: string;
  file_path: string;
  updated_at: string;
};

export type Note = {
  id: string;
  title: string;
  body: string;
  file_path: string;
  created_at: string;
  updated_at: string;
};

export type DocMeta = {
  id: string;
  title: string;
  path: string;
};

export type Doc = DocMeta & {
  body: string;
};

export type SearchHit = {
  id: string;
  kind: string;
  title: string;
  snippet: string;
};

export type AgentCapabilities = {
  streaming: boolean;
  approvals: boolean;
  cancel: boolean;
  usage: boolean;
  cost_cap: boolean;
  resume: boolean;
};

export type AgentManifest = {
  id: string;
  name: string;
  version: string;
  capabilities: AgentCapabilities;
};

export type SetTaskAgentsInput = {
  task_id: string;
  developer_adapter: string | null;
  reviewer_adapter: string | null;
};

export type Vault = {
  id: string;
  name: string;
  root_path: string;
  created_at: string;
};

/** 笔记库内的一条树项；目录项 is_dir=true（id/updated_at 可能为空）。 */
export type VaultEntry = {
  id?: string | null;
  path: string;
  title: string;
  is_dir: boolean;
  updated_at?: string | null;
};

export type VaultNote = {
  id: string;
  vault_id: string;
  path: string;
  title: string;
  body: string;
  updated_at: string;
};

export type VaultHit = {
  id: string;
  vault_id: string;
  path: string;
  title: string;
  snippet: string;
};

export type Backlink = {
  id: string;
  path: string;
  title: string;
  /** 反向链接里书写的原始目标，例如 `[[目标|别名]]` 中的「目标」。 */
  target: string;
};

export type VaultGraphNode = {
  id: string;
  path: string;
  title: string;
};

export type VaultGraphEdge = {
  /** 来源笔记 id。 */
  from: string;
  /** 已解析的目标笔记 id。 */
  to: string;
};

export type VaultGraph = {
  nodes: VaultGraphNode[];
  edges: VaultGraphEdge[];
};

export type AddVaultInput = {
  name: string;
  root_path: string;
  /** true 时目录不存在会自动创建；默认要求目录已存在。 */
  create?: boolean;
};

export type CreateVaultNoteInput = {
  vault_id: string;
  path: string;
  title: string;
  body?: string;
};

export type SaveVaultNoteInput = {
  note_id: string;
  title: string;
  body: string;
};

export type ContextItem = {
  id: string;
  kind: string;
  title: string;
  body?: string;
};

export type CreateNoteInput = {
  project_id: string;
  title: string;
  body: string;
};

export type UpdateNoteInput = {
  id: string;
  title: string;
  body: string;
};

export type CreateTaskFromNoteInput = {
  note_id: string;
  title?: string;
  prompt?: string;
};

export type LinkContextInput = {
  task_id: string;
  item_id: string;
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

export function syncNow(projectId: string): Promise<void> {
  return invoke<void>("sync_now", { projectId });
}

export function updateIssue(input: UpdateIssueInput): Promise<void> {
  return invoke<void>("update_issue", { input });
}

export function publishTask(taskId: string): Promise<number> {
  return invoke<number>("publish_task", { taskId });
}

export function getProjectIssues(projectId: string): Promise<ProjectIssue[]> {
  return invoke<ProjectIssue[]>("get_project_issues", { projectId });
}

export function getAutoMerge(projectId: string): Promise<boolean> {
  return invoke<boolean>("get_auto_merge", { projectId });
}

export function setAutoMerge(input: SetAutoMergeInput): Promise<void> {
  return invoke<void>("set_auto_merge", { input });
}

export function getRequireChecks(projectId: string): Promise<boolean> {
  return invoke<boolean>("get_require_checks", { projectId });
}

export function setRequireChecks(input: SetRequireChecksInput): Promise<void> {
  return invoke<void>("set_require_checks", { input });
}

export function listNotes(projectId: string): Promise<NoteMeta[]> {
  return invoke<NoteMeta[]>("list_notes", { projectId });
}

export function getNote(noteId: string): Promise<Note> {
  return invoke<Note>("get_note", { noteId });
}

export function createNote(input: CreateNoteInput): Promise<Note> {
  return invoke<Note>("create_note", { input });
}

export function updateNote(input: UpdateNoteInput): Promise<Note> {
  return invoke<Note>("update_note", { input });
}

export function deleteNote(noteId: string): Promise<void> {
  return invoke<void>("delete_note", { noteId });
}

export function listDocs(projectId: string): Promise<DocMeta[]> {
  return invoke<DocMeta[]>("list_docs", { projectId });
}

export function getDoc(docId: string): Promise<Doc> {
  return invoke<Doc>("get_doc", { docId });
}

export function reindexWorkspace(projectId: string): Promise<number> {
  return invoke<number>("reindex_workspace", { projectId });
}

export function searchWorkspace(
  projectId: string,
  query: string,
): Promise<SearchHit[]> {
  return invoke<SearchHit[]>("search_workspace", { projectId, query });
}

export function createTaskFromNote(
  input: CreateTaskFromNoteInput,
): Promise<TaskDetail> {
  return invoke<TaskDetail>("create_task_from_note", { input });
}

export function createTaskFromIssue(
  input: CreateTaskFromIssueInput,
): Promise<TaskDetail> {
  return invoke<TaskDetail>("create_task_from_issue", { input });
}

export function linkContext(input: LinkContextInput): Promise<void> {
  return invoke<void>("link_context", { input });
}

export function unlinkContext(input: LinkContextInput): Promise<void> {
  return invoke<void>("unlink_context", { input });
}

export function listContext(taskId: string): Promise<ContextItem[]> {
  return invoke<ContextItem[]>("list_context", { taskId });
}

export function listAdapters(): Promise<AgentManifest[]> {
  return invoke<AgentManifest[]>("list_adapters");
}

export function setTaskAgents(input: SetTaskAgentsInput): Promise<void> {
  return invoke<void>("set_task_agents", { input });
}

export function addVault(input: AddVaultInput): Promise<Vault> {
  return invoke<Vault>("add_vault", { input });
}

export function listVaults(): Promise<Vault[]> {
  return invoke<Vault[]>("list_vaults");
}

export function removeVault(vaultId: string): Promise<void> {
  return invoke<void>("remove_vault", { vaultId });
}

export function indexVault(vaultId: string): Promise<number> {
  return invoke<number>("index_vault", { vaultId });
}

export function vaultTree(vaultId: string): Promise<VaultEntry[]> {
  return invoke<VaultEntry[]>("vault_tree", { vaultId });
}

export function getVaultNote(noteId: string): Promise<VaultNote> {
  return invoke<VaultNote>("get_vault_note", { noteId });
}

export function createVaultNote(
  input: CreateVaultNoteInput,
): Promise<VaultNote> {
  return invoke<VaultNote>("create_vault_note", { input });
}

export function saveVaultNote(input: SaveVaultNoteInput): Promise<VaultNote> {
  return invoke<VaultNote>("save_vault_note", { input });
}

export function deleteVaultNote(noteId: string): Promise<void> {
  return invoke<void>("delete_vault_note", { noteId });
}

export function searchVault(
  vaultId: string,
  query: string,
): Promise<VaultHit[]> {
  return invoke<VaultHit[]>("search_vault", { vaultId, query });
}

export function noteBacklinks(noteId: string): Promise<Backlink[]> {
  return invoke<Backlink[]>("note_backlinks", { noteId });
}

export function vaultGraph(vaultId: string): Promise<VaultGraph> {
  return invoke<VaultGraph>("vault_graph", { vaultId });
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
  open: "打开",
  closed: "已关闭",
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
