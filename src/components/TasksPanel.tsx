import { useEffect, useRef, useState, type ReactNode } from "react";
import { Button, Card, Chip, Input, TextArea } from "@heroui/react";
import {
  AlertTriangle,
  CircleDot,
  Clock,
  GitBranch,
  Inbox,
  ListChecks,
  Play,
  Plus,
  ShieldCheck,
  Terminal,
  X,
} from "lucide-react";
import {
  compactJson,
  createTask,
  errorText,
  formatDate,
  getTask,
  listEvents,
  listTasks,
  resolveDecision,
  startTask,
  statusLabel,
  statusTone,
  textList,
  verdictLabel,
  type EventRow,
  type TaskDetail,
  type TaskSummary,
} from "../lib/api";

const POLL_INTERVAL_MS = 2000;
// start_task 在后台线程推进，queued 是进入 running 前的瞬态；继续轮询才能看到状态推进。
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
  const selectedRef = useRef<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    selectedRef.current = null;
    setLoading(true);
    setError(null);
    setTasks([]);
    setSelectedId(null);
    setDetail(null);
    setEvents([]);
    setResolvingDecisionId(null);
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

  async function refreshTasks() {
    setTasks(await listTasks(projectId));
  }

  async function loadDetail(taskId: string, silent: boolean) {
    if (!silent) setDetailLoading(true);
    try {
      const [nextDetail, nextEvents] = await Promise.all([
        getTask(taskId),
        listEvents(taskId).catch(() => null),
      ]);
      if (selectedRef.current !== taskId) return;
      setDetail(nextDetail);
      setEvents(nextEvents ?? nextDetail.events);
      if (!silent) setError(null);
    } catch (e) {
      if (selectedRef.current === taskId) setError(errorText(e));
    } finally {
      if (selectedRef.current === taskId && !silent) setDetailLoading(false);
    }
  }

  function selectTask(taskId: string) {
    selectedRef.current = taskId;
    setSelectedId(taskId);
    setDetail(null);
    setEvents([]);
    void loadDetail(taskId, false);
  }

  function closeDetail() {
    selectedRef.current = null;
    setSelectedId(null);
    setDetail(null);
    setEvents([]);
    setDetailLoading(false);
  }

  async function handleCreated(created: TaskDetail) {
    setShowCreate(false);
    setError(null);
    setDetailLoading(false);
    selectedRef.current = created.task.id;
    setSelectedId(created.task.id);
    setDetail(created);
    setEvents(created.events);
    try {
      await refreshTasks();
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
          const [nextDetail, nextEvents] = await Promise.all([
            getTask(taskId),
            listEvents(taskId).catch(() => null),
          ]);
          if (cancelled || selectedRef.current !== taskId) return;
          setDetail(nextDetail);
          setEvents(nextEvents ?? nextDetail.events);
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

  return (
    <div className="relative flex h-full min-h-0 gap-3">
      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex shrink-0 items-center justify-between gap-2 pb-2">
          <span className="flex items-center gap-1.5 text-ui-sm text-[var(--muted)]">
            <CircleDot size={14} />
            {tasks.length} 个任务
          </span>
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
          loading={detailLoading}
          busy={busy}
          resolvingDecisionId={resolvingDecisionId}
          onResolveDecision={(decisionId, resolution) =>
            void resolveTaskDecision(decisionId, resolution)
          }
          onStart={() => void startSelected()}
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
  loading,
  busy,
  resolvingDecisionId,
  onResolveDecision,
  onStart,
  onClose,
}: {
  detail: TaskDetail | null;
  events: EventRow[];
  loading: boolean;
  busy: boolean;
  resolvingDecisionId: string | null;
  onResolveDecision: (decisionId: string, resolution: "allow" | "deny") => void;
  onStart: () => void;
  onClose: () => void;
}) {
  const review = detail?.review ?? null;
  const riskFlags = review ? textList(review.risk_flags) : [];
  const testDiffs = review ? textList(review.test_diff) : [];
  const openDecisions =
    detail?.decisions.filter((decision) => decision.status === "open") ?? [];
  const startDisabled =
    busy ||
    !detail ||
    detail.task.status === "running" ||
    detail.task.status === "verifying" ||
    detail.task.status === "merged";

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
                            {decision.kind}
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
                      {run.error && (
                        <p className="mt-1 whitespace-pre-wrap break-words text-ui-sm text-[var(--danger)]">
                          {run.error}
                        </p>
                      )}
                    </div>
                  ))}
                </div>
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
