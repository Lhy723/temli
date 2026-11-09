import { useCallback, useEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Button, Chip } from "@heroui/react";
import {
  ChevronDown,
  ChevronRight,
  ExternalLink,
  FileText,
  ListChecks,
  RefreshCw,
  ShieldCheck,
} from "lucide-react";
import {
  errorText,
  formatDate,
  listReviews,
  statusTone,
  textList,
  verdictLabel,
  type ReviewListItem,
} from "../lib/api";

type ReviewFilter = "all" | "pass" | "issues";

const FILTERS: { id: ReviewFilter; label: string }[] = [
  { id: "all", label: "全部" },
  { id: "pass", label: "通过" },
  { id: "issues", label: "有问题" },
];

function matchesFilter(review: ReviewListItem, filter: ReviewFilter): boolean {
  if (filter === "pass") return review.verdict === "pass";
  if (filter === "issues") return review.verdict !== "pass";
  return true;
}

export function ReviewsPanel({ projectId }: { projectId: string }) {
  const [reviews, setReviews] = useState<ReviewListItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState<ReviewFilter>("all");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [prLinkErrors, setPrLinkErrors] = useState<Record<string, string>>({});
  const request = useRef(0);

  const load = useCallback(async (): Promise<void> => {
    const current = ++request.current;
    setLoading(true);
    setError(null);
    try {
      const rows = await listReviews(projectId);
      if (request.current !== current) return;
      setReviews(rows);
    } catch (e) {
      if (request.current !== current) return;
      setError(errorText(e));
    } finally {
      if (request.current === current) setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    setReviews([]);
    setExpanded(new Set());
    setFilter("all");
    setPrLinkErrors({});
    void load();
    return () => {
      request.current += 1;
    };
  }, [load]);

  function clearPrLinkError(runId: string) {
    setPrLinkErrors((prev) => {
      if (!(runId in prev)) return prev;
      const next = { ...prev };
      delete next[runId];
      return next;
    });
  }

  function toggle(runId: string) {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(runId)) next.delete(runId);
      else next.add(runId);
      return next;
    });
  }

  const counts: Record<ReviewFilter, number> = {
    all: reviews.length,
    pass: reviews.filter((review) => review.verdict === "pass").length,
    issues: reviews.filter((review) => review.verdict !== "pass").length,
  };
  const visible = reviews.filter((review) => matchesFilter(review, filter));

  return (
    <section className="flex h-full min-h-0 flex-col">
      <header className="shrink-0 pb-2">
        <div className="flex items-center justify-between gap-2">
          <div className="flex items-center gap-1.5">
            <ShieldCheck size={15} className="text-[var(--muted)]" />
            <h2 className="text-ui font-medium">审查</h2>
          </div>
          <Button
            isIconOnly
            variant="ghost"
            size="sm"
            className="h-6 w-6"
            aria-label="刷新审查"
            isDisabled={loading}
            onPress={() => void load()}
          >
            <RefreshCw size={14} />
          </Button>
        </div>
        <p className="mt-0.5 text-ui-sm text-[var(--muted)]">
          任务完成后的证据与独立审查报告
        </p>
        <div className="mt-2 flex flex-wrap items-center gap-1">
          {FILTERS.map((item) => (
            <Button
              key={item.id}
              variant={filter === item.id ? "secondary" : "ghost"}
              size="sm"
              className="h-6 gap-1 rounded-full border border-[var(--border)] px-2.5 text-ui-sm"
              aria-pressed={filter === item.id}
              onPress={() => setFilter(item.id)}
            >
              {item.label}
              <span className="text-label text-[var(--muted)]">
                {counts[item.id]}
              </span>
            </Button>
          ))}
        </div>
      </header>

      {error && (
        <div className="flex shrink-0 items-center gap-2 pb-2">
          <p role="alert" className="text-ui-sm text-[var(--danger)]">
            审查加载失败：{error}
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
        {loading && (
          <p className="text-ui-sm text-[var(--muted)]">加载中…</p>
        )}

        {!loading && !error && reviews.length === 0 && (
          <div className="flex h-full flex-col items-center justify-center gap-2 text-center">
            <ShieldCheck
              size={20}
              className="text-[var(--muted)] opacity-70"
            />
            <p className="text-ui text-[var(--muted)]">
              还没有审查报告。任务完成后，证据与独立审查会出现在这里。
            </p>
          </div>
        )}

        {!loading && !error && reviews.length > 0 && visible.length === 0 && (
          <div className="flex flex-col items-center gap-2 py-6 text-center">
            <p className="text-ui-sm text-[var(--muted)]">
              没有符合当前筛选的审查报告。
            </p>
            <Button
              variant="secondary"
              size="sm"
              className="h-7 px-3 text-ui"
              onPress={() => setFilter("all")}
            >
              查看全部
            </Button>
          </div>
        )}

        {!loading && visible.length > 0 && (
          <div className="flex flex-col gap-1.5">
            {visible.map((review) => {
              const isExpanded = expanded.has(review.run_id);
              const riskFlags = textList(review.risk_flags);
              const testDiffs = textList(review.test_diff);
              return (
                <article
                  key={review.run_id}
                  className="overflow-hidden rounded-md border border-[var(--border)]"
                >
                  <button
                    type="button"
                    className="anim-fast flex w-full items-start gap-2 px-3 py-2 text-left hover:bg-[var(--background-secondary)]"
                    aria-expanded={isExpanded}
                    aria-controls={`review-detail-${review.run_id}`}
                    onClick={() => toggle(review.run_id)}
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
                          title={review.task_title}
                        >
                          {review.task_title}
                        </span>
                        <Chip
                          size="sm"
                          variant="soft"
                          color={statusTone(review.verdict)}
                        >
                          {verdictLabel(review.verdict)}
                        </Chip>
                        <span className="ml-auto shrink-0 text-label text-[var(--muted)]">
                          {formatDate(review.created_at)}
                        </span>
                      </div>
                      <div className="mt-1 flex flex-wrap items-center gap-1.5 text-label text-[var(--muted)]">
                        <span className="flex items-center gap-1">
                          <ListChecks size={12} />
                          {review.evidence_total} 条验收证据 ·{" "}
                          {review.evidence_passed} 退出 0
                        </span>
                        {riskFlags.map((flag, index) => (
                          <Chip
                            key={`${flag}-${index}`}
                            size="sm"
                            variant="tertiary"
                            color="warning"
                          >
                            {flag}
                          </Chip>
                        ))}
                      </div>
                    </div>
                  </button>

                  {isExpanded && (
                    <div
                      id={`review-detail-${review.run_id}`}
                      className="grid gap-2 border-t border-[var(--border)] px-3 py-2"
                    >
                      <div className="grid gap-0.5">
                        <p className="text-label text-[var(--muted)]">
                          审查摘要
                        </p>
                        {review.summary ? (
                          <p className="whitespace-pre-wrap break-words text-ui-sm">
                            {review.summary}
                          </p>
                        ) : (
                          <p className="text-ui-sm text-[var(--muted)]">
                            没有摘要。
                          </p>
                        )}
                      </div>

                      <div className="grid gap-0.5">
                        <p className="text-label text-[var(--muted)]">
                          测试 / CI 改动
                        </p>
                        {testDiffs.length > 0 ? (
                          testDiffs.map((diff, index) => (
                            <p
                              key={index}
                              className="flex items-start gap-1.5 break-words font-mono text-label text-[var(--muted)]"
                            >
                              <FileText size={12} className="mt-0.5 shrink-0" />
                              {diff}
                            </p>
                          ))
                        ) : (
                          <p className="text-ui-sm text-[var(--muted)]">
                            无测试改动。
                          </p>
                        )}
                      </div>

                      {review.pr_url && (
                        <div className="flex flex-wrap items-center gap-2">
                          <Button
                            variant="ghost"
                            size="sm"
                            className="h-6 gap-1 px-1.5 text-label"
                            onPress={() => {
                              const runId = review.run_id;
                              clearPrLinkError(runId);
                              void openUrl(review.pr_url!).catch((e) =>
                                setPrLinkErrors((prev) => ({
                                  ...prev,
                                  [runId]: errorText(e),
                                })),
                              );
                            }}
                          >
                            <ExternalLink size={12} />
                            打开 PR
                          </Button>
                          {prLinkErrors[review.run_id] && (
                            <p
                              role="alert"
                              className="text-label text-[var(--danger)]"
                            >
                              {prLinkErrors[review.run_id]}
                            </p>
                          )}
                        </div>
                      )}
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
