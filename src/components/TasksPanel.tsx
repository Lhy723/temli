import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@heroui/react";
import { LayoutGrid, ListChecks } from "lucide-react";
import {
  cancelTask,
  createTaskFromIssue,
  errorText,
  getProjectIssues,
  getTask,
  listContext,
  listDependencies,
  listEvents,
  listTasks,
  publishTask,
  resolveDecision,
  setTaskAgents,
  setTaskModel,
  startTask,
  syncNow,
  updateIssue,
  type ContextItem,
  type EventRow,
  type ProjectIssue,
  type TaskDependencies,
  type TaskDetail,
  type TaskSummary,
  type UpdateIssueInput,
} from "../lib/api";
import { CreateTaskForm } from "./tasks/CreateTaskDialog";
import { DetailPane } from "./tasks/DetailPane";
import { PanelToolbar } from "./tasks/PanelToolbar";
import { PlanPanel } from "./tasks/PlanDialog";
import { TaskBoard } from "./tasks/BoardView";
import { TaskTable } from "./tasks/TableView";
import {
  useAdapters,
  useContextOptions,
  useProjectSettings,
  useSelectedTaskRelations,
  useTaskPolling,
} from "./tasks/hooks";
import { issueNeedsTask } from "./tasks/shared";

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
  const selectedRef = useRef<string | null>(null);
  const detailRequestRef = useRef(0);
  const projectRef = useRef(projectId);
  const tasksSettledRef = useRef<string | null>(null);
  const focusConsumedRef = useRef<string | null>(null);
  // 上一轮轮询看到的状态快照；用于识别“非 merged → merged”的合并转变。
  const polledStatusesRef = useRef<Map<string, string> | null>(null);

  const settings = useProjectSettings(projectId, projectRef);
  const {
    adapters,
    loading: adaptersLoading,
    error: adaptersError,
  } = useAdapters();
  const {
    options: contextOptions,
    loading: contextOptionsLoading,
    error: contextOptionsError,
  } = useContextOptions(projectId);
  const {
    addSelectedDependency,
    removeSelectedDependency,
    linkSelectedContext,
    unlinkSelectedContext,
  } = useSelectedTaskRelations({
    selectedRef,
    loadDetail,
    refreshTasks,
    setError,
    setAddingDependency,
    setDependencyError,
    setRemovingDependencyId,
    setLinkingContext,
    setContextError,
    setRemovingContextId,
  });

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

  useTaskPolling({
    projectId,
    tasks,
    detail,
    selectedRef,
    detailRequestRef,
    autoDocsRef: settings.autoDocsRef,
    polledStatusesRef,
    refreshTasks,
    setTasks,
    setDetail,
    setEvents,
    setDependencies,
    setContextItems,
    setError,
  });

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

  const selectedTaskId = detail?.task.id ?? null;
  const selectedIssue =
    selectedTaskId === null
      ? null
      : (issues.find((item) => item.task_id === selectedTaskId) ?? null);
  const pendingIssues = issues.filter(issueNeedsTask);

  return (
    <div className="relative flex h-full min-h-0 gap-3">
      <div className="flex min-w-0 flex-1 flex-col">
        <PanelToolbar
          taskCount={tasks.length}
          pendingIssueCount={pendingIssues.length}
          view={view}
          onViewChange={setView}
          settings={settings}
          showPlan={showPlan}
          onTogglePlan={() => {
            setShowPlan((prev) => !prev);
            setShowCreate(false);
            setNotice(null);
          }}
          onToggleCreate={() => {
            setShowCreate((prev) => !prev);
            setShowPlan(false);
            setNotice(null);
          }}
          error={error}
          issuesError={issuesError}
        />

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
