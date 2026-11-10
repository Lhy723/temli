import { Button, Chip, ListBox, Select } from "@heroui/react";
import { AlertTriangle, Link2, Trash2 } from "lucide-react";
import {
  statusLabel,
  statusTone,
  type TaskDependencies,
  type TaskSummary,
} from "../../../lib/api";
import { Muted, Section } from "../shared";

export function DependenciesSection({
  detailTaskId,
  status,
  tasks,
  dependencies,
  dependencyError,
  addingDependency,
  removingDependencyId,
  dependsOnKey,
  setDependsOnKey,
  onAddDependency,
  onRemoveDependency,
}: {
  detailTaskId: string;
  status: string;
  tasks: TaskSummary[];
  dependencies: TaskDependencies | null;
  dependencyError: string | null;
  addingDependency: boolean;
  removingDependencyId: string | null;
  dependsOnKey: string | null;
  setDependsOnKey: (key: string | null) => void;
  onAddDependency: (dependsOnTaskId: string) => Promise<boolean>;
  onRemoveDependency: (dependsOnTaskId: string) => void;
}) {
  const blockers = dependencies?.blockers ?? [];
  const blocks = dependencies?.blocks ?? [];
  const linkedIds = new Set([...blockers, ...blocks].map((task) => task.id));
  const dependencyOptions = tasks.filter(
    (task) => task.id !== detailTaskId && !linkedIds.has(task.id),
  );
  const unmetBlockerCount = blockers.filter(
    (task) => task.status !== "merged",
  ).length;

  async function submitDependency() {
    if (!dependsOnKey || addingDependency) return;
    const added = await onAddDependency(dependsOnKey);
    if (added) setDependsOnKey(null);
  }

  return (
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
