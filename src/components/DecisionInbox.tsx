import { useCallback, useEffect, useRef, useState } from "react";
import { Button, Chip } from "@heroui/react";
import { Inbox, RefreshCw } from "lucide-react";
import {
  decisionKindLabel,
  errorText,
  formatDate,
  listDecisions,
  resolveDecision,
  type DecisionRow,
} from "../lib/api";

const POLL_INTERVAL_MS = 2000;

export function DecisionInbox({ projectId }: { projectId: string }) {
  const [decisions, setDecisions] = useState<DecisionRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const inFlight = useRef<Promise<void> | null>(null);
  const generation = useRef(0);

  // 同一个 project 的请求串行化：轮询遇到在途请求直接跳过，手动刷新/决策后则等待在途请求再拉一次。
  const load = useCallback(
    async (waitForInFlight = false): Promise<void> => {
      const current = generation.current;
      while (inFlight.current) {
        if (!waitForInFlight || generation.current !== current) return;
        const pending = inFlight.current;
        await pending.catch(() => undefined);
      }
      if (generation.current !== current) return;
      const request = (async () => {
        try {
          const rows = await listDecisions(projectId);
          if (generation.current !== current) return;
          setDecisions(rows);
          setError(null);
        } catch (e) {
          if (generation.current !== current) return;
          setError(errorText(e));
        }
      })();
      inFlight.current = request;
      try {
        await request;
      } finally {
        if (inFlight.current === request) inFlight.current = null;
      }
    },
    [projectId],
  );

  useEffect(() => {
    let active = true;
    generation.current += 1;
    inFlight.current = null;
    setLoading(true);
    setError(null);
    setDecisions([]);
    setBusyId(null);

    const current = generation.current;
    void load().finally(() => {
      if (active && generation.current === current) setLoading(false);
    });

    const timer = window.setInterval(() => {
      if (active) void load();
    }, POLL_INTERVAL_MS);

    return () => {
      active = false;
      generation.current += 1;
      inFlight.current = null;
      window.clearInterval(timer);
    };
  }, [projectId, load]);

  async function resolve(id: string, resolution: "allow" | "deny") {
    const startingGeneration = generation.current;
    setBusyId(id);
    setError(null);
    let failure: string | null = null;
    try {
      await resolveDecision(id, resolution);
    } catch (e) {
      failure = errorText(e);
    } finally {
      if (generation.current !== startingGeneration) return;
      await load(true);
      if (generation.current !== startingGeneration) return;
      if (failure) setError(failure);
      setBusyId(null);
    }
  }

  const open = decisions.filter((decision) => decision.status === "open");

  return (
    <section className="overflow-hidden rounded-md border border-[var(--border)]">
      <header className="flex h-9 items-center justify-between gap-2 border-b border-[var(--border)] px-3">
        <span className="flex items-center gap-1.5 text-ui font-medium">
          <Inbox size={14} className="text-[var(--muted)]" />
          决策收件箱
          {open.length > 0 && (
            <Chip size="sm" variant="soft" color="warning">
              {open.length}
            </Chip>
          )}
        </span>
        <Button
          isIconOnly
          variant="ghost"
          size="sm"
          className="h-6 w-6"
          aria-label="刷新决策"
          onPress={() => void load(true)}
          isDisabled={loading}
        >
          <RefreshCw size={14} />
        </Button>
      </header>

      {error && (
        <p role="alert" className="px-3 pt-2 text-ui-sm text-[var(--danger)]">
          {error}
        </p>
      )}

      {loading && (
        <p className="px-3 py-2 text-ui-sm text-[var(--muted)]">加载中…</p>
      )}

      {!loading && !error && open.length === 0 && (
        <div className="flex items-center gap-2 px-3 py-3">
          <Inbox size={14} className="text-[var(--muted)] opacity-70" />
          <p className="text-ui-sm text-[var(--muted)]">
            没有待决策事项。高风险变更与审批请求会出现在这里。
          </p>
        </div>
      )}

      {!loading &&
        open.map((decision) => (
          <div
            key={decision.id}
            className="flex items-start gap-3 border-b border-[var(--border)] px-3 py-2 last:border-b-0"
          >
            <div className="min-w-0 flex-1">
              <p className="whitespace-pre-wrap break-words text-ui">
                {decision.prompt}
              </p>
              <div className="mt-1 flex flex-wrap items-center gap-1.5 text-label text-[var(--muted)]">
                <Chip size="sm" variant="tertiary" color="warning">
                  {decisionKindLabel(decision.kind)}
                </Chip>
                {decision.task_id && (
                  <span className="font-mono">
                    {decision.task_id.slice(0, 8)}
                  </span>
                )}
                <span>{formatDate(decision.created_at)}</span>
              </div>
            </div>
            <div className="flex shrink-0 items-center gap-1">
              <Button
                variant="primary"
                size="sm"
                className="h-7 px-2.5 text-ui"
                onPress={() => void resolve(decision.id, "allow")}
                isDisabled={busyId !== null}
              >
                {busyId === decision.id ? "处理中…" : "允许"}
              </Button>
              <Button
                variant="danger-soft"
                size="sm"
                className="h-7 px-2.5 text-ui"
                onPress={() => void resolve(decision.id, "deny")}
                isDisabled={busyId !== null}
              >
                拒绝
              </Button>
            </div>
          </div>
        ))}
    </section>
  );
}
