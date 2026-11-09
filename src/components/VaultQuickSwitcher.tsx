import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { FileText, Search, X } from "lucide-react";
import { createPortal } from "react-dom";

type QuickNote = { id: string; path: string; title: string };

/**
 * 快速切换（⌘O）覆盖层：按标题/路径模糊过滤，↑↓ 选择，Enter 打开，Esc 关闭。
 */
export function VaultQuickSwitcher({
  isOpen,
  notes,
  onClose,
  onPick,
}: {
  isOpen: boolean;
  notes: { id: string; path: string; title: string }[];
  onClose: () => void;
  onPick: (id: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const itemRefs = useRef(new Map<number, HTMLButtonElement>());
  const listId = useId();

  const results = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    if (!keyword) return notes;

    const ranked: { note: QuickNote; score: number }[] = [];
    for (const note of notes) {
      const title = note.title.toLowerCase();
      const path = note.path.toLowerCase();
      const titleIndex = title.indexOf(keyword);
      const pathIndex = path.indexOf(keyword);
      const titleFuzzy =
        titleIndex >= 0 || isSubsequence(title, keyword);
      const pathFuzzy = pathIndex >= 0 || isSubsequence(path, keyword);
      if (!titleFuzzy && !pathFuzzy) continue;

      let score = 5;
      if (titleIndex === 0) score = 0;
      else if (titleIndex > 0) score = 1;
      else if (pathIndex === 0) score = 2;
      else if (pathIndex > 0) score = 3;
      else if (titleFuzzy) score = 4;

      ranked.push({ note, score });
    }

    ranked.sort(
      (a, b) =>
        a.score - b.score || a.note.title.localeCompare(b.note.title),
    );
    return ranked.map((item) => item.note);
  }, [notes, query]);

  useEffect(() => {
    if (!isOpen) {
      setQuery("");
      setActiveIndex(0);
      return;
    }

    const previous = document.activeElement;
    inputRef.current?.focus();

    // 关闭（或卸载）时把焦点还给打开前的元素。
    return () => {
      if (previous instanceof HTMLElement) previous.focus();
    };
  }, [isOpen]);

  // Esc 绑定到 window：焦点滚出面板时也能关闭。
  useEffect(() => {
    if (!isOpen) return;

    function handleWindowKeyDown(event: KeyboardEvent): void {
      if (event.key !== "Escape" || event.isComposing) return;
      event.preventDefault();
      onClose();
    }

    window.addEventListener("keydown", handleWindowKeyDown);
    return () => window.removeEventListener("keydown", handleWindowKeyDown);
  }, [isOpen, onClose]);

  useEffect(() => {
    setActiveIndex((prev) =>
      results.length === 0 ? 0 : Math.min(prev, results.length - 1),
    );
  }, [results]);

  useEffect(() => {
    if (!isOpen) return;
    itemRefs.current.get(activeIndex)?.scrollIntoView({ block: "nearest" });
  }, [activeIndex, isOpen]);

  if (!isOpen) return null;

  const activeNote = results[activeIndex];

  function pick(note: QuickNote): void {
    onPick(note.id);
    onClose();
  }

  function handleKeyDown(event: ReactKeyboardEvent<HTMLDivElement>): void {
    if (event.nativeEvent.isComposing) return;

    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      if (results.length === 0) return;
      event.preventDefault();
      const delta = event.key === "ArrowDown" ? 1 : -1;
      setActiveIndex(
        (prev) => (prev + delta + results.length) % results.length,
      );
      return;
    }
    if (event.key === "Enter") {
      if (!activeNote) return;
      event.preventDefault();
      pick(activeNote);
    }
  }

  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/30 px-4 pt-[12vh]"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="快速切换笔记"
        onKeyDown={handleKeyDown}
        className="flex w-full max-w-[520px] flex-col overflow-hidden rounded-[10px] border border-[var(--border)] bg-[var(--background)] shadow-2xl"
      >
        <div className="flex items-center gap-2 border-b border-[var(--border)] px-3">
          <Search size={15} className="shrink-0 text-[var(--muted)]" />
          <input
            ref={inputRef}
            role="combobox"
            aria-label="搜索笔记"
            aria-expanded
            aria-controls={listId}
            aria-autocomplete="list"
            aria-activedescendant={
              activeNote ? `${listId}-option-${activeIndex}` : undefined
            }
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setActiveIndex(0);
            }}
            placeholder="搜索笔记标题或路径…"
            spellCheck={false}
            className="h-11 min-w-0 flex-1 bg-transparent text-ui text-[var(--foreground)] outline-none placeholder:text-[var(--muted)]"
          />
          {query && (
            <button
              type="button"
              aria-label="清空搜索"
              onClick={() => {
                setQuery("");
                setActiveIndex(0);
                inputRef.current?.focus();
              }}
              className="anim-fast flex size-5 shrink-0 items-center justify-center rounded text-[var(--muted)] hover:bg-[var(--background-tertiary)] hover:text-[var(--foreground)]"
            >
              <X size={13} />
            </button>
          )}
          <span className="shrink-0 rounded border border-[var(--border)] px-1 text-label text-[var(--muted)]">
            Esc
          </span>
        </div>

        <div
          id={listId}
          role="listbox"
          aria-label="笔记候选"
          className="max-h-[min(50vh,360px)] min-h-[64px] overflow-auto p-1"
        >
          {results.length === 0 ? (
            <div className="flex flex-col items-center justify-center gap-1.5 px-3 py-8 text-center">
              <Search size={18} className="text-[var(--muted)] opacity-70" />
              <p className="text-ui-sm text-[var(--muted)]">
                {query.trim()
                  ? `没有匹配「${query.trim()}」的笔记`
                  : "还没有笔记"}
              </p>
              <p className="text-label text-[var(--muted)] opacity-70">
                {notes.length === 0
                  ? "先在笔记库中创建一条笔记。"
                  : "换个关键词试试。"}
              </p>
            </div>
          ) : (
            results.map((note, index) => {
              const isActive = index === activeIndex;
              return (
                <button
                  key={note.id}
                  id={`${listId}-option-${index}`}
                  ref={(node) => {
                    if (node) itemRefs.current.set(index, node);
                    else itemRefs.current.delete(index);
                  }}
                  type="button"
                  role="option"
                  aria-selected={isActive}
                  onMouseEnter={() => setActiveIndex(index)}
                  onClick={() => pick(note)}
                  className={`anim-fast flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left ${
                    isActive
                      ? "bg-[var(--background-secondary)]"
                      : "hover:bg-[var(--background-secondary)]"
                  }`}
                >
                  <FileText
                    size={14}
                    className="shrink-0 text-[var(--muted)]"
                  />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-ui text-[var(--foreground)]">
                      {note.title || note.path.split("/").pop() || note.path}
                    </span>
                    <span className="block truncate font-mono text-label text-[var(--muted)]">
                      {note.path}
                    </span>
                  </span>
                </button>
              );
            })
          )}
        </div>

        <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1 border-t border-[var(--border)] px-3 py-1.5 text-label text-[var(--muted)]">
          <span>
            {query.trim()
              ? `${results.length} / ${notes.length} 条笔记`
              : `${notes.length} 条笔记`}
          </span>
          <span className="flex items-center gap-2">
            <span>↑↓ 选择</span>
            <span>↵ 打开</span>
            <span>Esc 关闭</span>
          </span>
        </div>
      </div>
    </div>,
    document.body,
  );
}

function isSubsequence(haystack: string, needle: string): boolean {
  if (!needle) return true;
  let cursor = 0;
  for (const char of haystack) {
    if (char === needle[cursor]) {
      cursor += 1;
      if (cursor === needle.length) return true;
    }
  }
  return false;
}
