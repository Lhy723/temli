import { useState } from "react";
import { Button, Card, Chip, TextArea } from "@heroui/react";
import {
  createTasksFromPlan,
  errorText,
  planProject,
  type PlanDraft,
  type TaskDetail,
} from "../../lib/api";

function PlanPanel({
  projectId,
  onCancel,
  onCreated,
}: {
  projectId: string;
  onCancel: () => void;
  onCreated: (created: TaskDetail[]) => Promise<void> | void;
}) {
  const [idea, setIdea] = useState("");
  const [draft, setDraft] = useState<PlanDraft | null>(null);
  const [generating, setGenerating] = useState(false);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const busy = generating || creating;

  async function generate() {
    const trimmed = idea.trim();
    if (!trimmed || busy) return;
    setGenerating(true);
    setError(null);
    try {
      const next = await planProject({ project_id: projectId, idea: trimmed });
      setDraft(next);
      if (next.issues.length === 0) {
        setError("规划结果为空，请调整想法后重试。");
      }
    } catch (e) {
      setDraft(null);
      setError(errorText(e));
    } finally {
      setGenerating(false);
    }
  }

  async function create() {
    if (!draft || draft.issues.length === 0 || busy) return;
    setCreating(true);
    setError(null);
    try {
      const created = await createTasksFromPlan({
        project_id: projectId,
        draft,
      });
      await onCreated(created);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setCreating(false);
    }
  }

  const issueCount = draft?.issues.length ?? 0;

  return (
    <Card className="mb-3 max-w-[720px] rounded-[10px] p-3.5">
      <Card.Header>
        <Card.Title className="text-[15px] font-medium leading-[1.4]">
          规划任务
        </Card.Title>
        <Card.Description className="text-ui-sm">
          描述一个想法，规划 Agent 会拆成带验收标准与依赖的任务。
        </Card.Description>
      </Card.Header>
      <Card.Content className="grid gap-2">
        <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
          想法
          <TextArea
            aria-label="规划想法"
            rows={3}
            placeholder="例如：让项目支持按计划批量拆分任务并自动建立依赖。"
            className="text-ui"
            value={idea}
            onChange={(e) => {
              setIdea(e.target.value);
              setDraft(null);
              setError(null);
            }}
            disabled={busy}
          />
        </label>
        <div className="flex justify-end gap-1.5">
          <Button
            variant="ghost"
            size="sm"
            className="h-8 px-3 text-ui"
            onPress={onCancel}
            isDisabled={busy}
          >
            取消
          </Button>
          <Button
            variant="secondary"
            size="sm"
            className="h-8 px-3 text-ui"
            onPress={() => void generate()}
            isDisabled={busy || !idea.trim()}
          >
            {generating ? "生成中…" : "生成计划"}
          </Button>
        </div>
        {error && (
          <p role="alert" className="text-ui-sm text-[var(--danger)]">
            {error}
          </p>
        )}
        {draft && draft.issues.length > 0 && (
          <div className="grid gap-2">
            <div className="max-h-72 overflow-auto rounded-[10px] border border-[var(--border)]">
              {draft.issues.map((issue, index) => (
                <div
                  key={`${index}-${issue.title}`}
                  className="grid gap-1 border-b border-[var(--border)] px-2 py-1.5 last:border-b-0"
                >
                  <div className="flex items-start gap-1.5">
                    <span className="shrink-0 font-mono text-label text-[var(--muted)]">
                      {index + 1}.
                    </span>
                    <p
                      className="min-w-0 text-ui-sm font-medium"
                      title={issue.title}
                    >
                      {issue.title}
                    </p>
                  </div>
                  {issue.prompt && (
                    <p
                      className="line-clamp-2 text-ui-sm text-[var(--muted)]"
                      title={issue.prompt}
                    >
                      {issue.prompt}
                    </p>
                  )}
                  {issue.acceptance.length > 0 ? (
                    <details>
                      <summary className="cursor-pointer text-ui-sm text-[var(--muted)]">
                        验收 {issue.acceptance.length} 项
                      </summary>
                      <ul className="mt-1 grid gap-1">
                        {issue.acceptance.map((criterion, criterionIndex) => (
                          <li
                            key={`${criterion.id}-${criterionIndex}`}
                            className="grid gap-0.5 rounded border border-[var(--border)] px-1.5 py-1"
                          >
                            <code
                              className="min-w-0 truncate font-mono text-label"
                              title={criterion.cmd}
                            >
                              {criterion.cmd}
                            </code>
                            <span className="text-label text-[var(--muted)]">
                              期望退出码 {criterion.expect_exit}
                              {criterion.expect_match
                                ? ` · 匹配 ${criterion.expect_match}`
                                : ""}
                            </span>
                          </li>
                        ))}
                      </ul>
                    </details>
                  ) : (
                    <span className="text-ui-sm text-[var(--muted)]">
                      验收 0 项
                    </span>
                  )}
                  {issue.depends_on.length > 0 && (
                    <div className="flex flex-wrap items-center gap-1">
                      <span className="text-ui-sm text-[var(--muted)]">
                        依赖
                      </span>
                      {issue.depends_on.map((dep, depIndex) => (
                        <Chip
                          key={`${dep}-${depIndex}`}
                          size="sm"
                          variant="tertiary"
                          color="default"
                        >
                          {draft.issues[dep]?.title ?? `#${dep + 1}`}
                        </Chip>
                      ))}
                    </div>
                  )}
                </div>
              ))}
            </div>
            <div className="flex items-center justify-end">
              <Button
                variant="primary"
                size="sm"
                className="h-8 px-3 text-ui"
                onPress={() => void create()}
                isDisabled={busy || issueCount === 0}
              >
                {creating ? "创建中…" : `创建 ${issueCount} 个任务`}
              </Button>
            </div>
          </div>
        )}
      </Card.Content>
    </Card>
  );
}


export { PlanPanel };
