import { Network } from "lucide-react";
import type { VaultGraph } from "../../lib/api";
import { VaultGraphView } from "./VaultGraphView";

export function VaultGraphPane({
  graphData,
  graphLoading,
  graphError,
  onOpenNote,
}: {
  graphData: VaultGraph | null;
  graphLoading: boolean;
  graphError: string | null;
  onOpenNote: (id: string) => void;
}) {
  return (
    <>
      <header className="flex min-h-12 shrink-0 flex-wrap items-center gap-2 border-b border-[var(--border)] px-3 py-1.5">
        <div className="min-w-0 flex-1">
          <p className="text-ui font-medium">知识图谱</p>
          <p className="text-label text-[var(--muted)]">
            {graphData
              ? `${graphData.nodes.length} 个节点 · ${graphData.edges.length} 条链接`
              : "按 [[双链]] 构建"}
          </p>
        </div>
      </header>
      {graphError ? (
        <p role="alert" className="p-3 text-ui-sm text-[var(--danger)]">
          {graphError}
        </p>
      ) : graphLoading && !graphData ? (
        <p className="p-3 text-ui-sm text-[var(--muted)]">加载中…</p>
      ) : graphData && graphData.nodes.length > 0 ? (
        <div className="min-h-0 flex-1 p-3">
          <VaultGraphView graph={graphData} onOpen={onOpenNote} />
        </div>
      ) : (
        <div className="flex flex-1 flex-col items-center justify-center gap-2 text-center">
          <Network size={20} className="text-[var(--muted)] opacity-70" />
          <p className="text-ui text-[var(--muted)]">
            笔记里用 [[标题]] 互链后会在这里生成图谱。
          </p>
        </div>
      )}
    </>
  );
}
