import { useMemo } from "react";
import { Tag, X } from "lucide-react";

/**
 * 标签云面板：按引用次数降序展示标签，支持选中/清除筛选。
 * 纯展示组件，不发起请求；数据由父组件传入。
 */
export function VaultTagsPanel({
  tags,
  activeTag,
  loading = false,
  onSelect,
}: {
  tags: { tag: string; count: number }[];
  activeTag: string | null;
  loading?: boolean;
  onSelect: (tag: string | null) => void;
}) {
  const sorted = useMemo(
    () =>
      [...tags].sort(
        (a, b) => b.count - a.count || a.tag.localeCompare(b.tag),
      ),
    [tags],
  );
  const total = sorted.reduce((sum, item) => sum + item.count, 0);
  const hasTags = sorted.length > 0;

  return (
    <section className="flex min-h-0 flex-col" aria-label="标签面板">
      <header className="flex h-9 shrink-0 items-center gap-1.5 border-b border-[var(--border)] px-2.5 text-ui-sm text-[var(--muted)]">
        <Tag size={13} className="shrink-0" />
        <span>标签</span>
        <span className="ml-auto text-label">
          {loading && !hasTags ? "…" : sorted.length}
        </span>
        {activeTag !== null && (
          <button
            type="button"
            aria-label="清除标签筛选"
            title="清除标签筛选"
            onClick={() => onSelect(null)}
            className="anim-fast flex size-5 shrink-0 items-center justify-center rounded text-[var(--muted)] hover:bg-[var(--background-tertiary)] hover:text-[var(--foreground)]"
          >
            <X size={12} />
          </button>
        )}
      </header>

      <div className="min-h-0 flex-1 overflow-auto p-2">
        {!hasTags ? (
          loading ? (
            <p className="px-1 py-2 text-ui-sm text-[var(--muted)]">
              加载标签…
            </p>
          ) : (
            <div className="flex flex-col items-center gap-1.5 px-2 py-6 text-center">
              <Tag size={16} className="text-[var(--muted)] opacity-70" />
              <p className="text-ui-sm text-[var(--muted)]">还没有标签</p>
              <p className="text-label text-[var(--muted)] opacity-70">
                在 frontmatter 或正文中用 #标签
              </p>
            </div>
          )
        ) : (
          <>
            <div className="flex flex-wrap gap-1.5" role="group" aria-label="标签筛选">
              <button
                type="button"
                aria-pressed={activeTag === null}
                onClick={() => onSelect(null)}
                className={`anim-fast inline-flex max-w-full items-center rounded-lg border px-2 py-1 text-ui-sm ${
                  activeTag === null
                    ? "border-[var(--accent)] bg-[var(--background-secondary)] font-medium text-[var(--accent)]"
                    : "border-[var(--border)] text-[var(--foreground)] hover:bg-[var(--background-secondary)]"
                }`}
              >
                全部
              </button>
              {sorted.map(({ tag, count }) => {
                const isActive = activeTag === tag;
                return (
                  <button
                    key={tag}
                    type="button"
                    aria-pressed={isActive}
                    title={`${tag.startsWith("#") ? tag : `#${tag}`} · ${count} 次引用`}
                    onClick={() => onSelect(isActive ? null : tag)}
                    className={`anim-fast inline-flex min-w-0 max-w-full items-center gap-1 rounded-lg border px-2 py-1 text-ui-sm ${
                      isActive
                        ? "border-[var(--accent)] bg-[var(--background-secondary)] font-medium text-[var(--accent)]"
                        : "border-[var(--border)] text-[var(--foreground)] hover:bg-[var(--background-secondary)]"
                    }`}
                  >
                    <span className="max-w-[180px] truncate">
                      {tag.startsWith("#") ? tag : `#${tag}`}
                    </span>
                    <span
                      className={`shrink-0 text-label ${
                        isActive ? "opacity-80" : "text-[var(--muted)]"
                      }`}
                    >
                      {count}
                    </span>
                  </button>
                );
              })}
            </div>
            <p className="mt-2 px-1 text-label text-[var(--muted)] opacity-70">
              {sorted.length} 个标签 · {total} 次引用
              {loading ? " · 更新中…" : ""}
            </p>
          </>
        )}
      </div>
    </section>
  );
}
