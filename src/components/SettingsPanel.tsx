import { useEffect, useRef, useState, type ReactNode } from "react";
import { Button, Input, Switch, TextArea } from "@heroui/react";
import { invoke } from "@tauri-apps/api/core";
import { openPath } from "@tauri-apps/plugin-opener";
import {
  Copy,
  Cpu,
  Eye,
  EyeOff,
  FolderKanban,
  FolderOpen,
  Info,
  Play,
  RefreshCw,
  Terminal,
} from "lucide-react";
import {
  errorText,
  getAppSettings,
  getAutoDocs,
  getAutoMerge,
  getDevStatus,
  getProjectConcurrency,
  getRequireChecks,
  listAdapters,
  listProjects,
  readAppLog,
  setAutoDocs as updateAutoDocs,
  setAutoMerge as updateAutoMerge,
  setDefaultAgent,
  setDevMode,
  setProjectConcurrency,
  setRequireChecks as updateRequireChecks,
  setWorktreeRetentionDays,
  type AgentManifest,
  type AppSettings,
  type DevStatus,
  type ProjectInfo,
} from "../lib/api";

export function SettingsPanel({ projectId }: { projectId: string | null }) {
  // —— 全局设置 ——
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [settingsLoading, setSettingsLoading] = useState(true);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  const [settingsRevision, setSettingsRevision] = useState(0);

  const [adapters, setAdapters] = useState<AgentManifest[]>([]);
  const [adaptersLoading, setAdaptersLoading] = useState(true);
  const [adaptersError, setAdaptersError] = useState<string | null>(null);
  const [adaptersRevision, setAdaptersRevision] = useState(0);

  const [agentSaving, setAgentSaving] = useState(false);
  const [agentError, setAgentError] = useState<string | null>(null);
  const [agentNotice, setAgentNotice] = useAutoClearNotice();

  const [retentionInput, setRetentionInput] = useState("");
  const [retentionSaving, setRetentionSaving] = useState(false);
  const [retentionError, setRetentionError] = useState<string | null>(null);
  const [retentionNotice, setRetentionNotice] = useAutoClearNotice();

  const [openingDataDir, setOpeningDataDir] = useState(false);
  const [dataDirError, setDataDirError] = useState<string | null>(null);

  // —— 开发者 ——
  const [devStatus, setDevStatus] = useState<DevStatus | null>(null);
  const [devLoading, setDevLoading] = useState(true);
  const [devError, setDevError] = useState<string | null>(null);
  const [devRevision, setDevRevision] = useState(0);

  const [devEnabled, setDevEnabled] = useState(false);
  const [devPortInput, setDevPortInput] = useState("");
  const [devSaving, setDevSaving] = useState(false);
  const [devSaveError, setDevSaveError] = useState<string | null>(null);
  const [devNotice, setDevNotice] = useAutoClearNotice();

  const [tokenVisible, setTokenVisible] = useState(false);
  const [tokenError, setTokenError] = useState<string | null>(null);
  const [tokenNotice, setTokenNotice] = useAutoClearNotice();

  const [logLines, setLogLines] = useState<string[]>([]);
  const [logLoading, setLogLoading] = useState(false);
  const [logError, setLogError] = useState<string | null>(null);
  const [logRevision, setLogRevision] = useState(0);
  const logRef = useRef<HTMLPreElement | null>(null);

  const [ipcCommand, setIpcCommand] = useState("list_projects");
  const [ipcArgs, setIpcArgs] = useState("");
  const [ipcRunning, setIpcRunning] = useState(false);
  const [ipcResult, setIpcResult] = useState<string | null>(null);
  const [ipcError, setIpcError] = useState<string | null>(null);

  // —— 项目设置 ——
  const [project, setProject] = useState<ProjectInfo | null>(null);
  const [projectLoading, setProjectLoading] = useState(projectId !== null);
  const [projectError, setProjectError] = useState<string | null>(null);
  const [projectRevision, setProjectRevision] = useState(0);

  const [concurrency, setConcurrency] = useState<number | null>(null);
  const [concurrencyInput, setConcurrencyInput] = useState("");
  const [concurrencySaving, setConcurrencySaving] = useState(false);
  const [concurrencyError, setConcurrencyError] = useState<string | null>(null);
  const [concurrencyNotice, setConcurrencyNotice] = useAutoClearNotice();

  const [autoMerge, setAutoMerge] = useState<boolean | null>(null);
  const [autoMergeSaving, setAutoMergeSaving] = useState(false);
  const [autoMergeError, setAutoMergeError] = useState<string | null>(null);
  const [autoMergeNotice, setAutoMergeNotice] = useAutoClearNotice();

  const [requireChecks, setRequireChecks] = useState<boolean | null>(null);
  const [requireChecksSaving, setRequireChecksSaving] = useState(false);
  const [requireChecksError, setRequireChecksError] = useState<string | null>(
    null,
  );
  const [requireChecksNotice, setRequireChecksNotice] = useAutoClearNotice();

  const [autoDocs, setAutoDocs] = useState<boolean | null>(null);
  const [autoDocsSaving, setAutoDocsSaving] = useState(false);
  const [autoDocsError, setAutoDocsError] = useState<string | null>(null);
  const [autoDocsNotice, setAutoDocsNotice] = useAutoClearNotice();

  const projectRef = useRef(projectId);
  useEffect(() => {
    projectRef.current = projectId;
  }, [projectId]);

  useEffect(() => {
    let active = true;
    setSettingsLoading(true);
    setSettingsError(null);
    getAppSettings()
      .then((value) => {
        if (!active) return;
        setSettings(value);
        setRetentionInput(String(value.worktree_retention_days));
      })
      .catch((e) => {
        if (active) setSettingsError(errorText(e));
      })
      .finally(() => {
        if (active) setSettingsLoading(false);
      });
    return () => {
      active = false;
    };
  }, [settingsRevision]);

  useEffect(() => {
    let active = true;
    setAdaptersLoading(true);
    setAdaptersError(null);
    listAdapters()
      .then((rows) => {
        if (active) setAdapters(rows);
      })
      .catch((e) => {
        if (!active) return;
        setAdapters([]);
        setAdaptersError(errorText(e));
      })
      .finally(() => {
        if (active) setAdaptersLoading(false);
      });
    return () => {
      active = false;
    };
  }, [adaptersRevision]);

  const devActive = devStatus?.enabled === true;
  const devBaseUrl = devStatus?.base_url || null;
  const devToken = devStatus?.token || null;

  useEffect(() => {
    let active = true;
    setDevLoading(true);
    setDevError(null);
    getDevStatus()
      .then((value) => {
        if (!active) return;
        setDevStatus(value);
        setDevEnabled(value.enabled);
        setDevPortInput(String(value.port));
      })
      .catch((e) => {
        if (active) setDevError(errorText(e));
      })
      .finally(() => {
        if (active) setDevLoading(false);
      });
    return () => {
      active = false;
    };
  }, [devRevision]);

  useEffect(() => {
    if (!devActive) {
      setLogLines([]);
      setLogError(null);
      setLogLoading(false);
      return;
    }
    let active = true;
    setLogLoading(true);
    setLogError(null);
    readAppLog(200)
      .then((rows) => {
        if (active) setLogLines(rows);
      })
      .catch((e) => {
        if (!active) return;
        setLogLines([]);
        setLogError(errorText(e));
      })
      .finally(() => {
        if (active) setLogLoading(false);
      });
    return () => {
      active = false;
    };
  }, [devActive, logRevision]);

  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [logLines]);

  useEffect(() => {
    setProject(null);
    setProjectLoading(false);
    setProjectError(null);
    setConcurrency(null);
    setConcurrencyInput("");
    setConcurrencySaving(false);
    setConcurrencyError(null);
    setConcurrencyNotice(null);
    setAutoMerge(null);
    setAutoMergeSaving(false);
    setAutoMergeError(null);
    setAutoMergeNotice(null);
    setRequireChecks(null);
    setRequireChecksSaving(false);
    setRequireChecksError(null);
    setRequireChecksNotice(null);
    setAutoDocs(null);
    setAutoDocsSaving(false);
    setAutoDocsError(null);
    setAutoDocsNotice(null);

    if (!projectId) return;
    let active = true;
    const requestedFor = projectId;
    setProjectLoading(true);
    Promise.all([
      listProjects(),
      getProjectConcurrency(requestedFor),
      getAutoMerge(requestedFor),
      getRequireChecks(requestedFor),
      getAutoDocs(requestedFor),
    ])
      .then(([projects, limit, merge, checks, docs]) => {
        if (!active || projectRef.current !== requestedFor) return;
        setProject(projects.find((item) => item.id === requestedFor) ?? null);
        setConcurrency(limit);
        setConcurrencyInput(String(limit));
        setAutoMerge(merge);
        setRequireChecks(checks);
        setAutoDocs(docs);
      })
      .catch((e) => {
        if (!active || projectRef.current !== requestedFor) return;
        setProjectError(errorText(e));
      })
      .finally(() => {
        if (active && projectRef.current === requestedFor) {
          setProjectLoading(false);
        }
      });
    return () => {
      active = false;
    };
    // setState 函数与 hook 返回的 setter 都是稳定引用，无需放进依赖。
  }, [projectId, projectRevision]);

  const parsedRetention = Number(retentionInput);
  const retentionValid =
    Number.isInteger(parsedRetention) &&
    parsedRetention >= 1 &&
    parsedRetention <= 90;
  const retentionDirty =
    settings !== null &&
    parsedRetention !== settings.worktree_retention_days;

  const parsedConcurrency = Number(concurrencyInput);
  const concurrencyValid =
    Number.isInteger(parsedConcurrency) &&
    parsedConcurrency >= 1 &&
    parsedConcurrency <= 16;
  const concurrencyDirty =
    concurrency !== null && parsedConcurrency !== concurrency;

  const parsedDevPort = Number(devPortInput);
  const devPortValid =
    Number.isInteger(parsedDevPort) &&
    parsedDevPort >= 1024 &&
    parsedDevPort <= 65535;
  const devDirty =
    devStatus !== null &&
    (devEnabled !== devStatus.enabled || parsedDevPort !== devStatus.port);

  const agentValue = settings?.default_agent || null;
  const agentListed =
    agentValue !== null && adapters.some((item) => item.id === agentValue);

  async function saveDefaultAgent(next: string | null) {
    if (!settings || agentSaving) return;
    if (next === settings.default_agent) return;
    setAgentSaving(true);
    setAgentError(null);
    setAgentNotice(null);
    try {
      await setDefaultAgent(next);
      setSettings((prev) =>
        prev ? { ...prev, default_agent: next } : prev,
      );
      setAgentNotice("已保存");
    } catch (e) {
      setAgentError(errorText(e));
    } finally {
      setAgentSaving(false);
    }
  }

  async function saveRetentionDays() {
    if (retentionSaving) return;
    if (!retentionValid) {
      setRetentionError("保留天数需为 1–90 的整数。");
      return;
    }
    setRetentionSaving(true);
    setRetentionError(null);
    setRetentionNotice(null);
    try {
      await setWorktreeRetentionDays(parsedRetention);
      setSettings((prev) =>
        prev
          ? { ...prev, worktree_retention_days: parsedRetention }
          : prev,
      );
      setRetentionInput(String(parsedRetention));
      setRetentionNotice("已保存");
    } catch (e) {
      setRetentionError(errorText(e));
    } finally {
      setRetentionSaving(false);
    }
  }

  async function saveConcurrency() {
    if (!projectId || concurrencySaving) return;
    if (!concurrencyValid) {
      setConcurrencyError("并发上限需为 1–16 的整数。");
      return;
    }
    const requestedFor = projectId;
    setConcurrencySaving(true);
    setConcurrencyError(null);
    setConcurrencyNotice(null);
    try {
      await setProjectConcurrency({ project_id: requestedFor, limit: parsedConcurrency });
      if (projectRef.current !== requestedFor) return;
      setConcurrency(parsedConcurrency);
      setConcurrencyInput(String(parsedConcurrency));
      setConcurrencyNotice("已保存");
    } catch (e) {
      if (projectRef.current !== requestedFor) return;
      setConcurrencyError(errorText(e));
    } finally {
      if (projectRef.current === requestedFor) setConcurrencySaving(false);
    }
  }

  async function saveAutoMerge(enabled: boolean) {
    if (!projectId || autoMerge === null || autoMergeSaving) return;
    const requestedFor = projectId;
    setAutoMergeSaving(true);
    setAutoMergeError(null);
    setAutoMergeNotice(null);
    try {
      await updateAutoMerge({ project_id: requestedFor, enabled });
      if (projectRef.current !== requestedFor) return;
      setAutoMerge(enabled);
      setAutoMergeNotice("已保存");
    } catch (e) {
      if (projectRef.current !== requestedFor) return;
      setAutoMergeError(errorText(e));
    } finally {
      if (projectRef.current === requestedFor) setAutoMergeSaving(false);
    }
  }

  async function saveRequireChecks(enabled: boolean) {
    if (!projectId || requireChecks === null || requireChecksSaving) return;
    const requestedFor = projectId;
    setRequireChecksSaving(true);
    setRequireChecksError(null);
    setRequireChecksNotice(null);
    try {
      await updateRequireChecks({ project_id: requestedFor, enabled });
      if (projectRef.current !== requestedFor) return;
      setRequireChecks(enabled);
      setRequireChecksNotice("已保存");
    } catch (e) {
      if (projectRef.current !== requestedFor) return;
      setRequireChecksError(errorText(e));
    } finally {
      if (projectRef.current === requestedFor) setRequireChecksSaving(false);
    }
  }

  async function saveAutoDocs(enabled: boolean) {
    if (!projectId || autoDocs === null || autoDocsSaving) return;
    const requestedFor = projectId;
    setAutoDocsSaving(true);
    setAutoDocsError(null);
    setAutoDocsNotice(null);
    try {
      await updateAutoDocs({ project_id: requestedFor, enabled });
      if (projectRef.current !== requestedFor) return;
      setAutoDocs(enabled);
      setAutoDocsNotice("已保存");
    } catch (e) {
      if (projectRef.current !== requestedFor) return;
      setAutoDocsError(errorText(e));
    } finally {
      if (projectRef.current === requestedFor) setAutoDocsSaving(false);
    }
  }

  async function openDataDir() {
    if (!settings || openingDataDir) return;
    setOpeningDataDir(true);
    setDataDirError(null);
    try {
      await openPath(settings.data_dir);
    } catch (e) {
      setDataDirError(errorText(e));
    } finally {
      setOpeningDataDir(false);
    }
  }

  async function saveDevMode() {
    if (devSaving) return;
    if (!devPortValid) {
      setDevSaveError("端口需为 1024–65535 的整数。");
      return;
    }
    setDevSaving(true);
    setDevSaveError(null);
    setDevNotice(null);
    try {
      const next = await setDevMode(devEnabled, parsedDevPort);
      setDevStatus(next);
      setDevEnabled(next.enabled);
      setDevPortInput(String(next.port));
      setTokenVisible(false);
      setTokenError(null);
      setDevNotice("已保存");
    } catch (e) {
      setDevSaveError(errorText(e));
    } finally {
      setDevSaving(false);
    }
  }

  async function copyText(text: string) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const area = document.createElement("textarea");
      area.value = text;
      area.setAttribute("readonly", "");
      area.style.position = "fixed";
      area.style.opacity = "0";
      document.body.appendChild(area);
      area.select();
      const copied = document.execCommand("copy");
      document.body.removeChild(area);
      if (!copied) throw new Error("剪贴板不可用");
    }
  }

  async function copyToken() {
    const token = devStatus?.token;
    if (!token) return;
    setTokenError(null);
    setTokenNotice(null);
    try {
      await copyText(token);
      setTokenNotice("已复制");
    } catch (e) {
      setTokenError(`复制失败：${errorText(e)}`);
    }
  }

  async function runIpc() {
    const command = ipcCommand.trim();
    if (!command || ipcRunning) return;
    setIpcError(null);
    setIpcResult(null);
    let args: Record<string, unknown> = {};
    const raw = ipcArgs.trim();
    if (raw) {
      let parsed: unknown;
      try {
        parsed = JSON.parse(raw);
      } catch (e) {
        setIpcError(`参数不是合法 JSON：${errorText(e)}`);
        return;
      }
      if (
        parsed === null ||
        typeof parsed !== "object" ||
        Array.isArray(parsed)
      ) {
        setIpcError('参数需为 JSON 对象，例如 { "projectId": "…" }。');
        return;
      }
      args = parsed as Record<string, unknown>;
    }
    setIpcRunning(true);
    try {
      const result = await invoke<unknown>(command, args);
      setIpcResult(
        JSON.stringify(result === undefined ? null : result, null, 2),
      );
    } catch (e) {
      setIpcError(errorText(e));
    } finally {
      setIpcRunning(false);
    }
  }

  return (
    <div className="flex max-w-[760px] flex-col gap-4 pb-2">
      <header>
        <h1 className="page-title">设置</h1>
        <p className="mt-1 text-ui text-[var(--muted)]">
          项目行为与全局运行偏好
        </p>
      </header>

      {settingsError && (
        <div
          role="alert"
          className="flex flex-wrap items-center justify-between gap-2 rounded-[10px] border border-[var(--border)] bg-[var(--background-secondary)] px-3 py-2"
        >
          <p className="min-w-0 break-words text-ui-sm text-[var(--danger)]">
            读取全局设置失败：{settingsError}
          </p>
          <Button
            variant="ghost"
            size="sm"
            className="h-7 shrink-0 px-2 text-ui"
            onPress={() => setSettingsRevision((value) => value + 1)}
          >
            重试
          </Button>
        </div>
      )}

      <Section
        icon={FolderKanban}
        title="项目"
        description={
          project
            ? `${project.name} 的仓库信息与自动化设置`
            : "当前项目的仓库信息与自动化设置"
        }
      >
        {!projectId && (
          <div className="flex items-start gap-2 py-1">
            <FolderKanban
              size={16}
              className="mt-0.5 shrink-0 text-[var(--muted)] opacity-70"
            />
            <p className="text-ui-sm text-[var(--muted)]">
              尚未选择项目。先在侧栏选择一个项目，再调整并发与自动化设置。
            </p>
          </div>
        )}

        {projectId && projectLoading && (
          <p className="text-ui-sm text-[var(--muted)]">
            正在加载项目设置…
          </p>
        )}

        {projectId && !projectLoading && projectError && (
          <div className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-[var(--border)] bg-[var(--background-secondary)] px-3 py-2">
            <p className="min-w-0 break-words text-ui-sm text-[var(--danger)]">
              读取项目设置失败：{projectError}
            </p>
            <Button
              variant="ghost"
              size="sm"
              className="h-7 shrink-0 px-2 text-ui"
              onPress={() => setProjectRevision((value) => value + 1)}
            >
              重试
            </Button>
          </div>
        )}

        {projectId && !projectLoading && !projectError && !project && (
          <p className="text-ui-sm text-[var(--muted)]">
            未找到该项目的仓库信息，可能已被移除。请重新注册项目。
          </p>
        )}

        {projectId && !projectLoading && !projectError && project && (
          <>
            <dl className="flex flex-col gap-2">
              <InfoLine label="仓库路径" value={project.repo_path} breakAll />
              <InfoLine label="默认分支" value={project.default_branch} />
              <InfoLine
                label="GitHub 仓库"
                value={
                  project.github_owner && project.github_repo
                    ? `${project.github_owner}/${project.github_repo}`
                    : "未关联"
                }
                muted={!project.github_owner || !project.github_repo}
              />
            </dl>

            <div className="mt-3 flex flex-col divide-y divide-[var(--border)] border-t border-[var(--border)]">
              <SettingRow
                title="并发上限"
                description="同一项目中同时运行的任务数（1–16）。"
              >
                <div className="flex items-center gap-2">
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
                      setConcurrencyNotice(null);
                    }}
                    className="h-8 w-20 text-ui"
                    disabled={concurrency === null || concurrencySaving}
                  />
                  <Button
                    variant="secondary"
                    size="sm"
                    className="h-8 px-3 text-ui"
                    onPress={() => void saveConcurrency()}
                    isDisabled={
                      !concurrencyDirty ||
                      !concurrencyValid ||
                      concurrencySaving
                    }
                  >
                    保存
                  </Button>
                  <RowFeedback
                    saving={concurrencySaving}
                    error={concurrencyError}
                    notice={concurrencyNotice}
                  />
                </div>
                {concurrencyInput !== "" && !concurrencyValid && (
                  <p role="alert" className="text-ui-sm text-[var(--danger)]">
                    并发上限需为 1–16 的整数。
                  </p>
                )}
              </SettingRow>

              <SettingRow
                title="自动合并"
                description="审查通过后自动合并任务分支对应的 PR。"
              >
                <div className="flex items-center gap-2">
                  <RowFeedback
                    saving={autoMergeSaving}
                    error={autoMergeError}
                    notice={autoMergeNotice}
                  />
                  <Switch
                    size="sm"
                    isSelected={autoMerge === true}
                    isDisabled={autoMerge === null || autoMergeSaving}
                    onChange={(enabled) => void saveAutoMerge(enabled)}
                  >
                    <Switch.Content>
                      <Switch.Control>
                        <Switch.Thumb />
                      </Switch.Control>
                      <span className="sr-only">自动合并</span>
                    </Switch.Content>
                  </Switch>
                </div>
              </SettingRow>

              <SettingRow
                title="要求 CI checks"
                description="没有通过的 CI checks 时阻止自动合并。"
              >
                <div className="flex items-center gap-2">
                  <RowFeedback
                    saving={requireChecksSaving}
                    error={requireChecksError}
                    notice={requireChecksNotice}
                  />
                  <Switch
                    size="sm"
                    isSelected={requireChecks === true}
                    isDisabled={
                      requireChecks === null || requireChecksSaving
                    }
                    onChange={(enabled) => void saveRequireChecks(enabled)}
                  >
                    <Switch.Content>
                      <Switch.Control>
                        <Switch.Thumb />
                      </Switch.Control>
                      <span className="sr-only">要求 CI checks</span>
                    </Switch.Content>
                  </Switch>
                </div>
              </SettingRow>

              <SettingRow
                title="自动维护文档"
                description="任务合并后自动创建文档更新任务。"
              >
                <div className="flex items-center gap-2">
                  <RowFeedback
                    saving={autoDocsSaving}
                    error={autoDocsError}
                    notice={autoDocsNotice}
                  />
                  <Switch
                    size="sm"
                    isSelected={autoDocs === true}
                    isDisabled={autoDocs === null || autoDocsSaving}
                    onChange={(enabled) => void saveAutoDocs(enabled)}
                  >
                    <Switch.Content>
                      <Switch.Control>
                        <Switch.Thumb />
                      </Switch.Control>
                      <span className="sr-only">自动维护文档</span>
                    </Switch.Content>
                  </Switch>
                </div>
              </SettingRow>
            </div>
          </>
        )}
      </Section>

      <Section
        icon={Cpu}
        title="运行"
        description="应用级运行偏好，对所有项目生效。"
      >
        {settingsLoading ? (
          <p className="text-ui-sm text-[var(--muted)]">正在加载设置…</p>
        ) : (
          <div className="flex flex-col divide-y divide-[var(--border)]">
            <SettingRow
              title="默认 Agent"
              description="新任务默认使用的适配器；「跟随环境」表示不覆盖。"
            >
              <div className="flex items-center gap-2">
                <select
                  aria-label="默认 Agent"
                  value={agentValue ?? ""}
                  disabled={
                    !settings || agentSaving || adaptersLoading
                  }
                  onChange={(e) =>
                    void saveDefaultAgent(e.target.value || null)
                  }
                  className="h-8 min-w-[168px] rounded-lg border border-[var(--border)] bg-[var(--background)] px-2 text-ui text-[var(--foreground)] disabled:cursor-not-allowed disabled:opacity-50"
                >
                  <option value="">跟随环境</option>
                  {adapters.map((adapter) => (
                    <option key={adapter.id} value={adapter.id}>
                      {adapter.name || adapter.id}
                    </option>
                  ))}
                  {agentValue !== null && !agentListed && (
                    <option value={agentValue}>
                      {agentValue}（未检测到）
                    </option>
                  )}
                </select>
                {adaptersLoading && (
                  <span className="text-ui-sm text-[var(--muted)]">
                    检测中…
                  </span>
                )}
                <RowFeedback
                  saving={agentSaving}
                  error={agentError}
                  notice={agentNotice}
                />
              </div>
              <p className="max-w-[280px] text-right text-ui-sm text-[var(--muted)]">
                环境变量 TEMLI_AGENT 存在时优先于此处设置。
              </p>
              {adaptersError && (
                <p className="max-w-[280px] text-right text-label text-[var(--danger)]">
                  适配器列表加载失败：{adaptersError}
                  <Button
                    variant="ghost"
                    size="sm"
                    className="ms-1 h-6 px-1 text-ui-sm"
                    onPress={() =>
                      setAdaptersRevision((value) => value + 1)
                    }
                    isDisabled={adaptersLoading}
                  >
                    重试
                  </Button>
                </p>
              )}
            </SettingRow>

            <SettingRow
              title="worktree 保留天数"
              description="孤儿 worktree 的保留天数（启动时清理；失败现场长期保留）。"
            >
              <div className="flex items-center gap-2">
                <Input
                  aria-label="worktree 保留天数"
                  type="number"
                  min={1}
                  max={90}
                  step={1}
                  value={retentionInput}
                  onChange={(e) => {
                    setRetentionInput(e.target.value);
                    setRetentionError(null);
                    setRetentionNotice(null);
                  }}
                  className="h-8 w-20 text-ui"
                  disabled={!settings || retentionSaving}
                />
                <span className="text-ui-sm text-[var(--muted)]">天</span>
                <Button
                  variant="secondary"
                  size="sm"
                  className="h-8 px-3 text-ui"
                  onPress={() => void saveRetentionDays()}
                  isDisabled={
                    !settings ||
                    !retentionDirty ||
                    !retentionValid ||
                    retentionSaving
                  }
                >
                  保存
                </Button>
                <RowFeedback
                  saving={retentionSaving}
                  error={retentionError}
                  notice={retentionNotice}
                />
              </div>
              {retentionInput !== "" && !retentionValid && (
                <p role="alert" className="text-ui-sm text-[var(--danger)]">
                  保留天数需为 1–90 的整数。
                </p>
              )}
            </SettingRow>
          </div>
        )}
      </Section>

      <Section icon={Info} title="关于" description="版本与本地数据位置。">
        <dl className="flex flex-col gap-3">
          <InfoLine
            label="版本"
            value={
              settings ? settings.version : settingsLoading ? "加载中…" : "—"
            }
            muted={!settings}
          />
          <div className="grid grid-cols-[88px_1fr] items-start gap-2">
            <dt className="pt-1.5 text-ui-sm text-[var(--muted)]">数据目录</dt>
            <dd className="flex min-w-0 flex-col gap-1">
              <div className="flex min-w-0 flex-wrap items-center gap-2">
                <span
                  className={`min-w-0 select-text break-all text-ui ${
                    settings ? "" : "text-[var(--muted)]"
                  }`}
                >
                  {settings
                    ? settings.data_dir
                    : settingsLoading
                      ? "加载中…"
                      : "—"}
                </span>
                <Button
                  variant="secondary"
                  size="sm"
                  className="h-8 shrink-0 gap-1 px-2.5 text-ui"
                  onPress={() => void openDataDir()}
                  isDisabled={!settings || openingDataDir}
                >
                  <FolderOpen size={14} />
                  {openingDataDir ? "打开中…" : "打开"}
                </Button>
              </div>
              {dataDirError && (
                <p role="alert" className="text-ui-sm text-[var(--danger)]">
                  打开数据目录失败：{dataDirError}
                </p>
              )}
            </dd>
          </div>
          <InfoLine label="许可证" value="MIT" />
        </dl>
      </Section>

      <Section
        icon={Terminal}
        title="开发者"
        description="本地 API、应用日志与 IPC 控制台，仅本机调试使用。"
      >
        {devLoading ? (
          <p className="text-ui-sm text-[var(--muted)]">
            正在读取开发者状态…
          </p>
        ) : (
          <>
            {devError && (
              <div className="mb-2 flex flex-wrap items-center justify-between gap-2 rounded-[10px] border border-[var(--border)] bg-[var(--background-secondary)] px-3 py-2">
                <p className="min-w-0 break-words text-ui-sm text-[var(--danger)]">
                  读取开发者状态失败：{devError}
                </p>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 shrink-0 px-2 text-ui"
                  onPress={() => setDevRevision((value) => value + 1)}
                >
                  重试
                </Button>
              </div>
            )}

            <div className="flex flex-col divide-y divide-[var(--border)]">
              <SettingRow
                title="开发者模式"
                description="在本机启动 HTTP API，仅监听 127.0.0.1 并要求 Bearer token。"
              >
                <Switch
                  size="sm"
                  isSelected={devEnabled}
                  isDisabled={devStatus === null || devSaving}
                  onChange={(enabled) => {
                    setDevEnabled(enabled);
                    setDevSaveError(null);
                    setDevNotice(null);
                  }}
                >
                  <Switch.Content>
                    <Switch.Control>
                      <Switch.Thumb />
                    </Switch.Control>
                    <span className="sr-only">开发者模式</span>
                  </Switch.Content>
                </Switch>
              </SettingRow>

              <SettingRow
                title="API 端口"
                description="1024–65535；保存后按新配置重启本地 API。"
              >
                <div className="flex items-center gap-2">
                  <Input
                    aria-label="开发者 API 端口"
                    type="number"
                    min={1024}
                    max={65535}
                    step={1}
                    value={devPortInput}
                    onChange={(e) => {
                      setDevPortInput(e.target.value);
                      setDevSaveError(null);
                      setDevNotice(null);
                    }}
                    className="h-8 w-24 text-ui"
                    disabled={devStatus === null || devSaving}
                  />
                  <Button
                    variant="secondary"
                    size="sm"
                    className="h-8 px-3 text-ui"
                    onPress={() => void saveDevMode()}
                    isDisabled={!devDirty || !devPortValid || devSaving}
                  >
                    保存
                  </Button>
                  <RowFeedback
                    saving={devSaving}
                    error={devSaveError}
                    notice={devNotice}
                  />
                </div>
                {devPortInput !== "" && !devPortValid && (
                  <p role="alert" className="text-ui-sm text-[var(--danger)]">
                    端口需为 1024–65535 的整数。
                  </p>
                )}
              </SettingRow>
            </div>

            {devActive && (
              <>
                <div className="mt-3 flex flex-col gap-2 border-t border-[var(--border)] pt-3">
                  <h3 className="text-ui font-medium">本地 API</h3>
                  <p className="text-ui-sm text-[var(--muted)]">
                    仅监听 127.0.0.1，所有请求需携带{" "}
                    <code className="rounded bg-[var(--background-tertiary)] px-1 py-0.5 font-mono text-label">
                      Authorization: Bearer &lt;token&gt;
                    </code>
                    。
                  </p>
                  <InfoLine
                    label="Base URL"
                    value={devBaseUrl || "—"}
                    muted={!devBaseUrl}
                    breakAll
                  />
                  <div className="grid grid-cols-[88px_1fr] items-center gap-2">
                    <span className="text-ui-sm text-[var(--muted)]">
                      Token
                    </span>
                    <div className="flex min-w-0 flex-wrap items-center gap-2">
                      <code className="min-w-0 select-text break-all rounded bg-[var(--background-tertiary)] px-1.5 py-1 font-mono text-label text-[var(--foreground)]">
                        {devToken
                          ? tokenVisible
                            ? devToken
                            : "•".repeat(16)
                          : "—"}
                      </code>
                      <Button
                        variant="secondary"
                        size="sm"
                        className="h-8 shrink-0 gap-1 px-2.5 text-ui"
                        onPress={() => {
                          setTokenVisible((value) => !value);
                          setTokenError(null);
                          setTokenNotice(null);
                        }}
                        isDisabled={!devToken}
                      >
                        {tokenVisible ? (
                          <EyeOff size={14} />
                        ) : (
                          <Eye size={14} />
                        )}
                        {tokenVisible ? "隐藏" : "显示"}
                      </Button>
                      <Button
                        variant="secondary"
                        size="sm"
                        className="h-8 shrink-0 gap-1 px-2.5 text-ui"
                        onPress={() => void copyToken()}
                        isDisabled={!devToken}
                      >
                        <Copy size={14} />
                        复制
                      </Button>
                      {tokenNotice && (
                        <span
                          role="status"
                          className="text-ui-sm text-[var(--accent)]"
                        >
                          {tokenNotice}
                        </span>
                      )}
                    </div>
                  </div>
                  {tokenError && (
                    <p role="alert" className="text-ui-sm text-[var(--danger)]">
                      {tokenError}
                    </p>
                  )}
                </div>

                <div className="mt-3 flex flex-col gap-2 border-t border-[var(--border)] pt-3">
                  <div className="flex flex-wrap items-start justify-between gap-2">
                    <div className="min-w-0">
                      <h3 className="text-ui font-medium">应用日志</h3>
                      <p className="mt-0.5 text-ui-sm text-[var(--muted)]">
                        最近 200 行，自动滚动到末尾。
                      </p>
                    </div>
                    <Button
                      variant="secondary"
                      size="sm"
                      className="h-8 shrink-0 gap-1 px-2.5 text-ui"
                      onPress={() => setLogRevision((value) => value + 1)}
                      isDisabled={logLoading}
                    >
                      <RefreshCw
                        size={14}
                        className={logLoading ? "animate-spin" : undefined}
                      />
                      {logLoading ? "刷新中…" : "刷新"}
                    </Button>
                  </div>
                  {logError && (
                    <p role="alert" className="text-ui-sm text-[var(--danger)]">
                      读取日志失败：{logError}
                    </p>
                  )}
                  <pre
                    ref={logRef}
                    aria-label="应用日志"
                    className="max-h-[220px] overflow-auto rounded-lg border border-[var(--border)] bg-[var(--background-secondary)] p-2 font-mono text-label leading-[1.6] text-[var(--foreground)]"
                  >
                    {logLines.length > 0
                      ? logLines.join("\n")
                      : logLoading
                        ? "加载中…"
                        : "暂无日志。"}
                  </pre>
                </div>

                <div className="mt-3 flex flex-col gap-2 border-t border-[var(--border)] pt-3">
                  <div>
                    <h3 className="text-ui font-medium">IPC 控制台</h3>
                    <p className="mt-0.5 text-ui-sm text-[var(--muted)]">
                      直接调用 Tauri 命令并查看 JSON 结果；仅本机调试使用。
                    </p>
                  </div>
                  <label className="grid gap-1 text-ui-sm text-[var(--muted)]">
                    命令名
                    <Input
                      aria-label="IPC 命令名"
                      value={ipcCommand}
                      onChange={(e) => {
                        setIpcCommand(e.target.value);
                        setIpcError(null);
                      }}
                      placeholder="list_projects"
                      className="h-8 font-mono text-ui"
                      spellCheck={false}
                    />
                  </label>
                  <label className="grid gap-1 text-ui-sm text-[var(--muted)]">
                    参数（JSON 对象，可留空）
                    <TextArea
                      aria-label="IPC 参数"
                      rows={3}
                      value={ipcArgs}
                      onChange={(e) => {
                        setIpcArgs(e.target.value);
                        setIpcError(null);
                      }}
                      placeholder='{ "projectId": "…" }'
                      className="font-mono text-ui-sm"
                      spellCheck={false}
                    />
                  </label>
                  <div className="flex flex-wrap items-center gap-2">
                    <Button
                      variant="primary"
                      size="sm"
                      className="h-8 gap-1 px-3 text-ui"
                      onPress={() => void runIpc()}
                      isDisabled={ipcRunning || !ipcCommand.trim()}
                    >
                      <Play size={13} />
                      {ipcRunning ? "运行中…" : "运行"}
                    </Button>
                    <span className="text-label text-[var(--muted)]">
                      {"示例：list_projects · list_tasks {\"projectId\":\"…\"} · get_dev_status"}
                    </span>
                  </div>
                  {ipcError && (
                    <p
                      role="alert"
                      className="break-words text-ui-sm text-[var(--danger)]"
                    >
                      运行失败：{ipcError}
                    </p>
                  )}
                  {ipcResult !== null && (
                    <pre className="max-h-[240px] overflow-auto rounded-lg border border-[var(--border)] bg-[var(--background-secondary)] p-2 font-mono text-label leading-[1.6] text-[var(--foreground)]">
                      {ipcResult}
                    </pre>
                  )}
                </div>
              </>
            )}
          </>
        )}
      </Section>
    </div>
  );
}

/** 保存成功提示自动消失；错误提示保留到用户再次操作。 */
function useAutoClearNotice() {
  const [notice, setNotice] = useState<string | null>(null);
  useEffect(() => {
    if (!notice) return;
    const timer = window.setTimeout(() => setNotice(null), 2400);
    return () => window.clearTimeout(timer);
  }, [notice]);
  return [notice, setNotice] as const;
}

function Section({
  icon: Icon,
  title,
  description,
  children,
}: {
  icon: typeof FolderKanban;
  title: string;
  description?: string;
  children: ReactNode;
}) {
  return (
    <section className="overflow-hidden rounded-[10px] border border-[var(--border)]">
      <header className="flex items-start gap-2 border-b border-[var(--border)] px-4 py-3">
        <Icon
          size={15}
          className="mt-0.5 shrink-0 text-[var(--muted)]"
        />
        <div className="min-w-0">
          <h2 className="text-ui font-medium">{title}</h2>
          {description && (
            <p className="mt-0.5 text-ui-sm text-[var(--muted)]">
              {description}
            </p>
          )}
        </div>
      </header>
      <div className="px-4 py-3">{children}</div>
    </section>
  );
}

function SettingRow({
  title,
  description,
  children,
}: {
  title: string;
  description?: string;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 py-2.5 first:pt-0 last:pb-0">
      <div className="min-w-0 max-w-[440px]">
        <p className="text-ui">{title}</p>
        {description && (
          <p className="mt-0.5 text-ui-sm text-[var(--muted)]">
            {description}
          </p>
        )}
      </div>
      <div className="flex flex-col items-end gap-1">{children}</div>
    </div>
  );
}

function InfoLine({
  label,
  value,
  muted,
  breakAll,
}: {
  label: string;
  value: string;
  muted?: boolean;
  breakAll?: boolean;
}) {
  return (
    <div className="grid grid-cols-[88px_1fr] items-baseline gap-2">
      <dt className="text-ui-sm text-[var(--muted)]">{label}</dt>
      <dd
        className={`min-w-0 select-text text-ui ${
          breakAll ? "break-all" : ""
        } ${muted ? "text-[var(--muted)]" : ""}`}
      >
        {value}
      </dd>
    </div>
  );
}

function RowFeedback({
  saving,
  error,
  notice,
}: {
  saving?: boolean;
  error?: string | null;
  notice?: string | null;
}) {
  if (saving) {
    return (
      <span className="text-ui-sm text-[var(--muted)]">保存中…</span>
    );
  }
  if (error) {
    return (
      <span
        role="alert"
        className="max-w-[260px] break-words text-right text-ui-sm text-[var(--danger)]"
      >
        {error}
      </span>
    );
  }
  if (notice) {
    return (
      <span role="status" className="text-ui-sm text-[var(--accent)]">
        {notice}
      </span>
    );
  }
  return null;
}
