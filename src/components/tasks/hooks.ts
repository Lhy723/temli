import {
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type RefObject,
  type SetStateAction,
} from "react";
import {
  addDependency,
  errorText,
  getAutoDocs,
  getAutoMerge,
  getProjectConcurrency,
  getRequireChecks,
  getTask,
  linkContext,
  listAdapters,
  listContext,
  listDependencies,
  listDocs,
  listEvents,
  listTasks,
  reindexWorkspace,
  removeDependency,
  setAutoDocs,
  setAutoMerge,
  setProjectConcurrency,
  setRequireChecks,
  unlinkContext,
  type AgentManifest,
  type ContextItem,
  type EventRow,
  type TaskDependencies,
  type TaskDetail,
  type TaskSummary,
} from "../../lib/api";
import {
  POLLING_STATUSES,
  POLL_INTERVAL_MS,
  type ContextOption,
} from "./shared";

export function useAdapters(): {
  adapters: AgentManifest[];
  loading: boolean;
  error: string | null;
} {
  const [adapters, setAdapters] = useState<AgentManifest[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    listAdapters()
      .then((next) => {
        if (!cancelled) setAdapters(next);
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
  }, []);

  return { adapters, loading, error };
}

export function useContextOptions(projectId: string): {
  options: ContextOption[];
  loading: boolean;
  error: string | null;
} {
  const [options, setOptions] = useState<ContextOption[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setOptions([]);
    setLoading(true);
    setError(null);
    Promise.all([
      reindexWorkspace(projectId)
        .catch(() => null)
        .then(() => listDocs(projectId)),
    ])
      .then(([docs]) => {
        if (cancelled) return;
        setOptions(
          docs.map((doc) => ({
            id: doc.id,
            kind: "doc" as const,
            title: doc.title,
          })),
        );
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

  return { options, loading, error };
}

export type ProjectSettings = {
  autoMerge: boolean | null;
  autoMergeSaving: boolean;
  autoMergeError: string | null;
  saveAutoMerge: (enabled: boolean) => Promise<void>;
  requireChecks: boolean | null;
  requireChecksSaving: boolean;
  requireChecksError: string | null;
  saveRequireChecks: (enabled: boolean) => Promise<void>;
  autoDocs: boolean | null;
  autoDocsSaving: boolean;
  autoDocsError: string | null;
  saveAutoDocs: (enabled: boolean) => Promise<void>;
  autoDocsRef: RefObject<boolean>;
  concurrency: number | null;
  concurrencyInput: string;
  setConcurrencyInput: (value: string) => void;
  setConcurrencyError: (value: string | null) => void;
  concurrencySaving: boolean;
  concurrencyError: string | null;
  concurrencyValid: boolean;
  concurrencyDirty: boolean;
  saveConcurrency: () => Promise<void>;
};

export function useProjectSettings(
  projectId: string,
  projectRef: RefObject<string>,
): ProjectSettings {
  const [concurrency, setConcurrency] = useState<number | null>(null);
  const [concurrencyInput, setConcurrencyInput] = useState("");
  const [concurrencySaving, setConcurrencySaving] = useState(false);
  const [concurrencyError, setConcurrencyError] = useState<string | null>(null);
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
  // 轮询回调里需要最新的自动文档开关，用 ref 读取以免重建轮询定时器。
  const autoDocsRef = useRef(false);

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
  }, [projectId, projectRef]);

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
  }, [projectId, projectRef]);

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
  }, [projectId, projectRef]);

  useEffect(() => {
    autoDocsRef.current = autoDocs === true;
  }, [autoDocs]);

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

  const parsedConcurrency = Number(concurrencyInput);
  const concurrencyValid =
    Number.isInteger(parsedConcurrency) &&
    parsedConcurrency >= 1 &&
    parsedConcurrency <= 16;
  const concurrencyDirty =
    concurrency !== null && parsedConcurrency !== concurrency;

  return {
    autoMerge,
    autoMergeSaving,
    autoMergeError,
    saveAutoMerge,
    requireChecks,
    requireChecksSaving,
    requireChecksError,
    saveRequireChecks,
    autoDocs,
    autoDocsSaving,
    autoDocsError,
    saveAutoDocs,
    autoDocsRef,
    concurrency,
    concurrencyInput,
    setConcurrencyInput,
    setConcurrencyError,
    concurrencySaving,
    concurrencyError,
    concurrencyValid,
    concurrencyDirty,
    saveConcurrency,
  };
}

export function useTaskPolling({
  projectId,
  tasks,
  detail,
  selectedRef,
  detailRequestRef,
  autoDocsRef,
  polledStatusesRef,
  refreshTasks,
  setTasks,
  setDetail,
  setEvents,
  setDependencies,
  setContextItems,
  setError,
}: {
  projectId: string;
  tasks: TaskSummary[];
  detail: TaskDetail | null;
  selectedRef: RefObject<string | null>;
  detailRequestRef: RefObject<number>;
  autoDocsRef: RefObject<boolean>;
  polledStatusesRef: RefObject<Map<string, string> | null>;
  refreshTasks: () => Promise<void>;
  setTasks: Dispatch<SetStateAction<TaskSummary[]>>;
  setDetail: Dispatch<SetStateAction<TaskDetail | null>>;
  setEvents: Dispatch<SetStateAction<EventRow[]>>;
  setDependencies: Dispatch<SetStateAction<TaskDependencies | null>>;
  setContextItems: Dispatch<SetStateAction<ContextItem[] | null>>;
  setError: Dispatch<SetStateAction<string | null>>;
}) {
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
}

export function useSelectedTaskRelations({
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
}: {
  selectedRef: RefObject<string | null>;
  loadDetail: (taskId: string, silent: boolean) => Promise<void>;
  refreshTasks: () => Promise<void>;
  setError: Dispatch<SetStateAction<string | null>>;
  setAddingDependency: Dispatch<SetStateAction<boolean>>;
  setDependencyError: Dispatch<SetStateAction<string | null>>;
  setRemovingDependencyId: Dispatch<SetStateAction<string | null>>;
  setLinkingContext: Dispatch<SetStateAction<boolean>>;
  setContextError: Dispatch<SetStateAction<string | null>>;
  setRemovingContextId: Dispatch<SetStateAction<string | null>>;
}) {
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

  return {
    addSelectedDependency,
    removeSelectedDependency,
    linkSelectedContext,
    unlinkSelectedContext,
  };
}
