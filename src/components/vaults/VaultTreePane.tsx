import { Tag, X } from "lucide-react";
import type { VaultEntry, VaultHit } from "../../lib/api";
import { noteTitle, type TreeNode } from "./shared";
import { TreeRows } from "./VaultTree";

export function VaultTreePane({
  searching,
  searchLoading,
  searchError,
  searchHits,
  selectedNoteId,
  onOpenNote,
  activeTag,
  tagEntries,
  tagEntriesLoading,
  tagEntriesError,
  onClearTag,
  treeLoading,
  entries,
  tree,
  collapsed,
  onToggleFolder,
  onStartCreate,
}: {
  searching: boolean;
  searchLoading: boolean;
  searchError: string | null;
  searchHits: VaultHit[];
  selectedNoteId: string | null;
  onOpenNote: (id: string) => void;
  activeTag: string | null;
  tagEntries: VaultEntry[];
  tagEntriesLoading: boolean;
  tagEntriesError: string | null;
  onClearTag: () => void;
  treeLoading: boolean;
  entries: VaultEntry[];
  tree: TreeNode[];
  collapsed: Set<string>;
  onToggleFolder: (path: string) => void;
  onStartCreate: (folder: string) => void;
}) {
  return (
    <div className="min-h-0 flex-1 overflow-auto rounded-md border border-[var(--border)]">
      {searching ? (
        searchLoading ? (
          <p className="px-3 py-2 text-ui-sm text-[var(--muted)]">
            搜索中…
          </p>
        ) : searchError ? (
          <p
            role="alert"
            className="px-3 py-2 text-ui-sm text-[var(--danger)]"
          >
            {searchError}
          </p>
        ) : searchHits.length === 0 ? (
          <p className="px-3 py-2 text-ui-sm text-[var(--muted)]">
            没有匹配的笔记。
          </p>
        ) : (
          searchHits.map((hit) => (
            <button
              key={hit.id}
              type="button"
              onClick={() => onOpenNote(hit.id)}
              className={`anim-fast block w-full border-b border-[var(--border)] px-3 py-2 text-left last:border-b-0 hover:bg-[var(--background-secondary)] ${
                selectedNoteId === hit.id
                  ? "bg-[var(--background-secondary)]"
                  : ""
              }`}
            >
              <span className="block truncate text-ui">
                {hit.title || noteTitle(hit.path)}
              </span>
              <span className="block truncate font-mono text-label text-[var(--muted)]">
                {hit.path}
              </span>
              {hit.snippet && (
                <span className="mt-0.5 line-clamp-2 text-ui-sm text-[var(--muted)]">
                  {hit.snippet}
                </span>
              )}
            </button>
          ))
        )
      ) : activeTag !== null ? (
        <>
          <div className="flex h-8 items-center gap-1 border-b border-[var(--border)] px-2.5 text-ui-sm">
            <Tag size={12} className="shrink-0 text-[var(--muted)]" />
            <span className="min-w-0 truncate">#{activeTag}</span>
            <span className="ml-auto shrink-0 text-label text-[var(--muted)]">
              {tagEntriesLoading ? "…" : tagEntries.length}
            </span>
            <button
              type="button"
              aria-label="清除标签筛选"
              title="清除标签筛选"
              onClick={onClearTag}
              className="anim-fast flex size-5 shrink-0 items-center justify-center rounded text-[var(--muted)] hover:bg-[var(--background-tertiary)] hover:text-[var(--foreground)]"
            >
              <X size={12} />
            </button>
          </div>
          {tagEntriesError ? (
            <p
              role="alert"
              className="px-3 py-2 text-ui-sm text-[var(--danger)]"
            >
              {tagEntriesError}
            </p>
          ) : tagEntriesLoading && tagEntries.length === 0 ? (
            <p className="px-3 py-2 text-ui-sm text-[var(--muted)]">
              加载中…
            </p>
          ) : tagEntries.length === 0 ? (
            <p className="px-3 py-2 text-ui-sm text-[var(--muted)]">
              该标签下还没有笔记。
            </p>
          ) : (
            tagEntries.map((entry) => (
              <button
                key={entry.path}
                type="button"
                onClick={() => entry.id && onOpenNote(entry.id)}
                disabled={!entry.id}
                className={`anim-fast block w-full border-b border-[var(--border)] px-3 py-2 text-left last:border-b-0 hover:bg-[var(--background-secondary)] ${
                  selectedNoteId === entry.id
                    ? "bg-[var(--background-secondary)]"
                    : ""
                }`}
                title={entry.path}
              >
                <span className="block truncate text-ui">
                  {entry.title || noteTitle(entry.path)}
                </span>
                <span className="block truncate font-mono text-label text-[var(--muted)]">
                  {entry.path}
                </span>
              </button>
            ))
          )}
        </>
      ) : treeLoading && entries.length === 0 ? (
        <p className="px-3 py-2 text-ui-sm text-[var(--muted)]">
          索引中…
        </p>
      ) : entries.length === 0 ? (
        <p className="px-3 py-2 text-ui-sm text-[var(--muted)]">
          目录里还没有 Markdown 笔记。
        </p>
      ) : (
        <TreeRows
          nodes={tree}
          depth={0}
          collapsed={collapsed}
          selectedId={selectedNoteId}
          onToggle={onToggleFolder}
          onOpen={onOpenNote}
          onCreate={onStartCreate}
        />
      )}
    </div>
  );
}
