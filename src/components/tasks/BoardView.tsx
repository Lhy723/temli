import { Button, Chip } from "@heroui/react";
import {
  statusLabel,
  statusTone,
  type ProjectIssue,
  type TaskSummary,
  type UpdateIssueInput,
} from "../../lib/api";
import { IssueEditControls } from "./IssueEditor";
import { BOARD_COLUMNS, NextActorTag, taskAgentInfo } from "./shared";

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

export { TaskBoard };
