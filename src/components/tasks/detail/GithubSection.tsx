import { Button, Chip } from "@heroui/react";
import { GitPullRequest, RefreshCw, Upload } from "lucide-react";
import {
  statusLabel,
  statusTone,
  type ProjectIssue,
  type UpdateIssueInput,
} from "../../../lib/api";
import { IssueEditControls } from "../IssueEditor";
import { Muted, Section } from "../shared";

export function GithubSection({
  issue,
  issuesLoading,
  issuesError,
  githubError,
  onUpdateIssue,
  syncing,
  publishing,
  onSync,
  onPublish,
}: {
  issue: ProjectIssue | null;
  issuesLoading: boolean;
  issuesError: string | null;
  githubError: string | null;
  onUpdateIssue: (input: UpdateIssueInput) => Promise<string | null>;
  syncing: boolean;
  publishing: boolean;
  onSync: () => void;
  onPublish: () => void;
}) {
  return (
            <Section title="GitHub" icon={<GitPullRequest size={14} />}>
              <div className="flex flex-wrap items-center gap-1.5">
                <Button
                  variant="secondary"
                  size="sm"
                  className="h-7 gap-1 px-2.5 text-ui"
                  onPress={onSync}
                  isDisabled={syncing}
                >
                  <RefreshCw
                    size={14}
                    className={syncing ? "animate-spin" : undefined}
                  />
                  {syncing ? "同步中…" : "同步"}
                </Button>
                <Button
                  variant="secondary"
                  size="sm"
                  className="h-7 gap-1 px-2.5 text-ui"
                  onPress={onPublish}
                  isDisabled={
                    !issue || publishing || issue.github_number != null
                  }
                >
                  <Upload size={14} />
                  {publishing ? "发布中…" : "发布到 GitHub"}
                </Button>
              </div>
              {(githubError || issuesError) && (
                <p role="alert" className="text-ui-sm text-[var(--danger)]">
                  {githubError ?? issuesError}
                </p>
              )}
              {issuesLoading && !issue ? (
                <Muted>正在加载 GitHub 关联…</Muted>
              ) : issue && issue.github_number != null ? (
                <div className="grid gap-1 rounded-lg border border-[var(--border)] px-2.5 py-2">
                  <div className="flex flex-wrap items-center gap-1.5">
                    <span className="font-mono text-ui-sm">
                      #{issue.github_number}
                    </span>
                    <Chip
                      size="sm"
                      variant="soft"
                      color={statusTone(issue.github_state ?? "")}
                    >
                      {issue.github_state
                        ? statusLabel(issue.github_state)
                        : "未知状态"}
                    </Chip>
                    {issue.ghost && (
                      <Chip size="sm" variant="soft" color="warning">
                        远端已删除
                      </Chip>
                    )}
                  </div>
                  <p
                    className="truncate text-ui-sm text-[var(--muted)]"
                    title={issue.title}
                  >
                    {issue.title}
                  </p>
                  {!issue.ghost && (
                    <IssueEditControls
                      key={issue.id}
                      issue={issue}
                      onSave={onUpdateIssue}
                    />
                  )}
                </div>
              ) : issue ? (
                <Muted>尚未发布到 GitHub。</Muted>
              ) : (
                <Muted>未找到关联的 Issue 条目。</Muted>
              )}
            </Section>
  );
}
