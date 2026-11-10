import { Button, Chip } from "@heroui/react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ExternalLink, GitBranch, GitPullRequest } from "lucide-react";
import {
  errorText,
  formatDate,
  statusLabel,
  statusTone,
  type TaskDetail,
} from "../../../lib/api";
import { Muted, Section } from "../shared";

export function RunsSection({
  runs,
  prLinkError,
  setPrLinkError,
}: {
  runs: TaskDetail["runs"];
  prLinkError: string | null;
  setPrLinkError: (message: string | null) => void;
}) {
  return (
            <Section
              title="运行"
              icon={<GitBranch size={14} />}
              count={runs.length}
            >
              {runs.length === 0 ? (
                <Muted>暂无运行记录。</Muted>
              ) : (
                <div className="grid gap-1.5">
                  {runs.map((run) => (
                    <div
                      key={run.id}
                      className="rounded-lg border border-[var(--border)] px-2.5 py-2"
                    >
                      <div className="flex items-center gap-1.5">
                        <Chip
                          size="sm"
                          variant="soft"
                          color={statusTone(run.status)}
                        >
                          {statusLabel(run.status)}
                        </Chip>
                        <span className="min-w-0 truncate font-mono text-ui-sm text-[var(--muted)]">
                          {run.adapter_id}
                        </span>
                        <span className="ml-auto shrink-0 text-label text-[var(--muted)]">
                          {formatDate(run.started_at)}
                        </span>
                      </div>
                      {run.branch && (
                        <p
                          className="mt-1 truncate font-mono text-label text-[var(--muted)]"
                          title={run.branch}
                        >
                          {run.branch}
                        </p>
                      )}
                      {run.worktree_path && (
                        <p
                          className="truncate font-mono text-label text-[var(--muted)]"
                          title={run.worktree_path}
                        >
                          {run.worktree_path}
                        </p>
                      )}
                      {(run.pr_number != null ||
                        run.pr_url ||
                        run.pr_state) && (
                        <div className="mt-1 flex flex-wrap items-center gap-1.5">
                          <GitPullRequest
                            size={12}
                            className="shrink-0 text-[var(--muted)]"
                          />
                          {run.pr_number != null && (
                            <span className="font-mono text-label text-[var(--muted)]">
                              PR #{run.pr_number}
                            </span>
                          )}
                          {run.pr_state && (
                            <Chip
                              size="sm"
                              variant="soft"
                              color={statusTone(run.pr_state ?? "")}
                            >
                              {statusLabel(run.pr_state ?? "")}
                            </Chip>
                          )}
                          {run.pr_url && (
                            <Button
                              variant="ghost"
                              size="sm"
                              className="h-6 gap-1 px-1.5 text-label"
                              onPress={() => {
                                setPrLinkError(null);
                                void openUrl(run.pr_url!).catch((e) =>
                                  setPrLinkError(errorText(e)),
                                );
                              }}
                            >
                              <ExternalLink size={12} />
                              打开 PR
                            </Button>
                          )}
                        </div>
                      )}
                      {run.error && (
                        <p className="mt-1 whitespace-pre-wrap break-words text-ui-sm text-[var(--danger)]">
                          {run.error}
                        </p>
                      )}
                    </div>
                  ))}
                </div>
              )}
              {prLinkError && (
                <p role="alert" className="text-ui-sm text-[var(--danger)]">
                  {prLinkError}
                </p>
              )}
            </Section>
  );
}
