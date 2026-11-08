import { useEffect, useRef, useState, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  Button,
  Card,
  Chip,
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
  GitBranch,
  GitPullRequest,
  Inbox,
  Link2,
  ListChecks,
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
  cancelTask,
  compactJson,
  createTask,
  decisionKindLabel,
  errorText,
  formatDate,
  getAutoMerge,
  getProjectConcurrency,
  getProjectIssues,
  getTask,
  listDependencies,
  listEvents,
  listTasks,
  publishTask,
  removeDependency,
  resolveDecision,
  setAutoMerge,
  setProjectConcurrency,
  startTask,
  statusLabel,
  statusTone,
  syncNow,
  textList,
  verdictLabel,
  type EventRow,
  type ProjectIssue,
  type TaskDependencies,
  type TaskDetail,
  type TaskSummary,
} from "../lib/api";

const POLL_INTERVAL_MS = 2000;
// start_task 在后台线程推进，queued 也可能由队列 worker 取走；继续轮询才能看到状态推进。
const POLLING_STATUSES: Record<string, true> = {
  queued: true,
  running: true,
  verifying: true,
};

export function TasksPanel({ projectId }: { projectId: string }) {
  const [tasks, setTasks] = useState<TaskSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showCreate, setShowCreate] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<TaskDetail | null>(null);
  const [events, setEvents] = useState<EventRow[]>([]);
  const [detailLoading, setDetailLoading] = useState(false);
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
  const [concurrency, setConcurrency] = useState<number | null>(null);
  const [concurrencyInput, setConcurrencyInput] = useState("");
  const [concurrencySaving, setConcurrencySaving] = useState(false);
  const [concurrencyError, setConcurrencyError] = useState<string | null>(null);
  const [issues, setIssues] = useState<ProjectIssue[]>([]);
  const [issuesLoading, setIssuesLoading] = useState(true);
  const [issuesError, setIssuesError] = useState<string | null>(null);
  const [githubError, setGithubError] = useState<string | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [publishing, setPublishing] = useState(false);
  const [autoMerge, setAutoMergeEnabled] = useState<boolean | null>(null);
  const [autoMergeSaving, setAutoMergeSaving] = useState(false);
  const [autoMergeError, setAutoMergeError] = useState<string | null>(null);
  const selectedRef = useRef<string | null>(null);
  const detailRequestRef = useRef(0);
  const projectRef = useRef(projectId);

  useEffect(() => {
    let cancelled = false;
    projectRef.current = projectId;
    selectedRef.current = null;
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
    setIssues([]);
    setIssuesLoading(true);
    setIssuesError(null);
    setGithubError(null);
    setSyncing(false);
    setPublishing(false);
    listTasks(projectId)
      .then((next) => {
        if (!cancelled) setTasks(next);
      })
      .catch((e) => {
        if (!cancelled) setError(errorText(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
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

  async function loadDetail(taskId: string, silent: boolean) {
    if (!silent) setDetailLoading(true);
    const request = ++detailRequestRef.current;
    try {
      let dependencyFailure: string | null = null;
      const [nextDetail, nextEvents, nextDependencies] = await Promise.all([
        getTask(taskId),
        listEvents(taskId).catch(() => null),
        listDependencies(taskId).catch((e) => {
          dependencyFailure = errorText(e);
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
    setDetailLoading(false);
    setCancelError(null);
    setGithubError(null);
  }

  async function handleCreated(created: TaskDetail) {
    setShowCreate(false);
    setError(null);
    setDetailLoading(false);
    selectedRef.current = created.task.id;
    setSelectedId(created.task.id);
    setDetail(created);
    setEvents(created.events);
    setDependencies({ blockers: [], blocks: [] });
    setDependencyError(null);
    try {
      await Promise.all([refreshTasks(), refreshIssues()]);
    } catch (e) {
      setError(errorText(e));
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
        setTasks(nextTasks);
        const taskId = selectedRef.current;
        if (taskId) {
          const request = ++detailRequestRef.current;
          const [nextDetail, nextEvents, nextDependencies] = await Promise.all([
            getTask(taskId),
            listEvents(taskId).catch(() => null),
            listDependencies(taskId).catch(() => null),
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

  return (
    <div className="relative flex h-full min-h-0 gap-3">
      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 pb-2">
          <span className="flex items-center gap-1.5 text-ui-sm text-[var(--muted)]">
            <CircleDot size={14} />
            {tasks.length} 个任务
          </span>
          <div className="flex items-center gap-2">
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
            <div className="flex items-center gap-1 text-ui-sm text-[var(--muted)]">
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
                className="h-7 w-16 text-ui"
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
              variant="primary"
              size="sm"
              className="h-7 gap-1 px-2.5 text-ui"
              onPress={() => setShowCreate((prev) => !prev)}
            >
              <Plus size={14} />
              新建任务
            </Button>
          </div>
        </div>

        {(concurrencyError ||
          (concurrencyDirty && !concurrencyValid)) && (
          <p role="alert" className="mb-2 text-ui-sm text-[var(--danger)]">
            {concurrencyError ?? "并发上限需为 1–16 的整数。"}
          </p>
        )}

        {autoMergeError && (
          <p role="alert" className="mb-2 text-ui-sm text-[var(--danger)]">
            {autoMergeError}
          </p>
        )}

        {error && (
          <p role="alert" className="mb-2 text-ui-sm text-[var(--danger)]">
            {error}
          </p>
        )}

        {showCreate && (
          <CreateTaskForm
            projectId={projectId}
            onCancel={() => setShowCreate(false)}
            onCreated={handleCreated}
          />
        )}

        <div className="min-h-0 flex-1 overflow-auto">
          {loading && (
            <p className="text-ui-sm text-[var(--muted)]">加载中…</p>
          )}

          {!loading && !error && tasks.length === 0 && (
            <div className="flex h-full flex-col items-center justify-center gap-2 text-center">
              <ListChecks size={20} className="text-[var(--muted)] opacity-70" />
              <p className="text-ui text-[var(--muted)]">
                还没有任务。新建一个任务开始执行。
              </p>
              {!showCreate && (
                <Button
                  variant="secondary"
                  size="sm"
                  className="h-7 px-3 text-ui"
                  onPress={() => setShowCreate(true)}
                >
                  新建任务
                </Button>
              )}
            </div>
          )}

          {!loading && tasks.length > 0 && (
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
          dependencies={dependencies}
          dependencyError={dependencyError}
          addingDependency={addingDependency}
          removingDependencyId={removingDependencyId}
          issue={selectedIssue}
          issuesLoading={issuesLoading}
          issuesError={issuesError}
          githubError={githubError}
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
    "grid-cols-[minmax(0,1fr)_78px_48px_48px_84px]";

  return (
    <div className="overflow-hidden rounded-md border border-[var(--border)]">
      <div
        className={`text-ui-sm grid ${columns} gap-2 border-b border-[var(--border)] bg-[var(--background-secondary)] px-3 py-1.5 text-[var(--muted)]`}
      >
        <span>名称</span>
        <span>状态</span>
        <span>尝试</span>
        <span>验收</span>
        <span>创建</span>
      </div>
      {tasks.map((task) => (
        <button
          key={task.id}
          type="button"
          onClick={() => onSelect(task.id)}
          className={`text-ui anim-fast grid ${columns} h-9 w-full items-center gap-2 border-b border-[var(--border)] px-3 text-left last:border-b-0 hover:bg-[var(--background-secondary)] ${
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
    <Card className="mb-2">
      <Card.Header>
        <Card.Title className="text-ui font-medium">新建任务</Card.Title>
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
          className="h-7 px-2.5 text-ui"
          onPress={onCancel}
          isDisabled={busy}
        >
          取消
        </Button>
        <Button
          variant="primary"
          size="sm"
          className="h-7 px-2.5 text-ui"
          onPress={submit}
          isDisabled={busy || !title.trim() || !prompt.trim()}
        >
          {busy ? "创建中…" : "创建"}
        </Button>
      </Card.Footer>
    </Card>
  );
}

function DetailPane({
  detail,
  events,
  tasks,
  dependencies,
  dependencyError,
  addingDependency,
  removingDependencyId,
  issue,
  issuesLoading,
  issuesError,
  githubError,
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
  onSync,
  onPublish,
  onStart,
  onCancel,
  onClose,
}: {
  detail: TaskDetail | null;
  events: EventRow[];
  tasks: TaskSummary[];
  dependencies: TaskDependencies | null;
  dependencyError: string | null;
  addingDependency: boolean;
  removingDependencyId: string | null;
  issue: ProjectIssue | null;
  issuesLoading: boolean;
  issuesError: string | null;
  githubError: string | null;
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
  onSync: () => void;
  onPublish: () => void;
  onStart: () => void;
  onCancel: () => void;
  onClose: () => void;
}) {
  const [dependsOnKey, setDependsOnKey] = useState<string | null>(null);
  const [prLinkError, setPrLinkError] = useState<string | null>(null);
  const detailTaskId = detail?.task.id ?? null;
  useEffect(() => {
    setDependsOnKey(null);
    setPrLinkError(null);
  }, [detailTaskId]);

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

  async function submitDependency() {
    if (!dependsOnKey || addingDependency) return;
    const added = await onAddDependency(dependsOnKey);
    if (added) setDependsOnKey(null);
  }

  return (
    <aside className="absolute inset-y-0 right-0 z-10 flex h-full w-[400px] max-w-full shrink-0 flex-col overflow-hidden rounded-md border border-[var(--border)] bg-[var(--background)] shadow-[0_8px_24px_rgba(0,0,0,0.12)] xl:static xl:w-[380px] xl:shadow-none">
      <header className="flex shrink-0 items-start gap-2 border-b border-[var(--border)] px-3 py-2">
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
            className="h-7 shrink-0 gap-1 px-2.5 text-ui"
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
            className="h-7 shrink-0 gap-1 px-2.5 text-ui"
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
          className="h-7 w-7 shrink-0"
          aria-label="关闭详情"
          onPress={onClose}
        >
          <X size={14} />
        </Button>
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
        <div className="min-h-0 flex-1 overflow-auto px-3 py-3">
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
                <div className="grid gap-1 rounded-md border border-[var(--border)] px-2 py-1.5">
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
                      <div className="overflow-hidden rounded-md border border-[var(--border)]">
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
                      <div className="overflow-hidden rounded-md border border-[var(--border)]">
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

            {openDecisions.length > 0 && (
              <Section
                title="待决策"
                icon={<Inbox size={14} />}
                count={openDecisions.length}
              >
                <div className="overflow-hidden rounded-md border border-[var(--border)]">
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
                          className="h-7 px-2.5 text-ui"
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
                          className="h-7 px-2.5 text-ui"
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
                <div className="overflow-hidden rounded-md border border-[var(--border)]">
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
                      className="rounded-md border border-[var(--border)] px-2 py-1.5"
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
                <div className="overflow-hidden rounded-md border border-[var(--border)]">
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

            <Section title="审查" icon={<ShieldCheck size={14} />}>
              {!review ? (
                <Muted>尚未审查。</Muted>
              ) : (
                <div className="grid gap-2 rounded-md border border-[var(--border)] px-2 py-1.5">
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
                      className="rounded-md border border-[var(--border)] px-2 py-1"
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
    <section className="grid gap-1.5">
      <div className="flex items-center justify-between gap-2">
        <h3 className="flex items-center gap-1.5 text-label font-medium uppercase tracking-wide text-[var(--muted)]">
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
