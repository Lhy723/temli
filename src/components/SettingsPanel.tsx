import { useEffect, useRef, useState, type ReactNode } from "react";
import { Button, Input, Switch } from "@heroui/react";
import { openPath } from "@tauri-apps/plugin-opener";
import { Cpu, FolderKanban, FolderOpen, Info } from "lucide-react";
import {
  errorText,
  getAppSettings,
  getAutoDocs,
  getAutoMerge,
  getProjectConcurrency,
  getRequireChecks,
  listAdapters,
  listProjects,
  setAutoDocs as updateAutoDocs,
  setAutoMerge as updateAutoMerge,
  setDefaultAgent,
  setProjectConcurrency,
  setRequireChecks as updateRequireChecks,
  setWorktreeRetentionDays,
  type AgentManifest,
  type AppSettings,
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
