import { useEffect, useState } from "react";
import { Button, Chip } from "@heroui/react";
import {
  AlertTriangle,
  Clock,
  Inbox,
  ListChecks,
  ShieldCheck,
  Terminal,
} from "lucide-react";
import {
  compactJson,
  decisionKindLabel,
  formatDate,
  statusTone,
  textList,
  verdictLabel,
  type AgentManifest,
  type ContextItem,
  type EventRow,
  type ProjectIssue,
  type TaskDependencies,
  type TaskDetail,
  type TaskSummary,
  type UpdateIssueInput,
} from "../../lib/api";
import { AttachmentSection } from "./AttachmentSection";
import { ContextSection } from "./detail/ContextSection";
import { DependenciesSection } from "./detail/DependenciesSection";
import { DetailHeader } from "./detail/DetailHeader";
import { GithubSection } from "./detail/GithubSection";
import { RunsSection } from "./detail/RunsSection";
import { Muted, Section, taskAgentInfo, type ContextOption } from "./shared";

export function DetailPane({
  detail,
  events,
  tasks,
  attachmentsRevision,
  onAttachmentsChanged,
  adapters,
  adaptersLoading,
  adaptersError,
  onSaveAgents,
  onSaveModel,
  dependencies,
  dependencyError,
  addingDependency,
  removingDependencyId,
  contextItems,
  contextError,
  contextOptions,
  contextOptionsLoading,
  contextOptionsError,
  linkingContext,
  removingContextId,
  issue,
  issuesLoading,
  issuesError,
  githubError,
  onUpdateIssue,
  syncing,
  publishing,
  loading,
  busy,
  cancelling,
  cancelError,
  resolvingDecisionId,
  onResolveDecision,
  onAddDependency,
  onRemoveDependency,
  onLinkContext,
  onUnlinkContext,
  onSync,
  onPublish,
  onStart,
  onCancel,
  onClose,
}: {
  detail: TaskDetail | null;
  events: EventRow[];
  tasks: TaskSummary[];
  attachmentsRevision: number;
  onAttachmentsChanged: () => void;
  adapters: AgentManifest[];
  adaptersLoading: boolean;
  adaptersError: string | null;
  onSaveAgents: (
    developer: string | null,
    reviewer: string | null,
  ) => Promise<string | null>;
  onSaveModel: (model: string | null) => Promise<string | null>;
  dependencies: TaskDependencies | null;
  dependencyError: string | null;
  addingDependency: boolean;
  removingDependencyId: string | null;
  contextItems: ContextItem[] | null;
  contextError: string | null;
  contextOptions: ContextOption[];
  contextOptionsLoading: boolean;
  contextOptionsError: string | null;
  linkingContext: boolean;
  removingContextId: string | null;
  issue: ProjectIssue | null;
  issuesLoading: boolean;
  issuesError: string | null;
  githubError: string | null;
  onUpdateIssue: (input: UpdateIssueInput) => Promise<string | null>;
  syncing: boolean;
  publishing: boolean;
  loading: boolean;
  busy: boolean;
  cancelling: boolean;
  cancelError: string | null;
  resolvingDecisionId: string | null;
  onResolveDecision: (decisionId: string, resolution: "allow" | "deny") => void;
  onAddDependency: (dependsOnTaskId: string) => Promise<boolean>;
  onRemoveDependency: (dependsOnTaskId: string) => void;
  onLinkContext: (itemId: string) => Promise<boolean>;
  onUnlinkContext: (itemId: string) => void;
  onSync: () => void;
  onPublish: () => void;
  onStart: () => void;
  onCancel: () => void;
  onClose: () => void;
}) {
  const [dependsOnKey, setDependsOnKey] = useState<string | null>(null);
  const [contextKey, setContextKey] = useState<string | null>(null);
  const [prLinkError, setPrLinkError] = useState<string | null>(null);
  const detailTaskId = detail?.task.id ?? null;
  const savedDeveloper = detail ? taskAgentInfo(detail.task).developer : null;
  const savedReviewer = detail ? taskAgentInfo(detail.task).reviewer : null;
  const savedModel = detail ? (detail.task.model ?? null) : null;
  useEffect(() => {
    setDependsOnKey(null);
    setContextKey(null);
    setPrLinkError(null);
  }, [detailTaskId, savedDeveloper, savedReviewer, savedModel]);

  const review = detail?.review ?? null;
  const riskFlags = review ? textList(review.risk_flags) : [];
  const testDiffs = review ? textList(review.test_diff) : [];
  const openDecisions =
    detail?.decisions.filter((decision) => decision.status === "open") ?? [];

  return (
    <aside className="absolute inset-y-0 right-0 z-10 flex h-full w-[400px] max-w-full shrink-0 flex-col overflow-hidden rounded-[10px] border border-[var(--border)] bg-[var(--background)] shadow-[0_8px_24px_rgba(0,0,0,0.12)] xl:static xl:w-[380px] xl:shadow-none">
      <DetailHeader
        detail={detail}
        adapters={adapters}
        adaptersLoading={adaptersLoading}
        adaptersError={adaptersError}
        onSaveAgents={onSaveAgents}
        onSaveModel={onSaveModel}
        busy={busy}
        cancelling={cancelling}
        onCancel={onCancel}
        onStart={onStart}
        onClose={onClose}
      />

      {cancelError && (
        <p
          role="alert"
          className="shrink-0 border-b border-[var(--border)] px-3 py-1.5 text-ui-sm text-[var(--danger)]"
        >
          {cancelError}
        </p>
      )}

      {!detail ? (
        <p className="p-3 text-ui-sm text-[var(--muted)]">
          {loading ? "加载中…" : "未找到任务详情。"}
        </p>
      ) : (
        <div className="min-h-0 flex-1 overflow-auto px-3.5 py-3.5">
          <div className="grid gap-4">
            <div>
              <p className="text-label font-medium uppercase tracking-wide text-[var(--muted)]">
                意图
              </p>
              <p className="mt-1 whitespace-pre-wrap break-words text-ui-sm">
                {detail.task.prompt}
              </p>
            </div>

            <GithubSection
              issue={issue}
              issuesLoading={issuesLoading}
              issuesError={issuesError}
              githubError={githubError}
              onUpdateIssue={onUpdateIssue}
              syncing={syncing}
              publishing={publishing}
              onSync={onSync}
              onPublish={onPublish}
            />

            <DependenciesSection
              detailTaskId={detail.task.id}
              status={detail.task.status}
              tasks={tasks}
              dependencies={dependencies}
              dependencyError={dependencyError}
              addingDependency={addingDependency}
              removingDependencyId={removingDependencyId}
              dependsOnKey={dependsOnKey}
              setDependsOnKey={setDependsOnKey}
              onAddDependency={onAddDependency}
              onRemoveDependency={onRemoveDependency}
            />

            <ContextSection
              contextItems={contextItems}
              contextError={contextError}
              contextOptions={contextOptions}
              contextOptionsLoading={contextOptionsLoading}
              contextOptionsError={contextOptionsError}
              linkingContext={linkingContext}
              removingContextId={removingContextId}
              contextKey={contextKey}
              setContextKey={setContextKey}
              onLinkContext={onLinkContext}
              onUnlinkContext={onUnlinkContext}
            />

            {openDecisions.length > 0 && (
              <Section
                title="待决策"
                icon={<Inbox size={14} />}
                count={openDecisions.length}
              >
                <div className="overflow-hidden rounded-[10px] border border-[var(--border)]">
                  {openDecisions.map((decision) => (
                    <div
                      key={decision.id}
                      className="flex items-start gap-2 border-b border-[var(--border)] px-2 py-1.5 last:border-b-0"
                    >
                      <div className="min-w-0 flex-1">
                        <p className="whitespace-pre-wrap break-words text-ui-sm">
                          {decision.prompt}
                        </p>
                        <div className="mt-1 flex flex-wrap items-center gap-1.5 text-label text-[var(--muted)]">
                          <Chip
                            size="sm"
                            variant="tertiary"
                            color="warning"
                          >
                            {decisionKindLabel(decision.kind)}
                          </Chip>
                          <span>{formatDate(decision.created_at)}</span>
                        </div>
                      </div>
                      <div className="flex shrink-0 items-center gap-1">
                        <Button
                          variant="primary"
                          size="sm"
                          className="h-8 px-3 text-ui"
                          onPress={() =>
                            onResolveDecision(decision.id, "allow")
                          }
                          isDisabled={resolvingDecisionId !== null}
                        >
                          {resolvingDecisionId === decision.id
                            ? "处理中…"
                            : "允许"}
                        </Button>
                        <Button
                          variant="danger-soft"
                          size="sm"
                          className="h-8 px-3 text-ui"
                          onPress={() =>
                            onResolveDecision(decision.id, "deny")
                          }
                          isDisabled={resolvingDecisionId !== null}
                        >
                          拒绝
                        </Button>
                      </div>
                    </div>
                  ))}
                </div>
              </Section>
            )}

            <Section
              title="验收标准"
              icon={<ListChecks size={14} />}
              count={detail.criteria.length}
            >
              {detail.criteria.length === 0 ? (
                <Muted>暂无验收标准。</Muted>
              ) : (
                <div className="overflow-hidden rounded-[10px] border border-[var(--border)]">
                  {detail.criteria.map((criterion) => (
                    <div
                      key={criterion.id}
                      className="grid gap-1 border-b border-[var(--border)] px-2 py-1.5 last:border-b-0"
                    >
                      <p
                        className="min-w-0 truncate font-mono text-ui-sm"
                        title={criterion.cmd}
                      >
                        {criterion.cmd}
                      </p>
                      <div className="flex items-center gap-2 text-label text-[var(--muted)]">
                        <span className="shrink-0 font-mono">
                          {criterion.id}
                        </span>
                        <span className="shrink-0">
                          期望退出码 {criterion.expect_exit}
                        </span>
                        {criterion.expect_match && (
                          <span
                            className="min-w-0 truncate"
                            title={criterion.expect_match}
                          >
                            匹配 {criterion.expect_match}
                          </span>
                        )}
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </Section>

            <RunsSection
              runs={detail.runs}
              prLinkError={prLinkError}
              setPrLinkError={setPrLinkError}
            />

            <Section
              title="证据"
              icon={<Terminal size={14} />}
              count={detail.evidence.length}
            >
              {detail.evidence.length === 0 ? (
                <Muted>暂无证据。</Muted>
              ) : (
                <div className="overflow-hidden rounded-[10px] border border-[var(--border)]">
                  {detail.evidence.map((item) => (
                    <div
                      key={item.id}
                      className="flex items-start gap-2 border-b border-[var(--border)] px-2 py-1.5 last:border-b-0"
                    >
                      <div className="min-w-0 flex-1">
                        <p
                          className="truncate font-mono text-ui-sm"
                          title={
                            item.cmd ?? item.output_path ?? item.kind
                          }
                        >
                          {item.cmd ?? item.output_path ?? item.kind}
                        </p>
                        <p className="text-label text-[var(--muted)]">
                          {item.kind}
                          {item.criterion_id
                            ? ` · ${item.criterion_id}`
                            : ""}
                          {item.duration_ms != null
                            ? ` · ${item.duration_ms}ms`
                            : ""}
                        </p>
                      </div>
                      <Chip
                        size="sm"
                        variant="soft"
                        color={
                          item.exit_code === 0
                            ? "success"
                            : item.exit_code == null
                              ? "default"
                              : "danger"
                        }
                      >
                        {item.exit_code == null
                          ? "—"
                          : `exit ${item.exit_code}`}
                      </Chip>
                    </div>
                  ))}
                </div>
              )}
            </Section>

            <AttachmentSection
              key={detail.task.id}
              taskId={detail.task.id}
              revision={attachmentsRevision}
              onChanged={onAttachmentsChanged}
            />

            <Section title="审查" icon={<ShieldCheck size={14} />}>
              {!review ? (
                <Muted>尚未审查。</Muted>
              ) : (
                <div className="grid gap-2 rounded-lg border border-[var(--border)] px-2.5 py-2">
                  <div className="flex items-center gap-1.5">
                    <Chip
                      size="sm"
                      variant="soft"
                      color={statusTone(review.verdict)}
                    >
                      {verdictLabel(review.verdict)}
                    </Chip>
                    <span className="text-ui-sm text-[var(--muted)]">
                      风险标记 {riskFlags.length}
                    </span>
                  </div>
                  {review.summary && (
                    <p className="whitespace-pre-wrap break-words text-ui-sm">
                      {review.summary}
                    </p>
                  )}
                  {riskFlags.length > 0 && (
                    <ul className="grid gap-1">
                      {riskFlags.map((flag, index) => (
                        <li
                          key={`${flag}-${index}`}
                          className="flex items-start gap-1.5 text-ui-sm text-[var(--danger)]"
                        >
                          <AlertTriangle
                            size={14}
                            className="mt-0.5 shrink-0"
                          />
                          <span className="break-words">{flag}</span>
                        </li>
                      ))}
                    </ul>
                  )}
                  {testDiffs.length > 0 && (
                    <div className="grid gap-0.5">
                      <p className="text-label text-[var(--muted)]">
                        测试/CI 改动
                      </p>
                      {testDiffs.map((diff, index) => (
                        <p
                          key={index}
                          className="break-words font-mono text-label text-[var(--muted)]"
                        >
                          {diff}
                        </p>
                      ))}
                    </div>
                  )}
                </div>
              )}
            </Section>

            <Section
              title="事件"
              icon={<Clock size={14} />}
              count={events.length}
            >
              {events.length === 0 ? (
                <Muted>暂无事件。</Muted>
              ) : (
                <div className="grid gap-1">
                  {events.map((event) => (
                    <div
                      key={event.id}
                      className="rounded-lg border border-[var(--border)] px-2 py-1"
                    >
                      <div className="flex items-center justify-between gap-2">
                        <span className="min-w-0 truncate text-ui-sm">
                          {event.kind}
                        </span>
                        <span className="shrink-0 text-label text-[var(--muted)]">
                          {formatDate(event.at)}
                        </span>
                      </div>
                      {event.payload_json && event.payload_json !== "{}" && (
                        <p
                          className="truncate font-mono text-label text-[var(--muted)]"
                          title={event.payload_json}
                        >
                          {compactJson(event.payload_json)}
                        </p>
                      )}
                    </div>
                  ))}
                </div>
              )}
            </Section>
          </div>
        </div>
      )}
    </aside>
  );
}
