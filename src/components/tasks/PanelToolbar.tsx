import {
  Button,
  Description,
  Input,
  Switch,
} from "@heroui/react";
import {
  CircleDot,
  Inbox,
  LayoutGrid,
  List,
  ListChecks,
  Plus,
} from "lucide-react";
import type { ProjectSettings } from "./hooks";

export function PanelToolbar({
  taskCount,
  pendingIssueCount,
  view,
  onViewChange,
  settings,
  showPlan,
  onTogglePlan,
  onToggleCreate,
  error,
  issuesError,
}: {
  taskCount: number;
  pendingIssueCount: number;
  view: "board" | "table";
  onViewChange: (view: "board" | "table") => void;
  settings: ProjectSettings;
  showPlan: boolean;
  onTogglePlan: () => void;
  onToggleCreate: () => void;
  error: string | null;
  issuesError: string | null;
}) {
  const {
    autoMerge,
    autoMergeSaving,
    saveAutoMerge,
    requireChecks,
    requireChecksSaving,
    saveRequireChecks,
    autoDocs,
    autoDocsSaving,
    saveAutoDocs,
    concurrency,
    concurrencyInput,
    setConcurrencyInput,
    setConcurrencyError,
    concurrencySaving,
    concurrencyError,
    concurrencyValid,
    concurrencyDirty,
    saveConcurrency,
  } = settings;

  return (
    <>
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 pb-3">
        <div className="flex items-center gap-2">
          <span className="flex items-center gap-1.5 text-ui-sm text-[var(--muted)]">
            <CircleDot size={14} />
            {taskCount} 个任务
          </span>
          {pendingIssueCount > 0 && (
            <span
              className="flex items-center gap-1.5 text-ui-sm text-[var(--muted)]"
              title="还没有关联任务的 open issue"
            >
              <Inbox size={14} />
              {pendingIssueCount} 个 issue 待创建
            </span>
          )}
          <div className="flex h-8 items-center rounded-lg border border-[var(--border)] p-0.5">
            <Button
              variant={view === "board" ? "secondary" : "ghost"}
              size="sm"
              className="h-7 gap-1 px-2 text-ui-sm"
              aria-pressed={view === "board"}
              onPress={() => onViewChange("board")}
            >
              <LayoutGrid size={13} />
              看板
            </Button>
            <Button
              variant={view === "table" ? "secondary" : "ghost"}
              size="sm"
              className="h-7 gap-1 px-2 text-ui-sm"
              aria-pressed={view === "table"}
              onPress={() => onViewChange("table")}
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
            onPress={onTogglePlan}
          >
            <ListChecks size={14} />
            规划
          </Button>
          <Button
            variant="primary"
            size="sm"
            className="h-8 gap-1 px-3 text-ui"
            onPress={onToggleCreate}
          >
            <Plus size={14} />
            新建任务
          </Button>
        </div>
      </div>

      {(concurrencyError || (concurrencyDirty && !concurrencyValid)) && (
        <p role="alert" className="mb-1 text-ui-sm text-[var(--danger)]">
          {concurrencyError ?? "并发上限需为 1–16 的整数。"}
        </p>
      )}

      {settings.autoMergeError && (
        <p role="alert" className="mb-1 text-ui-sm text-[var(--danger)]">
          {settings.autoMergeError}
        </p>
      )}

      {settings.requireChecksError && (
        <p role="alert" className="mb-1 text-ui-sm text-[var(--danger)]">
          {settings.requireChecksError}
        </p>
      )}

      {settings.autoDocsError && (
        <p role="alert" className="mb-1 text-ui-sm text-[var(--danger)]">
          {settings.autoDocsError}
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
    </>
  );
}
