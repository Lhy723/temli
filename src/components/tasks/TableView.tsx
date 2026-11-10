import { Chip } from "@heroui/react";
import {
  formatDate,
  statusLabel,
  statusTone,
  type TaskSummary,
} from "../../lib/api";
import { NextActorTag } from "./shared";

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


export { TaskTable };
