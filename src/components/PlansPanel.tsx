import { useCallback, useEffect, useRef, useState } from "react";
import { Button, Card, Chip, Input, TextArea } from "@heroui/react";
import {
  Check,
  ChevronDown,
  ChevronRight,
  Flag,
  Milestone,
  Plus,
  RefreshCw,
  Trash2,
} from "lucide-react";
import {
  acceptStage,
  createPlan,
  errorText,
  formatDate,
  listPlans,
  listTasks,
  startPlan,
  statusLabel,
  statusTone,
  type CreatePlanInput,
  type Plan,
  type Stage,
  type TaskSummary,
} from "../lib/api";

type ChipColor = "default" | "accent" | "success" | "warning" | "danger";

type StageMeta = {
  label: string;
  color: ChipColor;
  marker: string;
  text: string;
};

const PLAN_STATUS: Record<string, { label: string; color: ChipColor }> = {
  draft: { label: "草稿", color: "default" },
  active: { label: "进行中", color: "accent" },
  done: { label: "已完成", color: "success" },
};

const STAGE_STATUS: Record<string, StageMeta> = {
  locked: {
    label: "锁定",
    color: "default",
    marker: "border-[var(--border-secondary)] text-[var(--muted)]",
    text: "text-[var(--muted)]",
  },
  active: {
    label: "进行中",
    color: "accent",
    marker: "border-[var(--accent)] text-[var(--accent)]",
    text: "",
  },
  review: {
    label: "待验收",
    color: "warning",
    marker: "border-[var(--warning)] text-[var(--warning)]",
    text: "",
  },
  passed: {
    label: "已通过",
    color: "success",
    marker: "border-[var(--success)] text-[var(--success)]",
    text: "text-[var(--muted)]",
  },
};

type StageDraft = {
  key: string;
  title: string;
  taskIds: string[];
};

export function PlansPanel({ projectId }: { projectId: string }) {
  const [plans, setPlans] = useState<Plan[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [showCreate, setShowCreate] = useState(false);
  const [busyPlanId, setBusyPlanId] = useState<string | null>(null);
  const [acceptingStageId, setAcceptingStageId] = useState<string | null>(
    null,
  );
  const [actionError, setActionError] = useState<{
    planId: string;
    message: string;
  } | null>(null);
  const request = useRef(0);

  const load = useCallback(async (): Promise<void> => {
    const current = ++request.current;
    setLoading(true);
    setError(null);
    try {
      const rows = await listPlans(projectId);
      if (request.current !== current) return;
      setPlans(rows);
    } catch (e) {
      if (request.current !== current) return;
      setError(errorText(e));
    } finally {
      if (request.current === current) setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    setPlans([]);
    setExpanded(new Set());
    setShowCreate(false);
    setActionError(null);
    void load();
    return () => {
      request.current += 1;
    };
  }, [load]);

  function toggle(planId: string) {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(planId)) next.delete(planId);
      else next.add(planId);
      return next;
    });
  }

  async function start(plan: Plan) {
    if (busyPlanId) return;
    setBusyPlanId(plan.id);
    setActionError(null);
    try {
      await startPlan(projectId, plan.id);
      await load();
      setExpanded((prev) => new Set(prev).add(plan.id));
    } catch (e) {
      setActionError({ planId: plan.id, message: errorText(e) });
    } finally {
      setBusyPlanId(null);
    }
  }

  async function accept(plan: Plan, stage: Stage) {
    if (acceptingStageId) return;
    setAcceptingStageId(stage.id);
    setActionError(null);
    try {
      await acceptStage(projectId, plan.id, stage.id);
      await load();
      setExpanded((prev) => new Set(prev).add(plan.id));
    } catch (e) {
      setActionError({ planId: plan.id, message: errorText(e) });
    } finally {
      setAcceptingStageId(null);
    }
  }

  async function created(plan: Plan) {
    setShowCreate(false);
    await load();
    setExpanded((prev) => new Set(prev).add(plan.id));
  }

  return (
    <section className="flex h-full min-h-0 flex-col">
      <header className="shrink-0 pb-2">
        <div className="flex items-center justify-between gap-2">
          <div className="flex items-center gap-1.5">
            <Milestone size={15} className="text-[var(--muted)]" />
            <h2 className="text-ui font-medium">计划</h2>
          </div>
          <div className="flex items-center gap-1.5">
            <Button
              isIconOnly
              variant="ghost"
              size="sm"
              className="h-6 w-6"
              aria-label="刷新计划"
              isDisabled={loading}
              onPress={() => void load()}
            >
              <RefreshCw size={14} />
            </Button>
            {!showCreate && (
              <Button
                variant="primary"
                size="sm"
                className="h-8 gap-1 px-3 text-ui"
                onPress={() => setShowCreate(true)}
              >
                <Plus size={14} />
                新建计划
              </Button>
            )}
          </div>
        </div>
        <p className="mt-0.5 text-ui-sm text-[var(--muted)]">
          按阶段编排任务；当前阶段任务全部合并后验收，并解锁下一阶段
        </p>
      </header>

      {error && (
        <div className="flex shrink-0 items-center gap-2 pb-2">
          <p role="alert" className="text-ui-sm text-[var(--danger)]">
            计划加载失败：{error}
          </p>
          <Button
            variant="secondary"
            size="sm"
            className="h-6 px-2 text-ui-sm"
            onPress={() => void load()}
          >
            重试
          </Button>
        </div>
      )}

      <div className="min-h-0 flex-1 overflow-auto">
        {showCreate && (
          <CreatePlanWizard
            projectId={projectId}
            onCancel={() => setShowCreate(false)}
            onCreated={created}
          />
        )}

        {loading && plans.length === 0 && (
          <p className="text-ui-sm text-[var(--muted)]">加载中…</p>
        )}

        {!loading && !error && plans.length === 0 && !showCreate && (
          <div className="flex h-full flex-col items-center justify-center gap-2 text-center">
            <Milestone
              size={20}
              className="text-[var(--muted)] opacity-70"
            />
            <p className="text-ui text-[var(--muted)]">还没有计划</p>
            <p className="max-w-[320px] text-ui-sm text-[var(--muted)]">
              把任务按阶段编排，逐段验收推进。点击右上角「新建计划」开始。
            </p>
          </div>
        )}

        {plans.length > 0 && (
          <div className="flex flex-col gap-2">
            {plans.map((plan) => {
              const stages = [...plan.stages].sort((a, b) => a.seq - b.seq);
              const passed = stages.filter(
                (stage) => stage.status === "passed",
              ).length;
              const totalTasks = stages.reduce(
                (sum, stage) => sum + stage.tasks.length,
                0,
              );
              const currentStage =
                stages.find(
                  (stage) =>
                    stage.status === "active" || stage.status === "review",
                ) ?? null;
              const reviewStage =
                stages.find((stage) => stage.status === "review") ?? null;
              const isExpanded = expanded.has(plan.id);
              const starting = busyPlanId === plan.id;
              const accepting =
                reviewStage !== null && acceptingStageId === reviewStage.id;
              const meta = PLAN_STATUS[plan.status] ?? {
                label: plan.status,
                color: "default" as ChipColor,
              };

              return (
                <article
                  key={plan.id}
                  className="overflow-hidden rounded-[10px] border border-[var(--border)] bg-[var(--background)]"
                >
                  <div className="flex items-start gap-2 px-3 py-2.5">
                    <button
                      type="button"
                      className="anim-fast flex min-w-0 flex-1 items-start gap-2 rounded-lg py-0.5 text-left hover:bg-[var(--background-secondary)]"
                      aria-expanded={isExpanded}
                      onClick={() => toggle(plan.id)}
                    >
                      {isExpanded ? (
                        <ChevronDown
                          size={14}
                          className="mt-0.5 shrink-0 text-[var(--muted)]"
                        />
                      ) : (
                        <ChevronRight
                          size={14}
                          className="mt-0.5 shrink-0 text-[var(--muted)]"
                        />
                      )}
                      <div className="min-w-0 flex-1">
                        <div className="flex flex-wrap items-center gap-1.5">
                          <span
                            className="min-w-0 truncate text-ui font-medium"
                            title={plan.title}
                          >
                            {plan.title}
                          </span>
                          <Chip
                            size="sm"
                            variant="soft"
                            color={meta.color}
                          >
                            {meta.label}
                          </Chip>
                          <span className="text-label tabular-nums text-[var(--muted)]">
                            阶段 {passed}/{stages.length}
                          </span>
                        </div>
                        <p
                          className="mt-0.5 line-clamp-2 break-words text-ui-sm text-[var(--muted)]"
                          title={plan.goal}
                        >
                          {plan.goal || "未填写目标"}
                        </p>
                        <p className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-label text-[var(--muted)]">
                          <span className="flex items-center gap-1">
                            <Flag size={11} />
                            {plan.status === "draft"
                              ? `尚未启动 · 共 ${totalTasks} 个任务`
                              : currentStage
                                ? `当前阶段：${currentStage.title} · ${currentStage.tasks.length} 个任务`
                                : "全部阶段已通过"}
                          </span>
                        </p>
                      </div>
                    </button>
                    <div className="flex shrink-0 flex-col items-end gap-1 pt-0.5">
                      <div className="flex items-center gap-1.5">
                        {plan.status === "draft" && (
                          <Button
                            variant="secondary"
                            size="sm"
                            className="h-8 px-3 text-ui"
                            isDisabled={busyPlanId !== null}
                            onPress={() => void start(plan)}
                          >
                            {starting ? "启动中…" : "启动计划"}
                          </Button>
                        )}
                        {plan.status === "active" && reviewStage && (
                          <Button
                            variant="primary"
                            size="sm"
                            className="h-8 px-3 text-ui"
                            isDisabled={acceptingStageId !== null}
                            onPress={() => void accept(plan, reviewStage)}
                          >
                            {accepting ? "验收中…" : "验收阶段"}
                          </Button>
                        )}
                      </div>
                      {plan.status === "draft" && (
                        <p className="max-w-[240px] text-right text-label leading-snug text-[var(--muted)]">
                          启动后首阶段任务会自动入队；后续阶段通过验收解锁。
                        </p>
                      )}
                      {plan.status === "active" && reviewStage && (
                        <p className="max-w-[240px] text-right text-label leading-snug text-[var(--muted)]">
                          通过后下一阶段任务会自动入队。
                        </p>
                      )}
                    </div>
                  </div>

                  {actionError?.planId === plan.id && (
                    <p
                      role="alert"
                      className="px-3 pb-2 text-ui-sm text-[var(--danger)]"
                    >
                      {actionError.message}
                    </p>
                  )}

                  {isExpanded && (
                    <div className="border-t border-[var(--border)] px-3 py-2.5">
                      <ol className="grid gap-0">
                        {stages.map((stage, index) => {
                          const stageInfo = STAGE_STATUS[stage.status] ?? {
                            label: stage.status,
                            color: "default" as ChipColor,
                            marker:
                              "border-[var(--border-secondary)] text-[var(--muted)]",
                            text: "text-[var(--muted)]",
                          };
                          const last = index === stages.length - 1;
                          return (
                            <li key={stage.id} className="flex gap-2.5">
                              <div className="flex flex-col items-center">
                                <span
                                  className={`mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-full border ${stageInfo.marker}`}
                                >
                                  {stage.status === "passed" ? (
                                    <Check size={10} />
                                  ) : (
                                    <span className="size-1.5 rounded-full bg-current" />
                                  )}
                                </span>
                                {!last && (
                                  <span
                                    className="my-1 w-px flex-1 bg-[var(--border)]"
                                    aria-hidden="true"
                                  />
                                )}
                              </div>
                              <div
                                className={`min-w-0 flex-1 ${
                                  last ? "" : "pb-3"
                                }`}
                              >
                                <div className="flex flex-wrap items-center gap-1.5">
                                  <span className="font-mono text-label tabular-nums text-[var(--muted)]">
                                    #{stage.seq}
                                  </span>
                                  <span
                                    className={`text-ui-sm font-medium ${stageInfo.text}`}
                                  >
                                    {stage.title}
                                  </span>
                                  <Chip
                                    size="sm"
                                    variant="soft"
                                    color={stageInfo.color}
                                  >
                                    {stageInfo.label}
                                  </Chip>
                                  {stage.passed_at && (
                                    <span className="text-label text-[var(--muted)]">
                                      {formatDate(stage.passed_at)} 通过
                                    </span>
                                  )}
                                </div>
                                <div className="mt-1 grid gap-0.5">
                                  {stage.tasks.length === 0 ? (
                                    <p className="text-label text-[var(--muted)]">
                                      阶段内暂无任务。
                                    </p>
                                  ) : (
                                    stage.tasks.map((task) => (
                                      <div
                                        key={task.id}
                                        className="flex min-w-0 items-center gap-1.5 py-0.5"
                                      >
                                        <span
                                          className="size-1 shrink-0 rounded-full bg-[var(--border-secondary)]"
                                          aria-hidden="true"
                                        />
                                        <span
                                          className={`min-w-0 truncate text-ui-sm ${stageInfo.text}`}
                                          title={task.title}
                                        >
                                          {task.title}
                                        </span>
                                        <Chip
                                          size="sm"
                                          variant="tertiary"
                                          color={statusTone(task.status)}
                                        >
                                          {statusLabel(task.status)}
                                        </Chip>
                                      </div>
                                    ))
                                  )}
                                </div>
                              </div>
                            </li>
                          );
                        })}
                      </ol>
                    </div>
                  )}
                </article>
              );
            })}
          </div>
        )}
      </div>
    </section>
  );
}

function CreatePlanWizard({
  projectId,
  onCancel,
  onCreated,
}: {
  projectId: string;
  onCancel: () => void;
  onCreated: (plan: Plan) => Promise<void>;
}) {
  const [title, setTitle] = useState("");
  const [goal, setGoal] = useState("");
  const [stages, setStages] = useState<StageDraft[]>([
    { key: "stage-1", title: "", taskIds: [] },
  ]);
  const nextStageKey = useRef(2);
  const [tasks, setTasks] = useState<TaskSummary[]>([]);
  const [tasksLoading, setTasksLoading] = useState(true);
  const [tasksError, setTasksError] = useState<string | null>(null);
  const [tasksReload, setTasksReload] = useState(0);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setTasksLoading(true);
    setTasksError(null);
    listTasks(projectId)
      .then((rows) => {
        if (active) setTasks(rows);
      })
      .catch((e) => {
        if (!active) return;
        setTasks([]);
        setTasksError(errorText(e));
      })
      .finally(() => {
        if (active) setTasksLoading(false);
      });
    return () => {
      active = false;
    };
  }, [projectId, tasksReload]);

  const canCreate =
    !creating &&
    title.trim().length > 0 &&
    stages.length > 0 &&
    stages.every(
      (stage) => stage.title.trim().length > 0 && stage.taskIds.length > 0,
    );

  function setStageTitle(key: string, value: string) {
    setStages((prev) =>
      prev.map((stage) =>
        stage.key === key ? { ...stage, title: value } : stage,
      ),
    );
  }

  function addStage() {
    const key = `stage-${nextStageKey.current++}`;
    setStages((prev) => [...prev, { key, title: "", taskIds: [] }]);
  }

  function removeStage(key: string) {
    setStages((prev) =>
      prev.length <= 1 ? prev : prev.filter((stage) => stage.key !== key),
    );
  }

  function toggleTask(stageKey: string, taskId: string, checked: boolean) {
    setStages((prev) =>
      prev.map((stage) => {
        if (stage.key === stageKey) {
          if (!checked) {
            return {
              ...stage,
              taskIds: stage.taskIds.filter((id) => id !== taskId),
            };
          }
          return stage.taskIds.includes(taskId)
            ? stage
            : { ...stage, taskIds: [...stage.taskIds, taskId] };
        }
        // 一个任务最多属于一个阶段：勾选时从其他阶段移除。
        if (checked && stage.taskIds.includes(taskId)) {
          return {
            ...stage,
            taskIds: stage.taskIds.filter((id) => id !== taskId),
          };
        }
        return stage;
      }),
    );
  }

  async function submit() {
    if (!canCreate || creating) return;
    setCreating(true);
    setError(null);
    const input: CreatePlanInput = {
      title: title.trim(),
      goal: goal.trim(),
      stages: stages.map((stage) => ({
        title: stage.title.trim(),
        task_ids: stage.taskIds,
      })),
    };
    try {
      const plan = await createPlan(projectId, input);
      await onCreated(plan);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setCreating(false);
    }
  }

  return (
    <Card className="mb-3 max-w-[760px] rounded-[10px] p-3.5">
      <Card.Header>
        <Card.Title className="text-ui font-medium leading-[1.4]">
          新建计划
        </Card.Title>
        <Card.Description className="text-ui-sm">
          阶段按顺序解锁；当前阶段任务全部合并后进入验收。
        </Card.Description>
      </Card.Header>
      <Card.Content className="grid gap-3">
        <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
          标题
          <Input
            aria-label="计划标题"
            placeholder="例如：计划与阶段验收"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            className="h-8 text-ui"
            disabled={creating}
          />
        </label>
        <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
          目标
          <TextArea
            aria-label="计划目标"
            rows={2}
            placeholder="这个计划要达成的结果。"
            value={goal}
            onChange={(e) => setGoal(e.target.value)}
            className="text-ui"
            disabled={creating}
          />
        </label>

        <div className="grid gap-2">
          <div className="flex items-center justify-between gap-2">
            <span className="text-ui-sm font-medium">阶段</span>
            <Button
              variant="secondary"
              size="sm"
              className="h-8 gap-1 px-2.5 text-ui-sm"
              onPress={addStage}
              isDisabled={creating}
            >
              <Plus size={13} />
              添加阶段
            </Button>
          </div>

          {stages.map((stage, index) => (
            <div
              key={stage.key}
              className="rounded-[10px] border border-[var(--border)] p-2.5"
            >
              <div className="flex items-center gap-1.5">
                <span className="flex size-5 shrink-0 items-center justify-center rounded-full bg-[var(--background-tertiary)] text-label tabular-nums text-[var(--muted)]">
                  {index + 1}
                </span>
                <Input
                  aria-label={`阶段 ${index + 1} 标题`}
                  placeholder="阶段标题"
                  value={stage.title}
                  onChange={(e) => setStageTitle(stage.key, e.target.value)}
                  className="h-8 min-w-0 flex-1 text-ui"
                  disabled={creating}
                />
                <Button
                  isIconOnly
                  variant="ghost"
                  size="sm"
                  className="h-8 w-8"
                  aria-label={`删除阶段 ${index + 1}`}
                  isDisabled={creating || stages.length <= 1}
                  onPress={() => removeStage(stage.key)}
                >
                  <Trash2 size={14} />
                </Button>
              </div>

              <div className="mt-2">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-label text-[var(--muted)]">
                    任务 · 已选 {stage.taskIds.length}
                  </span>
                  {tasksError && !tasksLoading && (
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-6 px-1.5 text-label"
                      onPress={() => setTasksReload((value) => value + 1)}
                    >
                      重试
                    </Button>
                  )}
                </div>
                <div className="mt-1 max-h-40 overflow-auto rounded-lg border border-[var(--border)]">
                  {tasksLoading ? (
                    <p className="px-2 py-2 text-ui-sm text-[var(--muted)]">
                      任务加载中…
                    </p>
                  ) : tasksError ? (
                    <p
                      role="alert"
                      className="px-2 py-2 text-ui-sm text-[var(--danger)]"
                    >
                      任务加载失败：{tasksError}
                    </p>
                  ) : tasks.length === 0 ? (
                    <p className="px-2 py-2 text-ui-sm text-[var(--muted)]">
                      还没有任务。先到「我的任务」创建任务，再回来编排计划。
                    </p>
                  ) : (
                    tasks.map((task) => {
                      const checked = stage.taskIds.includes(task.id);
                      return (
                        <label
                          key={task.id}
                          className="anim-fast flex h-8 cursor-pointer items-center gap-2 border-b border-[var(--border)] px-2 last:border-b-0 hover:bg-[var(--background-secondary)]"
                        >
                          <input
                            type="checkbox"
                            className="size-3.5 shrink-0 accent-[var(--accent)]"
                            checked={checked}
                            disabled={creating}
                            onChange={(e) =>
                              toggleTask(stage.key, task.id, e.target.checked)
                            }
                          />
                          <span
                            className="min-w-0 flex-1 truncate text-ui-sm"
                            title={task.title}
                          >
                            {task.title}
                          </span>
                          <Chip
                            size="sm"
                            variant="tertiary"
                            color={statusTone(task.status)}
                          >
                            {statusLabel(task.status)}
                          </Chip>
                        </label>
                      );
                    })
                  )}
                </div>
              </div>
            </div>
          ))}

          <p className="text-label text-[var(--muted)]">
            每个阶段至少选择 1 个任务；同一个任务只能属于一个阶段。
          </p>
        </div>
      </Card.Content>
      <Card.Footer className="flex items-center justify-between gap-2">
        {error ? (
          <p
            role="alert"
            className="min-w-0 break-words text-ui-sm text-[var(--danger)]"
          >
            {error}
          </p>
        ) : (
          <span className="text-label text-[var(--muted)]">
            需填写标题，且每个阶段至少选择 1 个任务。
          </span>
        )}
        <div className="flex shrink-0 gap-1.5">
          <Button
            variant="ghost"
            size="sm"
            className="h-8 px-3 text-ui"
            onPress={onCancel}
            isDisabled={creating}
          >
            取消
          </Button>
          <Button
            variant="primary"
            size="sm"
            className="h-8 px-3 text-ui"
            onPress={() => void submit()}
            isDisabled={!canCreate}
          >
            {creating ? "创建中…" : "创建计划"}
          </Button>
        </div>
      </Card.Footer>
    </Card>
  );
}
