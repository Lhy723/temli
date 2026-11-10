import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ChangeEvent,
  type ReactNode,
} from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  Button,
  Card,
  Chip,
  Description,
  Input,
  ListBox,
  Select,
  Switch,
  TextArea,
} from "@heroui/react";
import {
  AlertTriangle,
  CircleDot,
  CircleStop,
  Clock,
  ExternalLink,
  FileText,
  GitBranch,
  GitPullRequest,
  Inbox,
  LayoutGrid,
  Link2,
  List,
  ListChecks,
  Paperclip,
  Play,
  Plus,
  RefreshCw,
  ShieldCheck,
  Terminal,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import {
  addDependency,
  addTaskAttachment,
  cancelTask,
  compactJson,
  createTask,
  createTaskFromIssue,
  createTasksFromPlan,
  decisionKindLabel,
  deleteTaskAttachment,
  errorText,
  formatDate,
  getAutoDocs,
  getAutoMerge,
  getProjectConcurrency,
  getProjectIssues,
  getRequireChecks,
  getTask,
  linkContext,
  listAdapters,
  listContext,
  listDependencies,
  listDocs,
  listEvents,
  listTaskAttachments,
  listTasks,
  planProject,
  publishTask,
  readTaskAttachment,
  reindexWorkspace,
  removeDependency,
  resolveDecision,
  setAutoDocs,
  setAutoMerge,
  setProjectConcurrency,
  setRequireChecks,
  setTaskAgents,
  setTaskModel,
  startTask,
  statusLabel,
  statusTone,
  syncNow,
  textList,
  unlinkContext,
  updateIssue,
  verdictLabel,
  type AgentManifest,
  type AgentRole,
  type Attachment,
  type ContextItem,
  type EventRow,
  type PlanDraft,
  type ProjectIssue,
  type TaskDependencies,
  type TaskDetail,
  type TaskSummary,
  type UpdateIssueInput,
} from "../lib/api";

type ContextOption = {
  id: string;
  kind: "note" | "doc";
  title: string;
};

const POLL_INTERVAL_MS = 2000;
// start_task 在后台线程推进，queued 也可能由队列 worker 取走；继续轮询才能看到状态推进。
const POLLING_STATUSES: Record<string, true> = {
  queued: true,
  running: true,
  verifying: true,
};

type BoardColumn = {
  key: string;
  title: string;
  statuses: string[];
};

// 看板四阶段；未列出的状态（failed / interrupted）归入“异常/中断”列。
const BOARD_COLUMNS: BoardColumn[] = [
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
function issueNeedsTask(issue: ProjectIssue): boolean {
  return (
    issue.task_id === null &&
    !issue.ghost &&
    issue.github_state !== "closed"
  );
}

const ADAPTER_DEFAULT_KEY = "__default__";
const ADAPTER_FOLLOW_KEY = "__follow__";

const AGENT_ROLE_LABEL: Record<AgentRole, string> = {
  developer: "开发",
  reviewer: "审查",
};
const DEFAULT_AGENT_ROLES: AgentRole[] = ["developer", "reviewer"];

// 未声明 roles 的旧清单按“开发 + 审查”处理，避免选择器出现空列表。
function adapterRoles(adapter: AgentManifest): AgentRole[] {
  return adapter.roles && adapter.roles.length > 0
    ? adapter.roles
    : DEFAULT_AGENT_ROLES;
}

type TaskAgentInfo = {
  developer: string | null;
  reviewer: string | null;
};

// list_tasks / get_task 都返回任务级适配器字段；null 表示回退默认。
function taskAgentInfo(task: TaskSummary | TaskDetail["task"]): TaskAgentInfo {
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

function NextActorTag({ status }: { status: string }) {
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

function adapterLabel(adapters: AgentManifest[], id: string | null): string {
  if (!id) return "默认适配器";
  const name = adapters.find((adapter) => adapter.id === id)?.name;
  return name && name.trim() ? name : id;
}

function AgentRoleTags({ adapter }: { adapter: AgentManifest }) {
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

export function TasksPanel({
  projectId,
  focusTaskId,
  onFocusConsumed,
}: {
  projectId: string;
  focusTaskId?: string;
  onFocusConsumed?: () => void;
}) {
  const [tasks, setTasks] = useState<TaskSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showCreate, setShowCreate] = useState(false);
  const [showPlan, setShowPlan] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [view, setView] = useState<"board" | "table">("board");
  const [adapters, setAdapters] = useState<AgentManifest[]>([]);
  const [adaptersLoading, setAdaptersLoading] = useState(true);
  const [adaptersError, setAdaptersError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<TaskDetail | null>(null);
  const [events, setEvents] = useState<EventRow[]>([]);
  const [detailLoading, setDetailLoading] = useState(false);
  // 附件上传/删除完成后递增，通知仍挂载的 AttachmentSection 重新拉取列表；
  // 切换任务期间完成的写入也能在回到该任务时刷新。
  const [attachmentsRevision, setAttachmentsRevision] = useState(0);
  const refreshAttachments = useCallback(() => {
    setAttachmentsRevision((value) => value + 1);
  }, []);
  const [busy, setBusy] = useState(false);
  const [resolvingDecisionId, setResolvingDecisionId] = useState<string | null>(
    null,
  );
  const [cancellingTasks, setCancellingTasks] = useState<Record<string, true>>(
    {},
  );
  const [cancelError, setCancelError] = useState<{
    taskId: string;
    message: string;
  } | null>(null);
  const [dependencies, setDependencies] = useState<TaskDependencies | null>(
    null,
  );
  const [dependencyError, setDependencyError] = useState<string | null>(null);
  const [addingDependency, setAddingDependency] = useState(false);
  const [removingDependencyId, setRemovingDependencyId] = useState<
    string | null
  >(null);
  const [contextItems, setContextItems] = useState<ContextItem[] | null>(null);
  const [contextError, setContextError] = useState<string | null>(null);
  const [linkingContext, setLinkingContext] = useState(false);
  const [removingContextId, setRemovingContextId] = useState<string | null>(
    null,
  );
  const [contextOptions, setContextOptions] = useState<ContextOption[]>([]);
  const [contextOptionsLoading, setContextOptionsLoading] = useState(true);
  const [contextOptionsError, setContextOptionsError] = useState<string | null>(
    null,
  );
  const [concurrency, setConcurrency] = useState<number | null>(null);
  const [concurrencyInput, setConcurrencyInput] = useState("");
  const [concurrencySaving, setConcurrencySaving] = useState(false);
  const [concurrencyError, setConcurrencyError] = useState<string | null>(null);
  const [issues, setIssues] = useState<ProjectIssue[]>([]);
  const [issuesLoading, setIssuesLoading] = useState(true);
  const [issuesError, setIssuesError] = useState<string | null>(null);
  const [creatingIssueId, setCreatingIssueId] = useState<string | null>(null);
  const [createIssueError, setCreateIssueError] = useState<{
    issueId: string;
    message: string;
  } | null>(null);
  const [githubError, setGithubError] = useState<string | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [publishing, setPublishing] = useState(false);
  const [autoMerge, setAutoMergeEnabled] = useState<boolean | null>(null);
  const [autoMergeSaving, setAutoMergeSaving] = useState(false);
  const [autoMergeError, setAutoMergeError] = useState<string | null>(null);
  const [requireChecks, setRequireChecksEnabled] = useState<boolean | null>(
    null,
  );
  const [requireChecksSaving, setRequireChecksSaving] = useState(false);
  const [requireChecksError, setRequireChecksError] = useState<string | null>(
    null,
  );
  const [autoDocs, setAutoDocsEnabled] = useState<boolean | null>(null);
  const [autoDocsSaving, setAutoDocsSaving] = useState(false);
  const [autoDocsError, setAutoDocsError] = useState<string | null>(null);
  const selectedRef = useRef<string | null>(null);
  const detailRequestRef = useRef(0);
  const projectRef = useRef(projectId);
  const tasksSettledRef = useRef<string | null>(null);
  const focusConsumedRef = useRef<string | null>(null);
  // 轮询回调里需要最新的自动文档开关，用 ref 读取以免重建轮询定时器。
  const autoDocsRef = useRef(false);
  // 上一轮轮询看到的状态快照；用于识别“非 merged → merged”的合并转变。
  const polledStatusesRef = useRef<Map<string, string> | null>(null);

  useEffect(() => {
    let cancelled = false;
    projectRef.current = projectId;
    selectedRef.current = null;
    tasksSettledRef.current = null;
    focusConsumedRef.current = null;
    polledStatusesRef.current = null;
    setLoading(true);
    setError(null);
    setTasks([]);
    setSelectedId(null);
    setDetail(null);
    setEvents([]);
    setResolvingDecisionId(null);
    setCancellingTasks({});
    setCancelError(null);
    setDependencies(null);
    setDependencyError(null);
    setAddingDependency(false);
    setRemovingDependencyId(null);
    setContextItems(null);
    setContextError(null);
    setLinkingContext(false);
    setRemovingContextId(null);
    setContextOptions([]);
    setContextOptionsLoading(true);
    setContextOptionsError(null);
    setIssues([]);
    setIssuesLoading(true);
    setIssuesError(null);
    setCreatingIssueId(null);
    setCreateIssueError(null);
    setGithubError(null);
    setSyncing(false);
    setPublishing(false);
    setShowCreate(false);
    setShowPlan(false);
    setNotice(null);
    listTasks(projectId)
      .then((next) => {
        if (cancelled) return;
        polledStatusesRef.current = new Map(
          next.map((task) => [task.id, task.status] as const),
        );
        setTasks(next);
      })
      .catch((e) => {
        if (!cancelled) setError(errorText(e));
      })
      .finally(() => {
        if (!cancelled) {
          tasksSettledRef.current = projectId;
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  useEffect(() => {
    let cancelled = false;
    setAdaptersLoading(true);
    setAdaptersError(null);
    listAdapters()
      .then((next) => {
        if (!cancelled) setAdapters(next);
      })
      .catch((e) => {
        if (!cancelled) setAdaptersError(errorText(e));
      })
      .finally(() => {
        if (!cancelled) setAdaptersLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    setContextOptions([]);
    setContextOptionsLoading(true);
    setContextOptionsError(null);
    Promise.all([
      reindexWorkspace(projectId)
        .catch(() => null)
        .then(() => listDocs(projectId)),
    ])
      .then(([docs]) => {
        if (cancelled) return;
        setContextOptions(
          docs.map((doc) => ({
            id: doc.id,
            kind: "doc" as const,
            title: doc.title,
          })),
        );
      })
      .catch((e) => {
        if (!cancelled) setContextOptionsError(errorText(e));
      })
      .finally(() => {
        if (!cancelled) setContextOptionsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  useEffect(() => {
    let cancelled = false;
    setConcurrency(null);
    setConcurrencyInput("");
    setConcurrencyError(null);
    getProjectConcurrency(projectId)
      .then((limit) => {
        if (cancelled) return;
        setConcurrency(limit);
        setConcurrencyInput(String(limit));
      })
      .catch((e) => {
        if (!cancelled) setConcurrencyError(errorText(e));
      });
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  useEffect(() => {
    let cancelled = false;
    getProjectIssues(projectId)
      .then((next) => {
        if (!cancelled) setIssues(next);
      })
      .catch((e) => {
        if (!cancelled) setIssuesError(errorText(e));
      })
      .finally(() => {
        if (!cancelled) setIssuesLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  useEffect(() => {
    let cancelled = false;
    setAutoMergeEnabled(null);
    setAutoMergeSaving(false);
    setAutoMergeError(null);
    getAutoMerge(projectId)
      .then((enabled) => {
        if (cancelled || projectRef.current !== projectId) return;
        setAutoMergeEnabled(enabled);
      })
      .catch((e) => {
        if (cancelled || projectRef.current !== projectId) return;
        setAutoMergeEnabled(false);
        setAutoMergeError(errorText(e));
      });
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  useEffect(() => {
    let cancelled = false;
    setRequireChecksEnabled(null);
    setRequireChecksSaving(false);
    setRequireChecksError(null);
    getRequireChecks(projectId)
      .then((enabled) => {
        if (cancelled || projectRef.current !== projectId) return;
        setRequireChecksEnabled(enabled);
      })
      .catch((e) => {
        if (cancelled || projectRef.current !== projectId) return;
        setRequireChecksEnabled(false);
        setRequireChecksError(errorText(e));
      });
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  useEffect(() => {
    let cancelled = false;
    setAutoDocsEnabled(null);
    setAutoDocsSaving(false);
    setAutoDocsError(null);
    getAutoDocs(projectId)
      .then((enabled) => {
        if (cancelled || projectRef.current !== projectId) return;
        setAutoDocsEnabled(enabled);
      })
      .catch((e) => {
        if (cancelled || projectRef.current !== projectId) return;
        setAutoDocsEnabled(false);
        setAutoDocsError(errorText(e));
      });
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  useEffect(() => {
    autoDocsRef.current = autoDocs === true;
  }, [autoDocs]);

  async function refreshTasks() {
    const requestedFor = projectId;
    const next = await listTasks(requestedFor);
    if (projectRef.current !== requestedFor) return;
    setTasks(next);
  }

  async function refreshIssues() {
    const requestedFor = projectId;
    const next = await getProjectIssues(requestedFor);
    if (projectRef.current !== requestedFor) return;
    setIssuesError(null);
    setIssues(next);
  }

  async function syncGithub() {
    if (syncing) return;
    setSyncing(true);
    setGithubError(null);
    let failure: string | null = null;
    try {
      await syncNow(projectId);
    } catch (e) {
      failure = errorText(e);
    }
    try {
      const taskId = selectedRef.current;
      await Promise.all([
        refreshTasks(),
        refreshIssues(),
        taskId ? loadDetail(taskId, true) : Promise.resolve(),
      ]);
    } catch (e) {
      failure ??= errorText(e);
    }
    if (failure) setGithubError(failure);
    setSyncing(false);
  }

  async function publishSelected() {
    const taskId = selectedRef.current;
    if (!taskId || publishing) return;
    setPublishing(true);
    setGithubError(null);
    try {
      await publishTask(taskId);
      await Promise.all([
        refreshIssues(),
        loadDetail(taskId, true),
        refreshTasks(),
      ]);
    } catch (e) {
      setGithubError(errorText(e));
    } finally {
      setPublishing(false);
    }
  }

  async function saveAutoMerge(enabled: boolean) {
    if (autoMerge === null || autoMergeSaving) return;
    const requestedFor = projectId;
    setAutoMergeSaving(true);
    setAutoMergeError(null);
    try {
      await setAutoMerge({ project_id: requestedFor, enabled });
      if (projectRef.current !== requestedFor) return;
      setAutoMergeEnabled(enabled);
    } catch (e) {
      if (projectRef.current !== requestedFor) return;
      setAutoMergeError(errorText(e));
    } finally {
      if (projectRef.current === requestedFor) setAutoMergeSaving(false);
    }
  }

  async function saveRequireChecks(enabled: boolean) {
    if (requireChecks === null || requireChecksSaving) return;
    const requestedFor = projectId;
    setRequireChecksSaving(true);
    setRequireChecksError(null);
    try {
      await setRequireChecks({ project_id: requestedFor, enabled });
      if (projectRef.current !== requestedFor) return;
      setRequireChecksEnabled(enabled);
    } catch (e) {
      if (projectRef.current !== requestedFor) return;
      setRequireChecksError(errorText(e));
    } finally {
      if (projectRef.current === requestedFor) setRequireChecksSaving(false);
    }
  }

  async function saveAutoDocs(enabled: boolean) {
    if (autoDocs === null || autoDocsSaving) return;
    const requestedFor = projectId;
    setAutoDocsSaving(true);
    setAutoDocsError(null);
    try {
      await setAutoDocs({ project_id: requestedFor, enabled });
      if (projectRef.current !== requestedFor) return;
      setAutoDocsEnabled(enabled);
    } catch (e) {
      if (projectRef.current !== requestedFor) return;
      setAutoDocsError(errorText(e));
    } finally {
      if (projectRef.current === requestedFor) setAutoDocsSaving(false);
    }
  }

  async function loadDetail(taskId: string, silent: boolean) {
    if (!silent) setDetailLoading(true);
    const request = ++detailRequestRef.current;
    try {
      let dependencyFailure: string | null = null;
      let contextFailure: string | null = null;
      const [nextDetail, nextEvents, nextDependencies, nextContext] =
        await Promise.all([
          getTask(taskId),
          listEvents(taskId).catch(() => null),
          listDependencies(taskId).catch((e) => {
            dependencyFailure = errorText(e);
            return null;
          }),
          listContext(taskId).catch((e) => {
            contextFailure = errorText(e);
            return null;
          }),
        ]);
      if (
        selectedRef.current !== taskId ||
        request !== detailRequestRef.current
      ) {
        return;
      }
      setDetail(nextDetail);
      setEvents(nextEvents ?? nextDetail.events);
      if (nextDependencies) setDependencies(nextDependencies);
      setDependencyError(dependencyFailure);
      if (nextContext) setContextItems(nextContext);
      setContextError(contextFailure);
      if (!silent) setError(null);
    } catch (e) {
      if (
        selectedRef.current === taskId &&
        request === detailRequestRef.current
      ) {
        setError(errorText(e));
      }
    } finally {
      if (selectedRef.current === taskId && !silent) setDetailLoading(false);
    }
  }

  function selectTask(taskId: string) {
    selectedRef.current = taskId;
    setSelectedId(taskId);
    setDetail(null);
    setEvents([]);
    setDependencies(null);
    setDependencyError(null);
    setContextItems(null);
    setContextError(null);
    setLinkingContext(false);
    setRemovingContextId(null);
    setCancelError(null);
    setGithubError(null);
    void loadDetail(taskId, false);
  }

  function closeDetail() {
    selectedRef.current = null;
    setSelectedId(null);
    setDetail(null);
    setEvents([]);
    setDependencies(null);
    setDependencyError(null);
    setContextItems(null);
    setContextError(null);
    setLinkingContext(false);
    setRemovingContextId(null);
    setDetailLoading(false);
    setCancelError(null);
    setGithubError(null);
  }

  async function handleCreated(created: TaskDetail) {
    setShowCreate(false);
    setNotice(null);
    setError(null);
    setDetailLoading(false);
    selectedRef.current = created.task.id;
    setSelectedId(created.task.id);
    setDetail(created);
    setEvents(created.events);
    setDependencies({ blockers: [], blocks: [] });
    setDependencyError(null);
    setContextItems([]);
    setContextError(null);
    setLinkingContext(false);
    setRemovingContextId(null);
    try {
      await Promise.all([refreshTasks(), refreshIssues()]);
    } catch (e) {
      setError(errorText(e));
    }
  }

  async function handlePlanCreated(created: TaskDetail[]) {
    setShowPlan(false);
    setNotice(`已从计划创建 ${created.length} 个任务`);
    setError(null);
    try {
      await Promise.all([refreshTasks(), refreshIssues()]);
    } catch (e) {
      setError(errorText(e));
    }
  }

  async function createTaskFromIssueItem(issue: ProjectIssue) {
    if (creatingIssueId) return;
    const requestedFor = projectId;
    setCreatingIssueId(issue.id);
    setCreateIssueError(null);
    try {
      const created = await createTaskFromIssue({ item_id: issue.id });
      if (projectRef.current !== requestedFor) return;
      // 先本地建立关联，刷新返回前看板不会同时出现任务卡与 issue 卡。
      setIssues((prev) =>
        prev.map((item) =>
          item.id === issue.id ? { ...item, task_id: created.task.id } : item,
        ),
      );
      await handleCreated(created);
    } catch (e) {
      if (projectRef.current !== requestedFor) return;
      setCreateIssueError({ issueId: issue.id, message: errorText(e) });
      // 命令可能已在后端成功但响应丢失；刷新一次以免残留重复卡片。
      void Promise.all([refreshTasks(), refreshIssues()]).catch(() => {});
    } finally {
      if (projectRef.current === requestedFor) setCreatingIssueId(null);
    }
  }

  // 已发布 issue 的远端编辑；成功后统一走列表刷新，不做本地乐观改写。
  async function saveIssueUpdate(
    input: UpdateIssueInput,
  ): Promise<string | null> {
    const requestedFor = projectId;
    try {
      await updateIssue(input);
      if (projectRef.current !== requestedFor) return null;
      const taskId = selectedRef.current;
      await Promise.all([
        refreshIssues(),
        refreshTasks(),
        taskId ? loadDetail(taskId, true) : Promise.resolve(),
      ]);
      return null;
    } catch (e) {
      if (projectRef.current !== requestedFor) return null;
      const message = errorText(e);
      return message.includes("远端已变更")
        ? "远端已变更，请先同步后重试"
        : message;
    }
  }

  async function startSelected() {
    const taskId = selectedRef.current;
    if (!taskId) return;
    setBusy(true);
    setError(null);
    try {
      await startTask(taskId);
      await Promise.all([refreshTasks(), loadDetail(taskId, true)]);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  async function cancelForTask(taskId: string) {
    if (cancellingTasks[taskId]) return;
    setCancellingTasks((prev) => ({ ...prev, [taskId]: true }));
    setCancelError(null);
    try {
      await cancelTask(taskId);
    } catch (e) {
      setCancelError({ taskId, message: errorText(e) });
    } finally {
      const reloads: Promise<void>[] = [refreshTasks()];
      if (selectedRef.current === taskId) {
        reloads.push(loadDetail(taskId, true));
      }
      try {
        await Promise.all(reloads);
      } catch (e) {
        setCancelError({ taskId, message: errorText(e) });
      }
      setCancellingTasks((prev) => {
        if (!prev[taskId]) return prev;
        const next = { ...prev };
        delete next[taskId];
        return next;
      });
    }
  }

  async function addSelectedDependency(
    dependsOnTaskId: string,
  ): Promise<boolean> {
    const taskId = selectedRef.current;
    if (!taskId) return false;
    setAddingDependency(true);
    setDependencyError(null);
    let failure: string | null = null;
    try {
      await addDependency({
        task_id: taskId,
        depends_on_task_id: dependsOnTaskId,
      });
    } catch (e) {
      failure = errorText(e);
    }
    try {
      await Promise.all([loadDetail(taskId, true), refreshTasks()]);
    } catch (e) {
      setError(errorText(e));
    }
    if (failure && selectedRef.current === taskId) {
      setDependencyError(failure);
    }
    setAddingDependency(false);
    return failure === null;
  }

  async function removeSelectedDependency(dependsOnTaskId: string) {
    const taskId = selectedRef.current;
    if (!taskId) return;
    setRemovingDependencyId(dependsOnTaskId);
    setDependencyError(null);
    let failure: string | null = null;
    try {
      await removeDependency({
        task_id: taskId,
        depends_on_task_id: dependsOnTaskId,
      });
    } catch (e) {
      failure = errorText(e);
    }
    try {
      await Promise.all([loadDetail(taskId, true), refreshTasks()]);
    } catch (e) {
      setError(errorText(e));
    }
    if (failure && selectedRef.current === taskId) {
      setDependencyError(failure);
    }
    setRemovingDependencyId(null);
  }

  async function linkSelectedContext(itemId: string): Promise<boolean> {
    const taskId = selectedRef.current;
    if (!taskId) return false;
    setLinkingContext(true);
    setContextError(null);
    let failure: string | null = null;
    try {
      await linkContext({ task_id: taskId, item_id: itemId });
    } catch (e) {
      failure = errorText(e);
    }
    try {
      if (selectedRef.current === taskId) {
        await loadDetail(taskId, true);
      }
    } catch (e) {
      setError(errorText(e));
    }
    if (failure && selectedRef.current === taskId) {
      setContextError(failure);
    }
    setLinkingContext(false);
    return failure === null;
  }

  async function unlinkSelectedContext(itemId: string) {
    const taskId = selectedRef.current;
    if (!taskId) return;
    setRemovingContextId(itemId);
    setContextError(null);
    let failure: string | null = null;
    try {
      await unlinkContext({ task_id: taskId, item_id: itemId });
    } catch (e) {
      failure = errorText(e);
    }
    try {
      if (selectedRef.current === taskId) {
        await loadDetail(taskId, true);
      }
    } catch (e) {
      setError(errorText(e));
    }
    if (failure && selectedRef.current === taskId) {
      setContextError(failure);
    }
    setRemovingContextId(null);
  }

  async function saveConcurrency() {
    const limit = Number(concurrencyInput);
    if (!Number.isInteger(limit) || limit < 1 || limit > 16) {
      setConcurrencyError("并发上限需为 1–16 的整数。");
      return;
    }
    setConcurrencySaving(true);
    setConcurrencyError(null);
    try {
      await setProjectConcurrency({ project_id: projectId, limit });
      setConcurrency(limit);
      setConcurrencyInput(String(limit));
    } catch (e) {
      setConcurrencyError(errorText(e));
    } finally {
      setConcurrencySaving(false);
    }
  }

  async function saveSelectedAgents(
    developer: string | null,
    reviewer: string | null,
  ): Promise<string | null> {
    const taskId = selectedRef.current;
    if (!taskId) return "未选择任务。";
    try {
      await setTaskAgents({
        task_id: taskId,
        developer_adapter: developer,
        reviewer_adapter: reviewer,
      });
    } catch (e) {
      return errorText(e);
    }
    try {
      await Promise.all([loadDetail(taskId, true), refreshTasks()]);
    } catch (e) {
      return errorText(e);
    }
    return null;
  }

  async function saveSelectedModel(
    model: string | null,
  ): Promise<string | null> {
    const taskId = selectedRef.current;
    if (!taskId) return "未选择任务。";
    try {
      await setTaskModel({ task_id: taskId, model });
    } catch (e) {
      return errorText(e);
    }
    try {
      await Promise.all([loadDetail(taskId, true), refreshTasks()]);
    } catch (e) {
      return errorText(e);
    }
    return null;
  }

  async function resolveTaskDecision(
    decisionId: string,
    resolution: "allow" | "deny",
  ) {
    const taskId = selectedRef.current;
    if (!taskId) return;
    setResolvingDecisionId(decisionId);
    setError(null);
    let failure: string | null = null;
    try {
      await resolveDecision(decisionId, resolution);
    } catch (e) {
      failure = errorText(e);
    } finally {
      await loadDetail(taskId, true);
      if (failure) setError(failure);
      setResolvingDecisionId(null);
    }
  }

  const selectedStatus = detail?.task.status;
  const hasOpenDecisions =
    detail?.decisions.some((decision) => decision.status === "open") ?? false;
  const polling =
    tasks.some((task) => POLLING_STATUSES[task.status]) ||
    (selectedStatus ? POLLING_STATUSES[selectedStatus] === true : false) ||
    hasOpenDecisions;

  useEffect(() => {
    if (!polling) return;
    let cancelled = false;
    let inFlight = false;

    const tick = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const nextTasks = await listTasks(projectId);
        if (cancelled) return;
        // 合并后后端会紧接着排队文档任务；观察到“非 merged → merged”的转变时
        // 立即补一次列表刷新，避免轮询因没有活跃任务而停摆、漏掉新的文档任务。
        const previousStatuses = polledStatusesRef.current;
        polledStatusesRef.current = new Map(
          nextTasks.map((task) => [task.id, task.status] as const),
        );
        const justMerged =
          previousStatuses !== null &&
          nextTasks.some(
            (task) =>
              task.status === "merged" &&
              previousStatuses.get(task.id) !== undefined &&
              previousStatuses.get(task.id) !== "merged",
          );
        setTasks(nextTasks);
        if (justMerged && autoDocsRef.current) {
          // 只补一次刷新，不新增定时器。
          void refreshTasks().catch((e) => {
            if (!cancelled) setError(errorText(e));
          });
        }
        const taskId = selectedRef.current;
        if (taskId) {
          const request = ++detailRequestRef.current;
          const [nextDetail, nextEvents, nextDependencies, nextContext] =
            await Promise.all([
              getTask(taskId),
              listEvents(taskId).catch(() => null),
              listDependencies(taskId).catch(() => null),
              listContext(taskId).catch(() => null),
            ]);
          if (
            cancelled ||
            selectedRef.current !== taskId ||
            request !== detailRequestRef.current
          ) {
            return;
          }
          setDetail(nextDetail);
          setEvents(nextEvents ?? nextDetail.events);
          if (nextDependencies) setDependencies(nextDependencies);
          if (nextContext) setContextItems(nextContext);
        }
      } catch (e) {
        if (!cancelled) setError(errorText(e));
      } finally {
        inFlight = false;
      }
    };

    const timer = window.setInterval(() => {
      void tick();
    }, POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [polling, projectId]);

  // 搜索面板深链：任务列表就绪后选中目标任务并消费一次，避免重复触发。
  useEffect(() => {
    if (!focusTaskId) {
      focusConsumedRef.current = null;
      return;
    }
    if (focusConsumedRef.current === focusTaskId) return;
    if (loading || tasksSettledRef.current !== projectId) return;
    focusConsumedRef.current = focusTaskId;
    if (tasks.some((task) => task.id === focusTaskId)) {
      selectTask(focusTaskId);
    }
    onFocusConsumed?.();
  }, [focusTaskId, projectId, loading, tasks, onFocusConsumed]);

  const parsedConcurrency = Number(concurrencyInput);
  const concurrencyValid =
    Number.isInteger(parsedConcurrency) &&
    parsedConcurrency >= 1 &&
    parsedConcurrency <= 16;
  const concurrencyDirty =
    concurrency !== null && parsedConcurrency !== concurrency;
  const selectedTaskId = detail?.task.id ?? null;
  const selectedIssue =
    selectedTaskId === null
      ? null
      : (issues.find((item) => item.task_id === selectedTaskId) ?? null);
  const pendingIssues = issues.filter(issueNeedsTask);

  return (
    <div className="relative flex h-full min-h-0 gap-3">
      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 pb-3">
          <div className="flex items-center gap-2">
            <span className="flex items-center gap-1.5 text-ui-sm text-[var(--muted)]">
              <CircleDot size={14} />
              {tasks.length} 个任务
            </span>
            {pendingIssues.length > 0 && (
              <span
                className="flex items-center gap-1.5 text-ui-sm text-[var(--muted)]"
                title="还没有关联任务的 open issue"
              >
                <Inbox size={14} />
                {pendingIssues.length} 个 issue 待创建
              </span>
            )}
            <div className="flex h-8 items-center rounded-lg border border-[var(--border)] p-0.5">
              <Button
                variant={view === "board" ? "secondary" : "ghost"}
                size="sm"
                className="h-7 gap-1 px-2 text-ui-sm"
                aria-pressed={view === "board"}
                onPress={() => setView("board")}
              >
                <LayoutGrid size={13} />
                看板
              </Button>
              <Button
                variant={view === "table" ? "secondary" : "ghost"}
                size="sm"
                className="h-7 gap-1 px-2 text-ui-sm"
                aria-pressed={view === "table"}
                onPress={() => setView("table")}
              >
                <List size={13} />
                列表
              </Button>
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <div
              className="flex h-8 items-center gap-3 rounded-lg bg-[var(--background-secondary)] px-2.5"
              title="要求 CI checks：无 checks 时阻止自动合并 · 自动更新文档：合并后创建文档任务"
            >
              <Switch
                size="sm"
                isSelected={autoMerge === true}
                isDisabled={autoMerge === null || autoMergeSaving}
                onChange={(enabled) => void saveAutoMerge(enabled)}
                className="text-ui-sm text-[var(--muted)]"
              >
                <Switch.Content>
                  <Switch.Control>
                    <Switch.Thumb />
                  </Switch.Control>
                  自动合并
                </Switch.Content>
              </Switch>
              <Switch
                size="sm"
                isSelected={requireChecks === true}
                isDisabled={requireChecks === null || requireChecksSaving}
                onChange={(enabled) => void saveRequireChecks(enabled)}
                className="text-ui-sm text-[var(--muted)]"
              >
                <Switch.Content>
                  <Switch.Control>
                    <Switch.Thumb />
                  </Switch.Control>
                  要求 CI checks
                </Switch.Content>
                <Description className="sr-only">
                  无 checks 时阻止自动合并
                </Description>
              </Switch>
              <Switch
                size="sm"
                isSelected={autoDocs === true}
                isDisabled={autoDocs === null || autoDocsSaving}
                onChange={(enabled) => void saveAutoDocs(enabled)}
                className="text-ui-sm text-[var(--muted)]"
              >
                <Switch.Content>
                  <Switch.Control>
                    <Switch.Thumb />
                  </Switch.Control>
                  自动更新文档
                </Switch.Content>
                <Description className="sr-only">
                  合并后创建文档任务
                </Description>
              </Switch>
            </div>
            <div className="flex h-8 items-center gap-1.5 rounded-lg bg-[var(--background-secondary)] px-2 text-ui-sm text-[var(--muted)]">
              <span className="shrink-0">并发</span>
              <Input
                aria-label="项目并发上限"
                type="number"
                min={1}
                max={16}
                step={1}
                value={concurrencyInput}
                onChange={(e) => {
                  setConcurrencyInput(e.target.value);
                  setConcurrencyError(null);
                }}
                className="h-7 w-14 text-ui"
                disabled={concurrency === null || concurrencySaving}
              />
              <span className="shrink-0">当前 {concurrency ?? "…"}</span>
              <Button
                variant="secondary"
                size="sm"
                className="h-7 px-2 text-ui"
                onPress={() => void saveConcurrency()}
                isDisabled={
                  !concurrencyDirty || !concurrencyValid || concurrencySaving
                }
              >
                {concurrencySaving ? "保存中…" : "保存"}
              </Button>
            </div>
            <Button
              variant="secondary"
              size="sm"
              className="h-8 gap-1 px-3 text-ui"
              aria-pressed={showPlan}
              onPress={() => {
                setShowPlan((prev) => !prev);
                setShowCreate(false);
                setNotice(null);
              }}
            >
              <ListChecks size={14} />
              规划
            </Button>
            <Button
              variant="primary"
              size="sm"
              className="h-8 gap-1 px-3 text-ui"
              onPress={() => {
                setShowCreate((prev) => !prev);
                setShowPlan(false);
                setNotice(null);
              }}
            >
              <Plus size={14} />
              新建任务
            </Button>
          </div>
        </div>

        {(concurrencyError ||
          (concurrencyDirty && !concurrencyValid)) && (
          <p role="alert" className="mb-1 text-ui-sm text-[var(--danger)]">
            {concurrencyError ?? "并发上限需为 1–16 的整数。"}
          </p>
        )}

        {autoMergeError && (
          <p role="alert" className="mb-1 text-ui-sm text-[var(--danger)]">
            {autoMergeError}
          </p>
        )}

        {requireChecksError && (
          <p role="alert" className="mb-1 text-ui-sm text-[var(--danger)]">
            {requireChecksError}
          </p>
        )}

        {autoDocsError && (
          <p role="alert" className="mb-1 text-ui-sm text-[var(--danger)]">
            {autoDocsError}
          </p>
        )}

        {error && (
          <p role="alert" className="mb-1 text-ui-sm text-[var(--danger)]">
            {error}
          </p>
        )}

        {issuesError && (
          <p role="alert" className="mb-1 text-ui-sm text-[var(--danger)]">
            issue 加载失败：{issuesError}
          </p>
        )}

        {showCreate && (
          <CreateTaskForm
            projectId={projectId}
            onCancel={() => setShowCreate(false)}
            onCreated={handleCreated}
          />
        )}

        {showPlan && (
          <PlanPanel
            projectId={projectId}
            onCancel={() => setShowPlan(false)}
            onCreated={handlePlanCreated}
          />
        )}

        {notice && (
          <p role="status" className="mb-2 text-ui-sm text-[var(--muted)]">
            {notice}
          </p>
        )}

        <div className="min-h-0 flex-1 overflow-auto">
          {loading && (
            <p className="text-ui-sm text-[var(--muted)]">加载中…</p>
          )}

          {!loading &&
            !error &&
            tasks.length === 0 &&
            pendingIssues.length === 0 && (
              <div className="flex h-full flex-col items-center justify-center gap-2 text-center">
                <ListChecks
                  size={20}
                  className="text-[var(--muted)] opacity-70"
                />
                <p className="text-ui font-medium">还没有任务</p>
                <p className="text-ui-sm text-[var(--muted)]">
                  新建一个任务开始执行。
                </p>
                {!showCreate && (
                  <Button
                    variant="secondary"
                    size="sm"
                    className="mt-1 h-8 px-3 text-ui"
                    onPress={() => {
                      setShowCreate(true);
                      setShowPlan(false);
                    }}
                  >
                    新建任务
                  </Button>
                )}
              </div>
            )}

          {!loading &&
            (tasks.length > 0 || pendingIssues.length > 0) &&
            view === "board" && (
              <TaskBoard
                tasks={tasks}
                issues={issues}
                pendingIssues={pendingIssues}
                selectedId={selectedId}
                onSelect={selectTask}
                issuesLoading={issuesLoading}
                creatingIssueId={creatingIssueId}
                createIssueError={createIssueError}
                onCreateFromIssue={createTaskFromIssueItem}
                onUpdateIssue={saveIssueUpdate}
              />
            )}

          {!loading && view === "table" && pendingIssues.length > 0 && (
            <div className="mb-3 flex items-center justify-between gap-2 rounded-[10px] border border-dashed border-[var(--border)] px-3 py-2">
              <p className="text-ui-sm text-[var(--muted)]">
                另有 {pendingIssues.length} 个 issue 尚未创建任务。
              </p>
              <Button
                variant="secondary"
                size="sm"
                className="h-7 gap-1 px-2.5 text-ui-sm"
                onPress={() => setView("board")}
              >
                <LayoutGrid size={13} />
                去创建
              </Button>
            </div>
          )}

          {!loading && tasks.length > 0 && view === "table" && (
            <TaskTable
              tasks={tasks}
              selectedId={selectedId}
              onSelect={selectTask}
            />
          )}
        </div>
      </div>

      {selectedId && (
        <DetailPane
          detail={detail}
          events={events}
          tasks={tasks}
          attachmentsRevision={attachmentsRevision}
          onAttachmentsChanged={refreshAttachments}
          adapters={adapters}
          adaptersLoading={adaptersLoading}
          adaptersError={adaptersError}
          onSaveAgents={saveSelectedAgents}
          onSaveModel={saveSelectedModel}
          dependencies={dependencies}
          dependencyError={dependencyError}
          addingDependency={addingDependency}
          removingDependencyId={removingDependencyId}
          contextItems={contextItems}
          contextError={contextError}
          contextOptions={contextOptions}
          contextOptionsLoading={contextOptionsLoading}
          contextOptionsError={contextOptionsError}
          linkingContext={linkingContext}
          removingContextId={removingContextId}
          issue={selectedIssue}
          issuesLoading={issuesLoading}
          issuesError={issuesError}
          githubError={githubError}
          onUpdateIssue={saveIssueUpdate}
          syncing={syncing}
          publishing={publishing}
          loading={detailLoading}
          busy={busy}
          cancelling={
            detail ? cancellingTasks[detail.task.id] === true : false
          }
          cancelError={
            cancelError && cancelError.taskId === detail?.task.id
              ? cancelError.message
              : null
          }
          resolvingDecisionId={resolvingDecisionId}
          onResolveDecision={(decisionId, resolution) =>
            void resolveTaskDecision(decisionId, resolution)
          }
          onAddDependency={addSelectedDependency}
          onRemoveDependency={(dependsOnTaskId) =>
            void removeSelectedDependency(dependsOnTaskId)
          }
          onLinkContext={linkSelectedContext}
          onUnlinkContext={(itemId) => void unlinkSelectedContext(itemId)}
          onSync={() => void syncGithub()}
          onPublish={() => void publishSelected()}
          onStart={() => void startSelected()}
          onCancel={() => {
            if (detail) void cancelForTask(detail.task.id);
          }}
          onClose={closeDetail}
        />
      )}
    </div>
  );
}

function TaskBoard({
  tasks,
  issues,
  pendingIssues,
  selectedId,
  onSelect,
  issuesLoading,
  creatingIssueId,
  createIssueError,
  onCreateFromIssue,
  onUpdateIssue,
}: {
  tasks: TaskSummary[];
  issues: ProjectIssue[];
  pendingIssues: ProjectIssue[];
  selectedId: string | null;
  onSelect: (taskId: string) => void;
  issuesLoading: boolean;
  creatingIssueId: string | null;
  createIssueError: { issueId: string; message: string } | null;
  onCreateFromIssue: (issue: ProjectIssue) => void;
  onUpdateIssue: (input: UpdateIssueInput) => Promise<string | null>;
}) {
  const issueByTask = new Map<string, ProjectIssue>();
  for (const issue of issues) {
    if (issue.task_id) issueByTask.set(issue.task_id, issue);
  }
  const knownStatuses = new Set(
    BOARD_COLUMNS.flatMap((column) => column.statuses),
  );
  const columns = BOARD_COLUMNS.map((column) => ({
    ...column,
    tasks: tasks.filter((task) => column.statuses.includes(task.status)),
    issues: column.key === "backlog" ? pendingIssues : [],
  }));
  const otherTasks = tasks.filter((task) => !knownStatuses.has(task.status));
  if (otherTasks.length > 0) {
    columns.push({
      key: "other",
      title: "异常/中断",
      statuses: [],
      tasks: otherTasks,
      issues: [],
    });
  }

  return (
    <div className="flex min-w-0 flex-col gap-3">
      {issuesLoading && (
        <div className="rounded-[10px] border border-dashed border-[var(--border)] px-3 py-2">
          <p className="text-ui-sm text-[var(--muted)]">issue 加载中…</p>
        </div>
      )}
      {/* 窄窗口下四列不折行：列容器横向滚动，列自身保持固定宽度。 */}
      <div className="flex min-h-full min-w-0 items-start gap-3 overflow-x-auto pb-1">
        {columns.map((column) => (
          <section
            key={column.key}
            aria-label={column.title}
            className="flex w-[280px] shrink-0 flex-col gap-2"
          >
            <div className="flex h-8 items-center justify-between px-1">
              <span className="text-ui-sm font-medium">{column.title}</span>
              <span className="rounded-full bg-[var(--background-tertiary)] px-1.5 py-0.5 text-label leading-none tabular-nums text-[var(--muted)]">
                {column.tasks.length + column.issues.length}
              </span>
            </div>
            {column.tasks.length === 0 && column.issues.length === 0 ? (
              <p className="rounded-[10px] border border-dashed border-[var(--border)] px-3 py-6 text-center text-ui-sm text-[var(--muted)]">
                暂无任务
              </p>
            ) : (
              <div className="grid gap-2">
                {column.issues.map((issue) => (
                  <IssueCard
                    key={`issue-${issue.id}`}
                    issue={issue}
                    creating={creatingIssueId === issue.id}
                    disabled={
                      creatingIssueId !== null && creatingIssueId !== issue.id
                    }
                    error={
                      createIssueError?.issueId === issue.id
                        ? createIssueError.message
                        : null
                    }
                    onCreate={() => onCreateFromIssue(issue)}
                    onUpdateIssue={onUpdateIssue}
                  />
                ))}
                {column.tasks.map((task) => (
                  <TaskCard
                    key={task.id}
                    task={task}
                    issue={issueByTask.get(task.id) ?? null}
                    selected={selectedId === task.id}
                    onSelect={onSelect}
                  />
                ))}
              </div>
            )}
          </section>
        ))}
      </div>
    </div>
  );
}

function TaskCard({
  task,
  issue,
  selected,
  onSelect,
}: {
  task: TaskSummary;
  issue: ProjectIssue | null;
  selected: boolean;
  onSelect: (taskId: string) => void;
}) {
  const agents = taskAgentInfo(task);
  const developer = agents.developer;
  const reviewer = agents.reviewer ?? agents.developer;

  return (
    <div
      role="button"
      tabIndex={0}
      aria-pressed={selected}
      onClick={() => onSelect(task.id)}
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onSelect(task.id);
        }
      }}
      className={`lift-hover min-w-0 cursor-pointer rounded-[10px] border bg-[var(--background)] p-3 ${
        selected
          ? "border-[var(--accent)]"
          : "border-[var(--border)] hover:border-[var(--border-secondary)]"
      }`}
    >
      <p
        className="line-clamp-2 break-words text-ui font-medium leading-[1.4]"
        title={task.title}
      >
        {task.title}
      </p>
      <div className="mt-2 flex flex-wrap items-center gap-1.5">
        <Chip size="sm" variant="soft" color={statusTone(task.status)}>
          {statusLabel(task.status)}
        </Chip>
        <NextActorTag status={task.status} />
        {task.auto_docs === true && (
          <Chip size="sm" variant="soft" color="accent">
            文档
          </Chip>
        )}
        <span className="text-label text-[var(--muted)]">
          尝试 {task.attempts}
        </span>
        <span className="text-label text-[var(--muted)]">
          验收 {task.criterion_count}
        </span>
      </div>
      <div className="mt-2 flex flex-wrap items-center gap-1.5">
        {issue && issue.github_number != null && (
          <span
            className="rounded bg-[var(--background-tertiary)] px-1.5 py-0.5 font-mono text-label leading-none text-[var(--muted)]"
            title={issue.title}
          >
            #{issue.github_number}
          </span>
        )}
        {issue &&
          (issue.github_state ? (
            <Chip
              size="sm"
              variant="soft"
              color={statusTone(issue.github_state)}
            >
              {statusLabel(issue.github_state)}
            </Chip>
          ) : (
            <Chip size="sm" variant="tertiary" color="default">
              未发布
            </Chip>
          ))}
        {issue?.ghost && (
          <Chip size="sm" variant="soft" color="warning">
            远端已删除
          </Chip>
        )}
        <span
          className="rounded bg-[var(--background-tertiary)] px-1.5 py-0.5 text-label leading-none text-[var(--muted)]"
          title="开发适配器"
        >
          开发 {developer ?? "默认"}
        </span>
        <span
          className="rounded bg-[var(--background-tertiary)] px-1.5 py-0.5 text-label leading-none text-[var(--muted)]"
          title="审查适配器（未设置时跟随开发者）"
        >
          审查 {reviewer ?? "默认"}
        </span>
        {task.model && (
          <span
            className="rounded bg-[var(--background-tertiary)] px-1.5 py-0.5 text-label leading-none text-[var(--muted)]"
            title="任务级模型覆盖"
          >
            模型 {task.model}
          </span>
        )}
      </div>
    </div>
  );
}

function IssueCard({
  issue,
  creating,
  disabled,
  error,
  onCreate,
  onUpdateIssue,
}: {
  issue: ProjectIssue;
  creating: boolean;
  disabled: boolean;
  error: string | null;
  onCreate: () => void;
  onUpdateIssue: (input: UpdateIssueInput) => Promise<string | null>;
}) {
  return (
    <div className="min-w-0 rounded-[10px] border border-dashed border-[var(--border)] bg-[var(--background)] p-3">
      <p
        className="line-clamp-2 break-words text-ui font-medium leading-[1.4]"
        title={issue.title}
      >
        {issue.title}
      </p>
      <div className="mt-2 flex flex-wrap items-center gap-1.5">
        {issue.github_number != null && (
          <span className="rounded bg-[var(--background-tertiary)] px-1.5 py-0.5 font-mono text-label leading-none text-[var(--muted)]">
            GitHub #{issue.github_number}
          </span>
        )}
        {issue.github_state ? (
          <Chip
            size="sm"
            variant="soft"
            color={statusTone(issue.github_state)}
          >
            {statusLabel(issue.github_state)}
          </Chip>
        ) : (
          <Chip size="sm" variant="tertiary" color="default">
            未发布
          </Chip>
        )}
      </div>
      <Button
        variant="secondary"
        size="sm"
        className="mt-2 h-7 w-full px-3 text-ui"
        isDisabled={creating || disabled}
        onPress={onCreate}
      >
        {creating ? "创建中…" : "创建任务"}
      </Button>
      {issue.github_number != null && !issue.ghost && (
        <div className="mt-2 border-t border-[var(--border)] pt-2">
          <IssueEditControls
            issue={issue}
            compact
            onSave={onUpdateIssue}
          />
        </div>
      )}
      {error && (
        <p role="alert" className="mt-1 text-label text-[var(--danger)]">
          {error}
        </p>
      )}
    </div>
  );
}

// 已发布 issue 的远端编辑入口：标题/正文、指派人与关闭/重开。
// 保存成功后由父级刷新列表，不在这里改本地 issues。
function IssueEditControls({
  issue,
  compact = false,
  onSave,
}: {
  issue: ProjectIssue;
  compact?: boolean;
  onSave: (input: UpdateIssueInput) => Promise<string | null>;
}) {
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState(issue.title);
  const [body, setBody] = useState("");
  const [assignee, setAssignee] = useState("");
  const [saving, setSaving] = useState<"fields" | "assignee" | "state" | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);

  const trimmedTitle = title.trim();
  const titleChanged = trimmedTitle !== issue.title;
  // 后端不返回 issue 正文，输入框留空表示只改其它字段，不覆盖远端正文。
  const bodyDirty = body.trim().length > 0;
  const closed = issue.github_state === "closed";
  const buttonClass = compact ? "h-7 px-2 text-label" : "h-8 px-3 text-ui";
  const fieldClass = compact ? "h-7 text-ui-sm" : "h-8 text-ui";
  const errorClass = compact ? "text-label" : "text-ui-sm";

  function openEditor() {
    setTitle(issue.title);
    setBody("");
    setAssignee("");
    setError(null);
    setEditing(true);
  }

  async function saveFields() {
    if (saving !== null) return;
    if (!trimmedTitle) {
      setError("标题不能为空。");
      return;
    }
    if (!titleChanged && !bodyDirty) return;
    setSaving("fields");
    setError(null);
    const input: UpdateIssueInput = { item_id: issue.id };
    if (titleChanged) input.title = trimmedTitle;
    if (bodyDirty) input.body = body;
    const message = await onSave(input);
    setSaving(null);
    if (message) {
      setError(message);
      return;
    }
    setEditing(false);
  }

  async function saveAssignee() {
    const login = assignee.trim();
    // 空输入不提交，避免把远端指派清空。
    if (!login || saving !== null) return;
    setSaving("assignee");
    setError(null);
    const message = await onSave({ item_id: issue.id, assignee: login });
    setSaving(null);
    if (message) {
      setError(message);
      return;
    }
    setAssignee("");
  }

  async function toggleState() {
    if (saving !== null) return;
    setSaving("state");
    setError(null);
    const message = await onSave({
      item_id: issue.id,
      state: closed ? "open" : "closed",
    });
    setSaving(null);
    if (message) setError(message);
  }

  return (
    <div className="grid gap-1">
      <div className="flex flex-wrap items-center gap-1">
        {!editing && (
          <Button
            variant="secondary"
            size="sm"
            className={buttonClass}
            onPress={openEditor}
            isDisabled={saving !== null}
          >
            编辑
          </Button>
        )}
        <Button
          variant="secondary"
          size="sm"
          className={buttonClass}
          onPress={() => void toggleState()}
          isDisabled={saving !== null}
        >
          {saving === "state"
            ? "处理中…"
            : closed
              ? "重开 issue"
              : "关闭 issue"}
        </Button>
      </div>
      {editing && (
        <div className="grid gap-1">
          <label className="grid gap-0.5 text-label text-[var(--muted)]">
            标题
            <Input
              aria-label="issue 标题"
              value={title}
              onChange={(event) => {
                setTitle(event.target.value);
                setError(null);
              }}
              disabled={saving !== null}
              fullWidth
              className={fieldClass}
            />
          </label>
          <label className="grid gap-0.5 text-label text-[var(--muted)]">
            正文
            <TextArea
              aria-label="issue 正文"
              rows={3}
              placeholder="留空则保持远端正文不变"
              value={body}
              onChange={(event) => {
                setBody(event.target.value);
                setError(null);
              }}
              disabled={saving !== null}
              fullWidth
              className={compact ? "text-ui-sm" : "text-ui"}
            />
          </label>
          <div className="flex justify-end gap-1">
            <Button
              variant="ghost"
              size="sm"
              className={buttonClass}
              onPress={() => {
                setEditing(false);
                setError(null);
              }}
              isDisabled={saving !== null}
            >
              取消
            </Button>
            <Button
              variant="secondary"
              size="sm"
              className={buttonClass}
              aria-label="保存标题与正文"
              onPress={() => void saveFields()}
              isDisabled={
                saving !== null || (!titleChanged && !bodyDirty)
              }
            >
              {saving === "fields" ? "保存中…" : "保存"}
            </Button>
          </div>
          <div className="flex items-end gap-1">
            <label className="grid min-w-0 flex-1 gap-0.5 text-label text-[var(--muted)]">
              指派人
              <Input
                aria-label="issue 指派人"
                placeholder="GitHub 登录名"
                value={assignee}
                onChange={(event) => {
                  setAssignee(event.target.value);
                  setError(null);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    void saveAssignee();
                  }
                }}
                disabled={saving !== null}
                fullWidth
                className={fieldClass}
              />
            </label>
            <Button
              variant="secondary"
              size="sm"
              className={`${buttonClass} shrink-0`}
              aria-label="保存指派人"
              onPress={() => void saveAssignee()}
              isDisabled={saving !== null || !assignee.trim()}
            >
              {saving === "assignee" ? "保存中…" : "保存"}
            </Button>
          </div>
        </div>
      )}
      {error && (
        <p role="alert" className={`text-[var(--danger)] ${errorClass}`}>
          {error}
        </p>
      )}
    </div>
  );
}

function TaskTable({
  tasks,
  selectedId,
  onSelect,
}: {
  tasks: TaskSummary[];
  selectedId: string | null;
  onSelect: (taskId: string) => void;
}) {
  const columns =
    "grid-cols-[minmax(0,1fr)_78px_56px_48px_48px_84px]";

  return (
    <div className="overflow-hidden rounded-[10px] border border-[var(--border)]">
      <div
        className={`text-label grid ${columns} gap-2 border-b border-[var(--border)] bg-[var(--background-secondary)] px-3 py-2 font-medium uppercase tracking-[0.06em] text-[var(--muted)]`}
      >
        <span>名称</span>
        <span>状态</span>
        <span>下一步</span>
        <span>尝试</span>
        <span>验收</span>
        <span>创建</span>
      </div>
      {tasks.map((task) => (
        <button
          key={task.id}
          type="button"
          onClick={() => onSelect(task.id)}
          className={`text-ui anim-fast grid ${columns} h-10 w-full items-center gap-2 border-b border-[var(--border)] px-3 text-left last:border-b-0 hover:bg-[var(--background-secondary)] ${
            selectedId === task.id ? "bg-[var(--background-secondary)]" : ""
          }`}
        >
          <span className="min-w-0 truncate">{task.title}</span>
          <span>
            <Chip
              size="sm"
              variant="soft"
              color={statusTone(task.status)}
            >
              {statusLabel(task.status)}
            </Chip>
          </span>
          <span>
            <NextActorTag status={task.status} />
          </span>
          <span className="text-[var(--muted)]">{task.attempts}</span>
          <span className="text-[var(--muted)]">{task.criterion_count}</span>
          <span className="text-ui-sm text-[var(--muted)]">
            {formatDate(task.created_at)}
          </span>
        </button>
      ))}
    </div>
  );
}

function CreateTaskForm({
  projectId,
  onCancel,
  onCreated,
}: {
  projectId: string;
  onCancel: () => void;
  onCreated: (detail: TaskDetail) => Promise<void> | void;
}) {
  const [title, setTitle] = useState("");
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit() {
    const trimmedTitle = title.trim();
    const trimmedPrompt = prompt.trim();
    if (!trimmedTitle || !trimmedPrompt) return;
    setBusy(true);
    setError(null);
    try {
      const created = await createTask({
        project_id: projectId,
        title: trimmedTitle,
        prompt: trimmedPrompt,
      });
      setTitle("");
      setPrompt("");
      await onCreated(created);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card className="mb-3 max-w-[720px] rounded-[10px] p-3.5">
      <Card.Header>
        <Card.Title className="text-[15px] font-medium leading-[1.4]">
          新建任务
        </Card.Title>
        <Card.Description className="text-ui-sm">
          描述目标与约束，验收标准由规划 Agent 生成。
        </Card.Description>
      </Card.Header>
      <Card.Content className="grid gap-2">
        <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
          标题
          <Input
            aria-label="任务标题"
            placeholder="例如：为登录页补充失败态测试"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            className="h-8 text-ui"
          />
        </label>
        <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
          意图
          <TextArea
            aria-label="任务意图"
            rows={3}
            placeholder="说明要做什么、为什么，以及需要注意的约束。"
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            className="text-ui"
          />
        </label>
        {error && (
          <p role="alert" className="text-ui-sm text-[var(--danger)]">
            {error}
          </p>
        )}
      </Card.Content>
      <Card.Footer className="flex justify-end gap-1.5">
        <Button
          variant="ghost"
          size="sm"
          className="h-8 px-3 text-ui"
          onPress={onCancel}
          isDisabled={busy}
        >
          取消
        </Button>
        <Button
          variant="primary"
          size="sm"
          className="h-8 px-3 text-ui"
          onPress={submit}
          isDisabled={busy || !title.trim() || !prompt.trim()}
        >
          {busy ? "创建中…" : "创建"}
        </Button>
      </Card.Footer>
    </Card>
  );
}

function PlanPanel({
  projectId,
  onCancel,
  onCreated,
}: {
  projectId: string;
  onCancel: () => void;
  onCreated: (created: TaskDetail[]) => Promise<void> | void;
}) {
  const [idea, setIdea] = useState("");
  const [draft, setDraft] = useState<PlanDraft | null>(null);
  const [generating, setGenerating] = useState(false);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const busy = generating || creating;

  async function generate() {
    const trimmed = idea.trim();
    if (!trimmed || busy) return;
    setGenerating(true);
    setError(null);
    try {
      const next = await planProject({ project_id: projectId, idea: trimmed });
      setDraft(next);
      if (next.issues.length === 0) {
        setError("规划结果为空，请调整想法后重试。");
      }
    } catch (e) {
      setDraft(null);
      setError(errorText(e));
    } finally {
      setGenerating(false);
    }
  }

  async function create() {
    if (!draft || draft.issues.length === 0 || busy) return;
    setCreating(true);
    setError(null);
    try {
      const created = await createTasksFromPlan({
        project_id: projectId,
        draft,
      });
      await onCreated(created);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setCreating(false);
    }
  }

  const issueCount = draft?.issues.length ?? 0;

  return (
    <Card className="mb-3 max-w-[720px] rounded-[10px] p-3.5">
      <Card.Header>
        <Card.Title className="text-[15px] font-medium leading-[1.4]">
          规划任务
        </Card.Title>
        <Card.Description className="text-ui-sm">
          描述一个想法，规划 Agent 会拆成带验收标准与依赖的任务。
        </Card.Description>
      </Card.Header>
      <Card.Content className="grid gap-2">
        <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
          想法
          <TextArea
            aria-label="规划想法"
            rows={3}
            placeholder="例如：让项目支持按计划批量拆分任务并自动建立依赖。"
            className="text-ui"
            value={idea}
            onChange={(e) => {
              setIdea(e.target.value);
              setDraft(null);
              setError(null);
            }}
            disabled={busy}
          />
        </label>
        <div className="flex justify-end gap-1.5">
          <Button
            variant="ghost"
            size="sm"
            className="h-8 px-3 text-ui"
            onPress={onCancel}
            isDisabled={busy}
          >
            取消
          </Button>
          <Button
            variant="secondary"
            size="sm"
            className="h-8 px-3 text-ui"
            onPress={() => void generate()}
            isDisabled={busy || !idea.trim()}
          >
            {generating ? "生成中…" : "生成计划"}
          </Button>
        </div>
        {error && (
          <p role="alert" className="text-ui-sm text-[var(--danger)]">
            {error}
          </p>
        )}
        {draft && draft.issues.length > 0 && (
          <div className="grid gap-2">
            <div className="max-h-72 overflow-auto rounded-[10px] border border-[var(--border)]">
              {draft.issues.map((issue, index) => (
                <div
                  key={`${index}-${issue.title}`}
                  className="grid gap-1 border-b border-[var(--border)] px-2 py-1.5 last:border-b-0"
                >
                  <div className="flex items-start gap-1.5">
                    <span className="shrink-0 font-mono text-label text-[var(--muted)]">
                      {index + 1}.
                    </span>
                    <p
                      className="min-w-0 text-ui-sm font-medium"
                      title={issue.title}
                    >
                      {issue.title}
                    </p>
                  </div>
                  {issue.prompt && (
                    <p
                      className="line-clamp-2 text-ui-sm text-[var(--muted)]"
                      title={issue.prompt}
                    >
                      {issue.prompt}
                    </p>
                  )}
                  {issue.acceptance.length > 0 ? (
                    <details>
                      <summary className="cursor-pointer text-ui-sm text-[var(--muted)]">
                        验收 {issue.acceptance.length} 项
                      </summary>
                      <ul className="mt-1 grid gap-1">
                        {issue.acceptance.map((criterion, criterionIndex) => (
                          <li
                            key={`${criterion.id}-${criterionIndex}`}
                            className="grid gap-0.5 rounded border border-[var(--border)] px-1.5 py-1"
                          >
                            <code
                              className="min-w-0 truncate font-mono text-label"
                              title={criterion.cmd}
                            >
                              {criterion.cmd}
                            </code>
                            <span className="text-label text-[var(--muted)]">
                              期望退出码 {criterion.expect_exit}
                              {criterion.expect_match
                                ? ` · 匹配 ${criterion.expect_match}`
                                : ""}
                            </span>
                          </li>
                        ))}
                      </ul>
                    </details>
                  ) : (
                    <span className="text-ui-sm text-[var(--muted)]">
                      验收 0 项
                    </span>
                  )}
                  {issue.depends_on.length > 0 && (
                    <div className="flex flex-wrap items-center gap-1">
                      <span className="text-ui-sm text-[var(--muted)]">
                        依赖
                      </span>
                      {issue.depends_on.map((dep, depIndex) => (
                        <Chip
                          key={`${dep}-${depIndex}`}
                          size="sm"
                          variant="tertiary"
                          color="default"
                        >
                          {draft.issues[dep]?.title ?? `#${dep + 1}`}
                        </Chip>
                      ))}
                    </div>
                  )}
                </div>
              ))}
            </div>
            <div className="flex items-center justify-end">
              <Button
                variant="primary"
                size="sm"
                className="h-8 px-3 text-ui"
                onPress={() => void create()}
                isDisabled={busy || issueCount === 0}
              >
                {creating ? "创建中…" : `创建 ${issueCount} 个任务`}
              </Button>
            </div>
          </div>
        )}
      </Card.Content>
    </Card>
  );
}

function DetailPane({
  detail,
  events,
  tasks,
  attachmentsRevision,
  onAttachmentsChanged,
  adapters,
  adaptersLoading,
  adaptersError,
  onSaveAgents,
  onSaveModel,
  dependencies,
  dependencyError,
  addingDependency,
  removingDependencyId,
  contextItems,
  contextError,
  contextOptions,
  contextOptionsLoading,
  contextOptionsError,
  linkingContext,
  removingContextId,
  issue,
  issuesLoading,
  issuesError,
  githubError,
  onUpdateIssue,
  syncing,
  publishing,
  loading,
  busy,
  cancelling,
  cancelError,
  resolvingDecisionId,
  onResolveDecision,
  onAddDependency,
  onRemoveDependency,
  onLinkContext,
  onUnlinkContext,
  onSync,
  onPublish,
  onStart,
  onCancel,
  onClose,
}: {
  detail: TaskDetail | null;
  events: EventRow[];
  tasks: TaskSummary[];
  attachmentsRevision: number;
  onAttachmentsChanged: () => void;
  adapters: AgentManifest[];
  adaptersLoading: boolean;
  adaptersError: string | null;
  onSaveAgents: (
    developer: string | null,
    reviewer: string | null,
  ) => Promise<string | null>;
  onSaveModel: (model: string | null) => Promise<string | null>;
  dependencies: TaskDependencies | null;
  dependencyError: string | null;
  addingDependency: boolean;
  removingDependencyId: string | null;
  contextItems: ContextItem[] | null;
  contextError: string | null;
  contextOptions: ContextOption[];
  contextOptionsLoading: boolean;
  contextOptionsError: string | null;
  linkingContext: boolean;
  removingContextId: string | null;
  issue: ProjectIssue | null;
  issuesLoading: boolean;
  issuesError: string | null;
  githubError: string | null;
  onUpdateIssue: (input: UpdateIssueInput) => Promise<string | null>;
  syncing: boolean;
  publishing: boolean;
  loading: boolean;
  busy: boolean;
  cancelling: boolean;
  cancelError: string | null;
  resolvingDecisionId: string | null;
  onResolveDecision: (decisionId: string, resolution: "allow" | "deny") => void;
  onAddDependency: (dependsOnTaskId: string) => Promise<boolean>;
  onRemoveDependency: (dependsOnTaskId: string) => void;
  onLinkContext: (itemId: string) => Promise<boolean>;
  onUnlinkContext: (itemId: string) => void;
  onSync: () => void;
  onPublish: () => void;
  onStart: () => void;
  onCancel: () => void;
  onClose: () => void;
}) {
  const [dependsOnKey, setDependsOnKey] = useState<string | null>(null);
  const [contextKey, setContextKey] = useState<string | null>(null);
  const [prLinkError, setPrLinkError] = useState<string | null>(null);
  const detailTaskId = detail?.task.id ?? null;
  const savedDeveloper = detail ? taskAgentInfo(detail.task).developer : null;
  const savedReviewer = detail ? taskAgentInfo(detail.task).reviewer : null;
  const savedModel = detail ? (detail.task.model ?? null) : null;
  const [developerKey, setDeveloperKey] = useState<string | null>(
    savedDeveloper,
  );
  const [reviewerKey, setReviewerKey] = useState<string | null>(
    savedReviewer ?? savedDeveloper,
  );
  const [reviewerFollows, setReviewerFollows] = useState(
    savedReviewer === null,
  );
  const [agentsSaving, setAgentsSaving] = useState(false);
  const [agentsSaveError, setAgentsSaveError] = useState<string | null>(null);
  const [modelInput, setModelInput] = useState(savedModel ?? "");
  const [modelSaving, setModelSaving] = useState(false);
  const [modelSaveError, setModelSaveError] = useState<string | null>(null);
  useEffect(() => {
    setDependsOnKey(null);
    setContextKey(null);
    setPrLinkError(null);
    setDeveloperKey(savedDeveloper);
    setReviewerKey(savedReviewer ?? savedDeveloper);
    setReviewerFollows(savedReviewer === null);
    setAgentsSaveError(null);
    setModelInput(savedModel ?? "");
    setModelSaveError(null);
  }, [detailTaskId, savedDeveloper, savedReviewer, savedModel]);

  const review = detail?.review ?? null;
  const riskFlags = review ? textList(review.risk_flags) : [];
  const testDiffs = review ? textList(review.test_diff) : [];
  const openDecisions =
    detail?.decisions.filter((decision) => decision.status === "open") ?? [];
  const blockers = dependencies?.blockers ?? [];
  const blocks = dependencies?.blocks ?? [];
  const linkedIds = new Set([...blockers, ...blocks].map((task) => task.id));
  const dependencyOptions = detail
    ? tasks.filter(
        (task) => task.id !== detailTaskId && !linkedIds.has(task.id),
      )
    : [];
  const linkedContextKeys = new Set(
    (contextItems ?? []).map((item) => `${item.kind}:${item.id}`),
  );
  const availableContextOptions = contextOptions.filter(
    (option) => !linkedContextKeys.has(`${option.kind}:${option.id}`),
  );
  const unmetBlockerCount = blockers.filter(
    (task) => task.status !== "merged",
  ).length;
  const status = detail?.task.status;
  const canCancel = status === "queued" || status === "running";
  const startDisabled =
    busy ||
    cancelling ||
    !detail ||
    detail.task.status === "running" ||
    detail.task.status === "verifying" ||
    detail.task.status === "merged";
  const agentsDirty =
    detail !== null &&
    (developerKey !== savedDeveloper ||
      (reviewerFollows
        ? savedReviewer !== null
        : reviewerKey !== savedReviewer));
  const knownAdapterIds = new Set(adapters.map((adapter) => adapter.id));
  const adapterFallbackLabel = adaptersLoading
    ? "加载中"
    : adaptersError
      ? "未校验"
      : "未注册";
  const unregisteredAdapterIds = Array.from(
    new Set(
      [developerKey, reviewerKey].filter(
        (id): id is string => id !== null && !knownAdapterIds.has(id),
      ),
    ),
  );
  // 审查下拉优先只列声明了 reviewer 角色的适配器；一个都没有时退回完整列表。
  const reviewerCapable = adapters.filter((adapter) =>
    adapterRoles(adapter).includes("reviewer"),
  );
  const reviewerCandidates =
    reviewerCapable.length > 0 ? reviewerCapable : adapters;
  const reviewerCandidateIds = new Set(
    reviewerCandidates.map((adapter) => adapter.id),
  );
  // 已保存/当前选择的适配器即使不支持审查也要可选，便于看清并改回。
  const retainedReviewerAdapters = adapters.filter(
    (adapter) =>
      !reviewerCandidateIds.has(adapter.id) &&
      ((!reviewerFollows && adapter.id === reviewerKey) ||
        adapter.id === savedReviewer),
  );
  const reviewerAdapterOptions = [
    ...reviewerCandidates,
    ...retainedReviewerAdapters,
  ];
  const normalizedModel = modelInput.trim() === "" ? null : modelInput.trim();
  const normalizedSavedModel =
    savedModel && savedModel.trim() ? savedModel.trim() : null;
  const modelDirty =
    detail !== null && normalizedModel !== normalizedSavedModel;

  async function submitAgents() {
    if (!detail || agentsSaving || !agentsDirty) return;
    setAgentsSaving(true);
    setAgentsSaveError(null);
    const failure = await onSaveAgents(
      developerKey,
      reviewerFollows ? null : reviewerKey,
    );
    if (failure) setAgentsSaveError(failure);
    setAgentsSaving(false);
  }

  async function submitModel() {
    if (!detail || modelSaving || !modelDirty) return;
    setModelSaving(true);
    setModelSaveError(null);
    const failure = await onSaveModel(normalizedModel);
    if (failure) setModelSaveError(failure);
    setModelSaving(false);
  }

  async function submitDependency() {
    if (!dependsOnKey || addingDependency) return;
    const added = await onAddDependency(dependsOnKey);
    if (added) setDependsOnKey(null);
  }

  async function submitContext() {
    if (!contextKey || linkingContext) return;
    const linked = await onLinkContext(contextKey);
    if (linked) setContextKey(null);
  }

  return (
    <aside className="absolute inset-y-0 right-0 z-10 flex h-full w-[400px] max-w-full shrink-0 flex-col overflow-hidden rounded-[10px] border border-[var(--border)] bg-[var(--background)] shadow-[0_8px_24px_rgba(0,0,0,0.12)] xl:static xl:w-[380px] xl:shadow-none">
      <header className="shrink-0 border-b border-[var(--border)] px-3.5 py-3">
        <div className="flex items-start gap-2">
          <div className="min-w-0 flex-1">
            <p className="truncate text-ui font-medium">
              {detail ? detail.task.title : "任务详情"}
            </p>
            {detail && (
              <div className="mt-1 flex items-center gap-1.5 text-ui-sm text-[var(--muted)]">
                <Chip
                  size="sm"
                  variant="soft"
                  color={statusTone(detail.task.status)}
                >
                  {statusLabel(detail.task.status)}
                </Chip>
                <span>尝试 {detail.task.attempts}</span>
                <span>{formatDate(detail.task.created_at)}</span>
              </div>
            )}
          </div>
          {detail && canCancel && (
            <Button
              variant="danger-soft"
              size="sm"
              className="h-7 shrink-0 gap-1 px-2 text-ui"
              onPress={onCancel}
              isDisabled={cancelling}
            >
              <CircleStop size={14} />
              {cancelling ? "取消中…" : "取消"}
            </Button>
          )}
          {detail && (
            <Button
              variant="primary"
              size="sm"
              className="h-8 shrink-0 gap-1 px-3 text-ui"
              onPress={onStart}
              isDisabled={startDisabled}
            >
              <Play size={14} />
              {busy ? "启动中…" : "开始执行"}
            </Button>
          )}
          <Button
            isIconOnly
            variant="ghost"
            size="sm"
            className="h-7 w-7 shrink-0 rounded-lg"
            aria-label="关闭详情"
            onPress={onClose}
          >
            <X size={14} />
          </Button>
        </div>
        {detail && (
          <div className="mt-1.5 grid gap-1">
            <div className="flex items-end gap-1.5">
              <div className="grid min-w-0 flex-1 gap-0.5">
                <span className="text-label text-[var(--muted)]">开发</span>
                <Select
                  aria-label="开发适配器"
                  className="w-full min-w-0"
                  placeholder={adaptersLoading ? "加载中…" : "默认适配器"}
                  selectedKey={developerKey ?? ADAPTER_DEFAULT_KEY}
                  onSelectionChange={(key) => {
                    const next =
                      key == null || String(key) === ADAPTER_DEFAULT_KEY
                        ? null
                        : String(key);
                    if (reviewerFollows) setReviewerKey(next);
                    setDeveloperKey(next);
                    setAgentsSaveError(null);
                  }}
                  isDisabled={agentsSaving}
                >
                  <Select.Trigger className="h-7 text-ui">
                    <Select.Value />
                    <Select.Indicator />
                  </Select.Trigger>
                  <Select.Popover>
                    <ListBox>
                      <ListBox.Item
                        id={ADAPTER_DEFAULT_KEY}
                        textValue="默认适配器"
                      >
                        <span className="flex min-w-0 flex-1 items-center gap-2">
                          <span className="min-w-0 flex-1 truncate">
                            默认适配器
                          </span>
                          <ListBox.ItemIndicator />
                        </span>
                      </ListBox.Item>
                      {adapters.map((adapter) => (
                        <ListBox.Item
                          key={adapter.id}
                          id={adapter.id}
                          textValue={`${adapter.name} (${adapter.id})`}
                        >
                          <span className="flex min-w-0 flex-1 items-center gap-2">
                            <span className="min-w-0 flex-1 truncate">
                              {adapter.name}
                            </span>
                            <AgentRoleTags adapter={adapter} />
                            <span className="shrink-0 font-mono text-label text-[var(--muted)]">
                              {adapter.id}
                            </span>
                            <ListBox.ItemIndicator />
                          </span>
                        </ListBox.Item>
                      ))}
                      {unregisteredAdapterIds.map((id) => (
                        <ListBox.Item
                          key={`missing-${id}`}
                          id={id}
                          textValue={`${id}（${adapterFallbackLabel}）`}
                        >
                          <span className="flex min-w-0 flex-1 items-center gap-2">
                            <span className="min-w-0 flex-1 truncate font-mono">
                              {id}
                            </span>
                            <Chip
                              size="sm"
                              variant="soft"
                              color={
                                adaptersLoading || adaptersError
                                  ? "default"
                                  : "warning"
                              }
                              className="shrink-0"
                            >
                              {adapterFallbackLabel}
                            </Chip>
                            <ListBox.ItemIndicator />
                          </span>
                        </ListBox.Item>
                      ))}
                    </ListBox>
                  </Select.Popover>
                </Select>
              </div>
              <div className="grid min-w-0 flex-1 gap-0.5">
                <span className="text-label text-[var(--muted)]">审查</span>
                <Select
                  aria-label="审查适配器"
                  className="w-full min-w-0"
                  placeholder={adaptersLoading ? "加载中…" : "跟随开发者"}
                  selectedKey={
                    reviewerFollows ? ADAPTER_FOLLOW_KEY : reviewerKey
                  }
                  onSelectionChange={(key) => {
                    if (key == null) return;
                    const raw = String(key);
                    const following = raw === ADAPTER_FOLLOW_KEY;
                    setReviewerFollows(following);
                    setReviewerKey(following ? developerKey : raw);
                    setAgentsSaveError(null);
                  }}
                  isDisabled={agentsSaving}
                >
                  <Select.Trigger className="h-7 text-ui">
                    <Select.Value />
                    <Select.Indicator />
                  </Select.Trigger>
                  <Select.Popover>
                    <ListBox>
                      <ListBox.Item
                        id={ADAPTER_FOLLOW_KEY}
                        textValue={`跟随开发者（${adapterLabel(
                          adapters,
                          developerKey,
                        )}）`}
                      >
                        <span className="flex min-w-0 flex-1 items-center gap-2">
                          <span className="min-w-0 flex-1 truncate">
                            跟随开发者
                          </span>
                          <span className="shrink-0 truncate text-label text-[var(--muted)]">
                            {adapterLabel(adapters, developerKey)}
                          </span>
                          <ListBox.ItemIndicator />
                        </span>
                      </ListBox.Item>
                      {reviewerAdapterOptions.map((adapter) => (
                        <ListBox.Item
                          key={adapter.id}
                          id={adapter.id}
                          textValue={`${adapter.name} (${adapter.id})`}
                        >
                          <span className="flex min-w-0 flex-1 items-center gap-2">
                            <span className="min-w-0 flex-1 truncate">
                              {adapter.name}
                            </span>
                            <AgentRoleTags adapter={adapter} />
                            <span className="shrink-0 font-mono text-label text-[var(--muted)]">
                              {adapter.id}
                            </span>
                            {!reviewerCandidateIds.has(adapter.id) && (
                              <Chip
                                size="sm"
                                variant="soft"
                                color="warning"
                                className="shrink-0"
                              >
                                不支持审查
                              </Chip>
                            )}
                            <ListBox.ItemIndicator />
                          </span>
                        </ListBox.Item>
                      ))}
                      {unregisteredAdapterIds.map((id) => (
                        <ListBox.Item
                          key={`missing-${id}`}
                          id={id}
                          textValue={`${id}（${adapterFallbackLabel}）`}
                        >
                          <span className="flex min-w-0 flex-1 items-center gap-2">
                            <span className="min-w-0 flex-1 truncate font-mono">
                              {id}
                            </span>
                            <Chip
                              size="sm"
                              variant="soft"
                              color={
                                adaptersLoading || adaptersError
                                  ? "default"
                                  : "warning"
                              }
                              className="shrink-0"
                            >
                              {adapterFallbackLabel}
                            </Chip>
                            <ListBox.ItemIndicator />
                          </span>
                        </ListBox.Item>
                      ))}
                    </ListBox>
                  </Select.Popover>
                </Select>
              </div>
              <Button
                variant="secondary"
                size="sm"
                className="h-7 shrink-0 px-2.5 text-ui"
                onPress={() => void submitAgents()}
                isDisabled={!agentsDirty || agentsSaving}
              >
                {agentsSaving ? "保存中…" : "保存"}
              </Button>
            </div>
            <div className="flex items-end gap-1.5">
              <div className="grid min-w-0 flex-1 gap-0.5">
                <span className="text-label text-[var(--muted)]">模型</span>
                <Input
                  aria-label="任务模型"
                  placeholder="默认模型"
                  value={modelInput}
                  onChange={(event) => {
                    setModelInput(event.target.value);
                    setModelSaveError(null);
                  }}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      event.preventDefault();
                      void submitModel();
                    }
                  }}
                  disabled={modelSaving}
                  fullWidth
                  className="h-7 text-ui"
                />
              </div>
              <Button
                variant="secondary"
                size="sm"
                className="h-7 shrink-0 px-2.5 text-ui"
                aria-label="保存模型"
                onPress={() => void submitModel()}
                isDisabled={!modelDirty || modelSaving}
              >
                {modelSaving ? "保存中…" : "保存"}
              </Button>
            </div>
            {adaptersError && (
              <p role="alert" className="text-ui-sm text-[var(--danger)]">
                适配器列表加载失败：{adaptersError}
              </p>
            )}
            {agentsSaveError && (
              <p role="alert" className="text-ui-sm text-[var(--danger)]">
                {agentsSaveError}
              </p>
            )}
            {modelSaveError && (
              <p role="alert" className="text-ui-sm text-[var(--danger)]">
                {modelSaveError}
              </p>
            )}
          </div>
        )}
      </header>

      {cancelError && (
        <p
          role="alert"
          className="shrink-0 border-b border-[var(--border)] px-3 py-1.5 text-ui-sm text-[var(--danger)]"
        >
          {cancelError}
        </p>
      )}

      {!detail ? (
        <p className="p-3 text-ui-sm text-[var(--muted)]">
          {loading ? "加载中…" : "未找到任务详情。"}
        </p>
      ) : (
        <div className="min-h-0 flex-1 overflow-auto px-3.5 py-3.5">
          <div className="grid gap-4">
            <div>
              <p className="text-label font-medium uppercase tracking-wide text-[var(--muted)]">
                意图
              </p>
              <p className="mt-1 whitespace-pre-wrap break-words text-ui-sm">
                {detail.task.prompt}
              </p>
            </div>

            <Section title="GitHub" icon={<GitPullRequest size={14} />}>
              <div className="flex flex-wrap items-center gap-1.5">
                <Button
                  variant="secondary"
                  size="sm"
                  className="h-7 gap-1 px-2.5 text-ui"
                  onPress={onSync}
                  isDisabled={syncing}
                >
                  <RefreshCw
                    size={14}
                    className={syncing ? "animate-spin" : undefined}
                  />
                  {syncing ? "同步中…" : "同步"}
                </Button>
                <Button
                  variant="secondary"
                  size="sm"
                  className="h-7 gap-1 px-2.5 text-ui"
                  onPress={onPublish}
                  isDisabled={
                    !issue || publishing || issue.github_number != null
                  }
                >
                  <Upload size={14} />
                  {publishing ? "发布中…" : "发布到 GitHub"}
                </Button>
              </div>
              {(githubError || issuesError) && (
                <p role="alert" className="text-ui-sm text-[var(--danger)]">
                  {githubError ?? issuesError}
                </p>
              )}
              {issuesLoading && !issue ? (
                <Muted>正在加载 GitHub 关联…</Muted>
              ) : issue && issue.github_number != null ? (
                <div className="grid gap-1 rounded-lg border border-[var(--border)] px-2.5 py-2">
                  <div className="flex flex-wrap items-center gap-1.5">
                    <span className="font-mono text-ui-sm">
                      #{issue.github_number}
                    </span>
                    <Chip
                      size="sm"
                      variant="soft"
                      color={statusTone(issue.github_state ?? "")}
                    >
                      {issue.github_state
                        ? statusLabel(issue.github_state)
                        : "未知状态"}
                    </Chip>
                    {issue.ghost && (
                      <Chip size="sm" variant="soft" color="warning">
                        远端已删除
                      </Chip>
                    )}
                  </div>
                  <p
                    className="truncate text-ui-sm text-[var(--muted)]"
                    title={issue.title}
                  >
                    {issue.title}
                  </p>
                  {!issue.ghost && (
                    <IssueEditControls
                      key={issue.id}
                      issue={issue}
                      onSave={onUpdateIssue}
                    />
                  )}
                </div>
              ) : issue ? (
                <Muted>尚未发布到 GitHub。</Muted>
              ) : (
                <Muted>未找到关联的 Issue 条目。</Muted>
              )}
            </Section>

            <Section
              title="依赖"
              icon={<Link2 size={14} />}
              count={blockers.length + blocks.length}
            >
              {dependencyError && (
                <p role="alert" className="text-ui-sm text-[var(--danger)]">
                  {dependencyError}
                </p>
              )}
              {dependencies === null ? (
                <Muted>
                  {dependencyError ? "无法加载依赖。" : "加载中…"}
                </Muted>
              ) : (
                <div className="grid gap-2">
                  <div className="grid gap-1">
                    <p className="text-label text-[var(--muted)]">
                      被阻塞（前置）
                    </p>
                    {blockers.length === 0 ? (
                      <Muted>没有前置任务。</Muted>
                    ) : (
                      <div className="overflow-hidden rounded-[10px] border border-[var(--border)]">
                        {blockers.map((task) => (
                          <DependencyRow
                            key={task.id}
                            task={task}
                            removing={removingDependencyId === task.id}
                            onRemove={() => onRemoveDependency(task.id)}
                          />
                        ))}
                      </div>
                    )}
                  </div>
                  <div className="grid gap-1">
                    <p className="text-label text-[var(--muted)]">
                      阻塞（下游）
                    </p>
                    {blocks.length === 0 ? (
                      <Muted>没有下游任务。</Muted>
                    ) : (
                      <div className="overflow-hidden rounded-[10px] border border-[var(--border)]">
                        {blocks.map((task) => (
                          <DependencyRow key={task.id} task={task} />
                        ))}
                      </div>
                    )}
                  </div>
                  <div className="flex items-center gap-1.5">
                    <Select
                      aria-label="选择前置任务"
                      className="min-w-0 flex-1"
                      placeholder={
                        dependencyOptions.length === 0
                          ? "没有可添加的任务"
                          : "选择要依赖的任务…"
                      }
                      selectedKey={dependsOnKey}
                      onSelectionChange={(key) =>
                        setDependsOnKey(key == null ? null : String(key))
                      }
                      isDisabled={
                        addingDependency || dependencyOptions.length === 0
                      }
                    >
                      <Select.Trigger className="h-7 text-ui">
                        <Select.Value />
                        <Select.Indicator />
                      </Select.Trigger>
                      <Select.Popover>
                        <ListBox>
                          {dependencyOptions.map((task) => (
                            <ListBox.Item
                              key={task.id}
                              id={task.id}
                              textValue={task.title}
                            >
                              <span className="flex min-w-0 flex-1 items-center gap-2">
                                <span className="min-w-0 flex-1 truncate">
                                  {task.title}
                                </span>
                                <Chip
                                  size="sm"
                                  variant="soft"
                                  color={statusTone(task.status)}
                                  className="shrink-0"
                                >
                                  {statusLabel(task.status)}
                                </Chip>
                                <ListBox.ItemIndicator />
                              </span>
                            </ListBox.Item>
                          ))}
                        </ListBox>
                      </Select.Popover>
                    </Select>
                    <Button
                      variant="secondary"
                      size="sm"
                      className="h-7 shrink-0 px-2.5 text-ui"
                      onPress={() => void submitDependency()}
                      isDisabled={!dependsOnKey || addingDependency}
                    >
                      {addingDependency ? "添加中…" : "添加"}
                    </Button>
                  </div>
                  {unmetBlockerCount > 0 && (
                    <p className="flex items-start gap-1.5 text-ui-sm text-[var(--warning)]">
                      <AlertTriangle size={14} className="mt-0.5 shrink-0" />
                      <span>
                        {status === "queued"
                          ? `${unmetBlockerCount} 个前置任务尚未合并；调度器会在依赖合并后再执行。`
                          : `${unmetBlockerCount} 个前置任务尚未合并；开始执行会先排队，待依赖合并后自动运行。`}
                      </span>
                    </p>
                  )}
                </div>
              )}
            </Section>

            <Section
              title="上下文"
              icon={<FileText size={14} />}
              count={contextItems?.length ?? 0}
            >
              {contextError && (
                <p role="alert" className="text-ui-sm text-[var(--danger)]">
                  {contextError}
                </p>
              )}
              {contextItems === null ? (
                <Muted>
                  {contextError ? "无法加载上下文。" : "加载中…"}
                </Muted>
              ) : (
                <div className="grid gap-2">
                  {contextItems.length === 0 ? (
                    <Muted>没有关联的笔记或文档。</Muted>
                  ) : (
                    <div className="overflow-hidden rounded-[10px] border border-[var(--border)]">
                      {contextItems.map((item) => (
                        <div
                          key={`${item.kind}:${item.id}`}
                          className="flex items-center gap-2 border-b border-[var(--border)] px-2 py-1.5 last:border-b-0"
                        >
                          <Chip
                            size="sm"
                            variant="tertiary"
                            color="default"
                            className="shrink-0"
                          >
                            {item.kind === "doc"
                              ? "文档"
                              : item.kind === "note"
                                ? "笔记"
                                : item.kind}
                          </Chip>
                          <span
                            className="min-w-0 flex-1 truncate"
                            title={item.title}
                          >
                            {item.title}
                          </span>
                          <Button
                            isIconOnly
                            variant="ghost"
                            size="sm"
                            className="h-6 w-6 shrink-0 text-[var(--muted)]"
                            aria-label={`取消关联：${item.title}`}
                            onPress={() => onUnlinkContext(item.id)}
                            isDisabled={removingContextId !== null}
                          >
                            <X size={13} />
                          </Button>
                        </div>
                      ))}
                    </div>
                  )}
                  <div className="flex items-center gap-1.5">
                    <Select
                      aria-label="选择要关联的笔记或文档"
                      className="min-w-0 flex-1"
                      placeholder={
                        contextOptionsLoading
                          ? "加载可选内容…"
                          : availableContextOptions.length === 0
                            ? "没有可关联的内容"
                            : "选择笔记或文档…"
                      }
                      selectedKey={contextKey}
                      onSelectionChange={(key) =>
                        setContextKey(key == null ? null : String(key))
                      }
                      isDisabled={
                        linkingContext ||
                        contextOptionsLoading ||
                        availableContextOptions.length === 0
                      }
                    >
                      <Select.Trigger className="h-7 text-ui">
                        <Select.Value />
                        <Select.Indicator />
                      </Select.Trigger>
                      <Select.Popover>
                        <ListBox>
                          {availableContextOptions.map((option) => (
                            <ListBox.Item
                              key={`${option.kind}:${option.id}`}
                              id={option.id}
                              textValue={`${
                                option.kind === "doc" ? "文档" : "笔记"
                              } · ${option.title}`}
                            >
                              <span className="flex min-w-0 flex-1 items-center gap-2">
                                <Chip
                                  size="sm"
                                  variant="tertiary"
                                  color="default"
                                  className="shrink-0"
                                >
                                  {option.kind === "doc" ? "文档" : "笔记"}
                                </Chip>
                                <span className="min-w-0 flex-1 truncate">
                                  {option.title}
                                </span>
                                <ListBox.ItemIndicator />
                              </span>
                            </ListBox.Item>
                          ))}
                        </ListBox>
                      </Select.Popover>
                    </Select>
                    <Button
                      variant="secondary"
                      size="sm"
                      className="h-7 shrink-0 px-2.5 text-ui"
                      onPress={() => void submitContext()}
                      isDisabled={!contextKey || linkingContext}
                    >
                      {linkingContext ? "关联中…" : "关联"}
                    </Button>
                  </div>
                  {contextOptionsError && (
                    <p role="alert" className="text-ui-sm text-[var(--danger)]">
                      {contextOptionsError}
                    </p>
                  )}
                </div>
              )}
            </Section>

            {openDecisions.length > 0 && (
              <Section
                title="待决策"
                icon={<Inbox size={14} />}
                count={openDecisions.length}
              >
                <div className="overflow-hidden rounded-[10px] border border-[var(--border)]">
                  {openDecisions.map((decision) => (
                    <div
                      key={decision.id}
                      className="flex items-start gap-2 border-b border-[var(--border)] px-2 py-1.5 last:border-b-0"
                    >
                      <div className="min-w-0 flex-1">
                        <p className="whitespace-pre-wrap break-words text-ui-sm">
                          {decision.prompt}
                        </p>
                        <div className="mt-1 flex flex-wrap items-center gap-1.5 text-label text-[var(--muted)]">
                          <Chip
                            size="sm"
                            variant="tertiary"
                            color="warning"
                          >
                            {decisionKindLabel(decision.kind)}
                          </Chip>
                          <span>{formatDate(decision.created_at)}</span>
                        </div>
                      </div>
                      <div className="flex shrink-0 items-center gap-1">
                        <Button
                          variant="primary"
                          size="sm"
                          className="h-8 px-3 text-ui"
                          onPress={() =>
                            onResolveDecision(decision.id, "allow")
                          }
                          isDisabled={resolvingDecisionId !== null}
                        >
                          {resolvingDecisionId === decision.id
                            ? "处理中…"
                            : "允许"}
                        </Button>
                        <Button
                          variant="danger-soft"
                          size="sm"
                          className="h-8 px-3 text-ui"
                          onPress={() =>
                            onResolveDecision(decision.id, "deny")
                          }
                          isDisabled={resolvingDecisionId !== null}
                        >
                          拒绝
                        </Button>
                      </div>
                    </div>
                  ))}
                </div>
              </Section>
            )}

            <Section
              title="验收标准"
              icon={<ListChecks size={14} />}
              count={detail.criteria.length}
            >
              {detail.criteria.length === 0 ? (
                <Muted>暂无验收标准。</Muted>
              ) : (
                <div className="overflow-hidden rounded-[10px] border border-[var(--border)]">
                  {detail.criteria.map((criterion) => (
                    <div
                      key={criterion.id}
                      className="grid gap-1 border-b border-[var(--border)] px-2 py-1.5 last:border-b-0"
                    >
                      <p
                        className="min-w-0 truncate font-mono text-ui-sm"
                        title={criterion.cmd}
                      >
                        {criterion.cmd}
                      </p>
                      <div className="flex items-center gap-2 text-label text-[var(--muted)]">
                        <span className="shrink-0 font-mono">
                          {criterion.id}
                        </span>
                        <span className="shrink-0">
                          期望退出码 {criterion.expect_exit}
                        </span>
                        {criterion.expect_match && (
                          <span
                            className="min-w-0 truncate"
                            title={criterion.expect_match}
                          >
                            匹配 {criterion.expect_match}
                          </span>
                        )}
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </Section>

            <Section
              title="运行"
              icon={<GitBranch size={14} />}
              count={detail.runs.length}
            >
              {detail.runs.length === 0 ? (
                <Muted>暂无运行记录。</Muted>
              ) : (
                <div className="grid gap-1.5">
                  {detail.runs.map((run) => (
                    <div
                      key={run.id}
                      className="rounded-lg border border-[var(--border)] px-2.5 py-2"
                    >
                      <div className="flex items-center gap-1.5">
                        <Chip
                          size="sm"
                          variant="soft"
                          color={statusTone(run.status)}
                        >
                          {statusLabel(run.status)}
                        </Chip>
                        <span className="min-w-0 truncate font-mono text-ui-sm text-[var(--muted)]">
                          {run.adapter_id}
                        </span>
                        <span className="ml-auto shrink-0 text-label text-[var(--muted)]">
                          {formatDate(run.started_at)}
                        </span>
                      </div>
                      {run.branch && (
                        <p
                          className="mt-1 truncate font-mono text-label text-[var(--muted)]"
                          title={run.branch}
                        >
                          {run.branch}
                        </p>
                      )}
                      {run.worktree_path && (
                        <p
                          className="truncate font-mono text-label text-[var(--muted)]"
                          title={run.worktree_path}
                        >
                          {run.worktree_path}
                        </p>
                      )}
                      {(run.pr_number != null ||
                        run.pr_url ||
                        run.pr_state) && (
                        <div className="mt-1 flex flex-wrap items-center gap-1.5">
                          <GitPullRequest
                            size={12}
                            className="shrink-0 text-[var(--muted)]"
                          />
                          {run.pr_number != null && (
                            <span className="font-mono text-label text-[var(--muted)]">
                              PR #{run.pr_number}
                            </span>
                          )}
                          {run.pr_state && (
                            <Chip
                              size="sm"
                              variant="soft"
                              color={statusTone(run.pr_state ?? "")}
                            >
                              {statusLabel(run.pr_state ?? "")}
                            </Chip>
                          )}
                          {run.pr_url && (
                            <Button
                              variant="ghost"
                              size="sm"
                              className="h-6 gap-1 px-1.5 text-label"
                              onPress={() => {
                                setPrLinkError(null);
                                void openUrl(run.pr_url!).catch((e) =>
                                  setPrLinkError(errorText(e)),
                                );
                              }}
                            >
                              <ExternalLink size={12} />
                              打开 PR
                            </Button>
                          )}
                        </div>
                      )}
                      {run.error && (
                        <p className="mt-1 whitespace-pre-wrap break-words text-ui-sm text-[var(--danger)]">
                          {run.error}
                        </p>
                      )}
                    </div>
                  ))}
                </div>
              )}
              {prLinkError && (
                <p role="alert" className="text-ui-sm text-[var(--danger)]">
                  {prLinkError}
                </p>
              )}
            </Section>

            <Section
              title="证据"
              icon={<Terminal size={14} />}
              count={detail.evidence.length}
            >
              {detail.evidence.length === 0 ? (
                <Muted>暂无证据。</Muted>
              ) : (
                <div className="overflow-hidden rounded-[10px] border border-[var(--border)]">
                  {detail.evidence.map((item) => (
                    <div
                      key={item.id}
                      className="flex items-start gap-2 border-b border-[var(--border)] px-2 py-1.5 last:border-b-0"
                    >
                      <div className="min-w-0 flex-1">
                        <p
                          className="truncate font-mono text-ui-sm"
                          title={
                            item.cmd ?? item.output_path ?? item.kind
                          }
                        >
                          {item.cmd ?? item.output_path ?? item.kind}
                        </p>
                        <p className="text-label text-[var(--muted)]">
                          {item.kind}
                          {item.criterion_id
                            ? ` · ${item.criterion_id}`
                            : ""}
                          {item.duration_ms != null
                            ? ` · ${item.duration_ms}ms`
                            : ""}
                        </p>
                      </div>
                      <Chip
                        size="sm"
                        variant="soft"
                        color={
                          item.exit_code === 0
                            ? "success"
                            : item.exit_code == null
                              ? "default"
                              : "danger"
                        }
                      >
                        {item.exit_code == null
                          ? "—"
                          : `exit ${item.exit_code}`}
                      </Chip>
                    </div>
                  ))}
                </div>
              )}
            </Section>

            <AttachmentSection
              key={detail.task.id}
              taskId={detail.task.id}
              revision={attachmentsRevision}
              onChanged={onAttachmentsChanged}
            />

            <Section title="审查" icon={<ShieldCheck size={14} />}>
              {!review ? (
                <Muted>尚未审查。</Muted>
              ) : (
                <div className="grid gap-2 rounded-lg border border-[var(--border)] px-2.5 py-2">
                  <div className="flex items-center gap-1.5">
                    <Chip
                      size="sm"
                      variant="soft"
                      color={statusTone(review.verdict)}
                    >
                      {verdictLabel(review.verdict)}
                    </Chip>
                    <span className="text-ui-sm text-[var(--muted)]">
                      风险标记 {riskFlags.length}
                    </span>
                  </div>
                  {review.summary && (
                    <p className="whitespace-pre-wrap break-words text-ui-sm">
                      {review.summary}
                    </p>
                  )}
                  {riskFlags.length > 0 && (
                    <ul className="grid gap-1">
                      {riskFlags.map((flag, index) => (
                        <li
                          key={`${flag}-${index}`}
                          className="flex items-start gap-1.5 text-ui-sm text-[var(--danger)]"
                        >
                          <AlertTriangle
                            size={14}
                            className="mt-0.5 shrink-0"
                          />
                          <span className="break-words">{flag}</span>
                        </li>
                      ))}
                    </ul>
                  )}
                  {testDiffs.length > 0 && (
                    <div className="grid gap-0.5">
                      <p className="text-label text-[var(--muted)]">
                        测试/CI 改动
                      </p>
                      {testDiffs.map((diff, index) => (
                        <p
                          key={index}
                          className="break-words font-mono text-label text-[var(--muted)]"
                        >
                          {diff}
                        </p>
                      ))}
                    </div>
                  )}
                </div>
              )}
            </Section>

            <Section
              title="事件"
              icon={<Clock size={14} />}
              count={events.length}
            >
              {events.length === 0 ? (
                <Muted>暂无事件。</Muted>
              ) : (
                <div className="grid gap-1">
                  {events.map((event) => (
                    <div
                      key={event.id}
                      className="rounded-lg border border-[var(--border)] px-2 py-1"
                    >
                      <div className="flex items-center justify-between gap-2">
                        <span className="min-w-0 truncate text-ui-sm">
                          {event.kind}
                        </span>
                        <span className="shrink-0 text-label text-[var(--muted)]">
                          {formatDate(event.at)}
                        </span>
                      </div>
                      {event.payload_json && event.payload_json !== "{}" && (
                        <p
                          className="truncate font-mono text-label text-[var(--muted)]"
                          title={event.payload_json}
                        >
                          {compactJson(event.payload_json)}
                        </p>
                      )}
                    </div>
                  ))}
                </div>
              )}
            </Section>
          </div>
        </div>
      )}
    </aside>
  );
}

const MAX_TASK_ATTACHMENT_BYTES = 20 * 1024 * 1024;

function formatBytes(size: number): string {
  if (!Number.isFinite(size) || size < 0) return "—";
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}

function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = typeof reader.result === "string" ? reader.result : "";
      const comma = result.indexOf(",");
      resolve(comma >= 0 ? result.slice(comma + 1) : result);
    };
    reader.onerror = () => reject(reader.error ?? new Error("读取文件失败"));
    reader.readAsDataURL(file);
  });
}

/** 任务详情里的证据附件：上传、缩略图预览、删除与状态反馈。 */
function AttachmentSection({
  taskId,
  revision,
  onChanged,
}: {
  taskId: string;
  revision: number;
  onChanged: () => void;
}) {
  const [items, setItems] = useState<Attachment[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [uploading, setUploading] = useState(false);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [confirmingId, setConfirmingId] = useState<string | null>(null);
  const [previews, setPreviews] = useState<Record<string, string>>({});
  const [previewErrors, setPreviewErrors] = useState<Record<string, true>>({});
  const [preview, setPreview] = useState<{
    id: string;
    url: string;
    name: string;
  } | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const request = useRef(0);
  const loadedOnce = useRef(false);

  const load = useCallback(
    async (showLoading = true): Promise<void> => {
      const current = ++request.current;
      if (showLoading) setLoading(true);
      setError(null);
      try {
        const rows = await listTaskAttachments(taskId);
        if (request.current !== current) return;
        loadedOnce.current = true;
        setItems(rows);
        setPreviews({});
        setPreviewErrors({});
        setLoading(false);
        await Promise.all(
          rows.filter((item) =>
            item.mime.toLowerCase().startsWith("image/"),
          ).map(async (item) => {
            try {
              const base64 = await readTaskAttachment(item.id);
              if (request.current !== current) return;
              setPreviews((prev) => ({
                ...prev,
                [item.id]: `data:${item.mime || "application/octet-stream"};base64,${base64}`,
              }));
            } catch {
              if (request.current === current) {
                setPreviewErrors((prev) => ({ ...prev, [item.id]: true }));
              }
            }
          }),
        );
      } catch (e) {
        if (request.current !== current) return;
        setError(errorText(e));
        setLoading(false);
      }
    },
    [taskId],
  );

  // revision 由父级在上传/删除成功后递增；即使完成时本实例已被卸载，
  // 任务重新挂载后也会因 revision 变化重新拉取。已加载过就不再显示
  // “加载中”，避免刷新时列表闪烁。
  useEffect(() => {
    void load(!loadedOnce.current);
    return () => {
      request.current += 1;
    };
  }, [load, revision]);

  useEffect(() => {
    if (!preview) return;
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") setPreview(null);
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [preview]);

  async function upload(file: File) {
    if (uploading) return;
    if (file.size > MAX_TASK_ATTACHMENT_BYTES) {
      setError(`附件超过 20 MB，无法上传（${formatBytes(file.size)}）。`);
      return;
    }
    setUploading(true);
    setError(null);
    try {
      const dataBase64 = await fileToBase64(file);
      await addTaskAttachment(
        taskId,
        file.name || `附件-${Date.now()}`,
        dataBase64,
      );
      onChanged();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setUploading(false);
    }
  }

  function handleInput(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (file) void upload(file);
  }

  async function remove(item: Attachment) {
    if (deletingId !== null) return;
    if (confirmingId !== item.id) {
      setConfirmingId(item.id);
      return;
    }
    setDeletingId(item.id);
    setError(null);
    try {
      await deleteTaskAttachment(item.id);
      setConfirmingId(null);
      setItems((prev) =>
        prev === null ? prev : prev.filter((row) => row.id !== item.id),
      );
      if (preview?.id === item.id) setPreview(null);
      onChanged();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setDeletingId(null);
    }
  }

  return (
    <>
      <Section
        title="证据附件"
        icon={<Paperclip size={14} />}
        count={items?.length ?? 0}
      >
        <div className="flex flex-wrap items-center gap-1.5">
          <Button
            variant="secondary"
            size="sm"
            className="h-7 gap-1 px-2.5 text-ui"
            onPress={() => inputRef.current?.click()}
            isDisabled={uploading || loading || deletingId !== null}
          >
            <Upload size={14} />
            {uploading ? "上传中…" : "上传附件"}
          </Button>
          <span className="text-label text-[var(--muted)]">
            单个文件不超过 20 MB；图片会显示缩略图。
          </span>
        </div>
        <input
          ref={inputRef}
          type="file"
          className="hidden"
          aria-label="选择证据附件"
          onChange={handleInput}
          disabled={uploading}
        />
        {error && (
          <p role="alert" className="text-ui-sm text-[var(--danger)]">
            {error}
          </p>
        )}
        {loading ? (
          <Muted>加载中…</Muted>
        ) : items === null ? (
          <div className="flex items-center gap-2">
            <Muted>无法加载证据附件。</Muted>
            <Button
              variant="secondary"
              size="sm"
              className="h-6 px-2 text-label"
              onPress={() => void load()}
            >
              重试
            </Button>
          </div>
        ) : items.length === 0 ? (
          <Muted>暂无证据附件。</Muted>
        ) : (
          <div className="overflow-hidden rounded-[10px] border border-[var(--border)]">
            {items.map((item) => {
              const image = item.mime.toLowerCase().startsWith("image/");
              const thumbnail = previews[item.id];
              const confirming = confirmingId === item.id;
              const deleting = deletingId === item.id;
              return (
                <div
                  key={item.id}
                  className="flex items-center gap-2 border-b border-[var(--border)] px-2 py-1.5 last:border-b-0"
                >
                  {image ? (
                    <button
                      type="button"
                      className="flex size-9 shrink-0 items-center justify-center overflow-hidden rounded-md border border-[var(--border)] bg-[var(--background-secondary)]"
                      aria-label={`预览附件：${item.file_name}`}
                      title={thumbnail ? "点击放大" : undefined}
                      onClick={() => {
                        if (!thumbnail) return;
                        setPreview({
                          id: item.id,
                          url: thumbnail,
                          name: item.file_name,
                        });
                      }}
                      disabled={!thumbnail}
                    >
                      {thumbnail ? (
                        <img
                          src={thumbnail}
                          alt={item.file_name}
                          className="h-full w-full object-cover"
                        />
                      ) : (
                        <span className="text-label text-[var(--muted)]">
                          {previewErrors[item.id] ? "无法预览" : "…"}
                        </span>
                      )}
                    </button>
                  ) : (
                    <span className="flex size-9 shrink-0 items-center justify-center rounded-md border border-[var(--border)] bg-[var(--background-secondary)] text-[var(--muted)]">
                      <FileText size={15} />
                    </span>
                  )}
                  <div className="min-w-0 flex-1">
                    <p
                      className="truncate text-ui-sm"
                      title={item.file_name}
                    >
                      {item.file_name}
                    </p>
                    <p className="text-label text-[var(--muted)]">
                      {formatBytes(item.size)}
                      {image && previewErrors[item.id] ? " · 无法预览" : ""}
                      {item.created_at
                        ? ` · ${formatDate(item.created_at)}`
                        : ""}
                    </p>
                  </div>
                  <Button
                    variant={confirming ? "danger-soft" : "ghost"}
                    size="sm"
                    className={`h-6 shrink-0 px-2 text-label ${
                      confirming ? "" : "text-[var(--muted)]"
                    }`}
                    aria-label={
                      confirming
                        ? `确认删除附件：${item.file_name}`
                        : `删除附件：${item.file_name}`
                    }
                    onPress={() => void remove(item)}
                    isDisabled={deletingId !== null || uploading}
                  >
                    {deleting ? (
                      "删除中…"
                    ) : confirming ? (
                      "确认删除"
                    ) : (
                      <Trash2 size={13} />
                    )}
                  </Button>
                </div>
              );
            })}
          </div>
        )}
      </Section>

      {preview && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-6"
          role="dialog"
          aria-modal="true"
          aria-label={`预览 ${preview.name}`}
          onClick={() => setPreview(null)}
        >
          <button
            type="button"
            className="absolute right-4 top-4 flex size-8 items-center justify-center rounded-full bg-black/50 text-white"
            aria-label="关闭预览"
            onClick={() => setPreview(null)}
          >
            <X size={16} />
          </button>
          <img
            src={preview.url}
            alt={preview.name}
            className="max-h-full max-w-full rounded-lg object-contain"
            onClick={(event) => event.stopPropagation()}
          />
        </div>
      )}
    </>
  );
}

function DependencyRow({
  task,
  removing = false,
  onRemove,
}: {
  task: TaskSummary;
  removing?: boolean;
  onRemove?: () => void;
}) {
  return (
    <div className="flex items-center gap-2 border-b border-[var(--border)] px-2 py-1.5 last:border-b-0">
      <span className="min-w-0 flex-1 truncate" title={task.title}>
        {task.title}
      </span>
      <Chip
        size="sm"
        variant="soft"
        color={statusTone(task.status)}
        className="shrink-0"
      >
        {statusLabel(task.status)}
      </Chip>
      {onRemove && (
        <Button
          isIconOnly
          variant="ghost"
          size="sm"
          className="h-6 w-6 shrink-0 text-[var(--muted)]"
          aria-label={`移除依赖：${task.title}`}
          onPress={onRemove}
          isDisabled={removing}
        >
          <Trash2 size={13} />
        </Button>
      )}
    </div>
  );
}

function Section({
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

function Muted({ children }: { children: ReactNode }) {
  return <p className="text-ui-sm text-[var(--muted)]">{children}</p>;
}
