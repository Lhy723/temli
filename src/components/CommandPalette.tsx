import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";
import { Button, Kbd, Modal, SearchField, Separator, Spinner } from "@heroui/react";
import { CircleDot, FileText, NotebookPen, Search } from "lucide-react";
import { errorText, globalSearch, type GlobalHit } from "../lib/api";

const GROUPS: {
  kind: GlobalHit["kind"];
  label: string;
  icon: typeof CircleDot;
}[] = [
  { kind: "task", label: "任务", icon: CircleDot },
  { kind: "doc", label: "文档", icon: FileText },
  { kind: "note", label: "笔记", icon: NotebookPen },
];

const SEARCH_LIMIT = 30;
const DEBOUNCE_MS = 200;

export function CommandPalette({
  isOpen,
  onClose,
  projectId,
  projectName,
  onSelect,
}: {
  isOpen: boolean;
  onClose: () => void;
  projectId: string | null;
  projectName: string | null;
  onSelect: (hit: GlobalHit) => void;
}) {
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<GlobalHit[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [activeIndex, setActiveIndex] = useState(0);
  const requestRef = useRef(0);
  const itemRefs = useRef(new Map<number, HTMLButtonElement>());

  const trimmed = query.trim();

  useEffect(() => {
    if (isOpen) return;
    requestRef.current += 1;
    setQuery("");
    setHits([]);
    setLoading(false);
    setError(null);
    setActiveIndex(0);
  }, [isOpen]);

  useEffect(() => {
    if (!isOpen) return;
    if (!trimmed) {
      requestRef.current += 1;
      setHits([]);
      setLoading(false);
      setError(null);
      return;
    }

    const requestId = ++requestRef.current;
    setHits([]);
    setLoading(true);
    setError(null);
    setActiveIndex(0);

    const timer = window.setTimeout(() => {
      globalSearch({
        query: trimmed,
        project_id: projectId ?? undefined,
        limit: SEARCH_LIMIT,
      })
        .then((next) => {
          if (requestRef.current !== requestId) return;
          setHits(next);
          setActiveIndex(0);
        })
        .catch((reason) => {
          if (requestRef.current !== requestId) return;
          setHits([]);
          setError(errorText(reason));
        })
        .finally(() => {
          if (requestRef.current !== requestId) return;
          setLoading(false);
        });
    }, DEBOUNCE_MS);

    return () => window.clearTimeout(timer);
  }, [isOpen, trimmed, projectId]);

  const groups = useMemo(() => {
    let start = 0;
    return GROUPS.map((group) => {
      const items = hits.filter((hit) => hit.kind === group.kind);
      const offset = start;
      start += items.length;
      return { ...group, items, start: offset };
    }).filter((group) => group.items.length > 0);
  }, [hits]);

  const flatHits = useMemo(
    () => groups.flatMap((group) => group.items),
    [groups],
  );

  useEffect(() => {
    if (!isOpen) return;
    itemRefs.current.get(activeIndex)?.scrollIntoView({ block: "nearest" });
  }, [activeIndex, isOpen, groups.length]);

  function choose(hit: GlobalHit) {
    onSelect(hit);
    onClose();
  }

  function handleKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.nativeEvent.isComposing) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      if (flatHits.length === 0) return;
      setActiveIndex((prev) => (prev + 1) % flatHits.length);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      if (flatHits.length === 0) return;
      setActiveIndex((prev) => (prev - 1 + flatHits.length) % flatHits.length);
    } else if (event.key === "Enter") {
      if (loading) return;
      const hit = flatHits[activeIndex];
      if (!hit) return;
      event.preventDefault();
      choose(hit);
    } else if (event.key === "Escape") {
      event.preventDefault();
      onClose();
    }
  }

  return (
    <Modal.Backdrop
      isOpen={isOpen}
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <Modal.Container placement="top" scroll="inside" size="lg">
        <Modal.Dialog
          aria-label="全局搜索"
          className="gap-0 overflow-hidden p-0 sm:max-w-[560px]"
        >
          <Modal.Header className="px-3 pb-0 pt-3">
            <SearchField
              aria-label="搜索任务、文档与笔记"
              fullWidth
              onChange={setQuery}
              value={query}
              variant="secondary"
            >
              <SearchField.Group>
                <SearchField.SearchIcon />
                <SearchField.Input
                  autoFocus
                  placeholder="搜索任务、文档与笔记…"
                  onKeyDown={handleKeyDown}
                />
                <SearchField.ClearButton />
              </SearchField.Group>
            </SearchField>
          </Modal.Header>

          <Modal.Body className="max-h-[min(52vh,420px)] px-2 py-2">
            {error ? (
              <div className="flex h-[140px] flex-col items-center justify-center gap-1.5 px-4 text-center">
                <p role="alert" className="text-ui-sm text-[var(--danger)]">
                  {error}
                </p>
                <p className="text-ui-sm text-[var(--muted)] opacity-70">
                  请稍后重试
                </p>
              </div>
            ) : loading ? (
              <div className="flex h-[120px] items-center justify-center gap-2 text-ui-sm text-[var(--muted)]">
                <Spinner size="sm" />
                搜索中…
              </div>
            ) : !trimmed ? (
              <div className="flex h-[160px] flex-col items-center justify-center gap-2 text-center">
                <Search
                  size={18}
                  className="text-[var(--muted)] opacity-70"
                />
                <p className="text-ui text-[var(--muted)]">
                  搜索任务、文档与笔记
                </p>
                <p className="text-ui-sm text-[var(--muted)] opacity-70">
                  输入关键词开始搜索，⌘K 可随时唤起
                </p>
              </div>
            ) : flatHits.length === 0 ? (
              <div className="flex h-[140px] flex-col items-center justify-center gap-1.5 px-4 text-center">
                <p className="max-w-full truncate text-ui text-[var(--muted)]">
                  没有匹配「{trimmed}」的结果
                </p>
                <p className="text-ui-sm text-[var(--muted)] opacity-70">
                  换个关键词试试
                </p>
              </div>
            ) : (
              <div className="flex flex-col gap-1">
                {groups.map((group) => (
                  <div key={group.kind}>
                    <p className="text-label px-2 pb-1 pt-1 font-medium uppercase tracking-wide text-[var(--muted)]">
                      {group.label}
                    </p>
                    <div className="flex flex-col gap-0.5">
                      {group.items.map((hit, itemIndex) => {
                        const index = group.start + itemIndex;
                        return (
                          <Button
                            key={`${group.kind}:${hit.id}`}
                            ref={(node) => {
                              if (node) itemRefs.current.set(index, node);
                              else itemRefs.current.delete(index);
                            }}
                            variant={index === activeIndex ? "tertiary" : "ghost"}
                            size="sm"
                            className="h-auto w-full justify-start gap-2 px-2 py-1.5 text-left font-normal"
                            onFocus={() => setActiveIndex(index)}
                            onHoverStart={() => setActiveIndex(index)}
                            onPress={() => choose(hit)}
                          >
                            <group.icon
                              size={15}
                              className="shrink-0 text-[var(--muted)]"
                            />
                            <span className="flex min-w-0 flex-1 flex-col items-start">
                              <span className="w-full truncate text-ui text-[var(--foreground)]">
                                {hit.title || "无标题"}
                              </span>
                              {hit.snippet && (
                                <span className="mt-0.5 line-clamp-1 w-full text-ui-sm text-[var(--muted)]">
                                  {hit.snippet}
                                </span>
                              )}
                            </span>
                          </Button>
                        );
                      })}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </Modal.Body>

          <Separator />

          <Modal.Footer className="px-3 py-2">
            <div className="flex w-full items-center justify-between gap-3">
              <span className="truncate text-ui-sm text-[var(--muted)]">
                {projectName
                  ? `范围：${projectName} 的任务/文档 · 全部笔记`
                  : "范围：全部项目与笔记库"}
              </span>
              <span className="flex shrink-0 items-center gap-2 text-label text-[var(--muted)]">
                <span className="flex items-center gap-0.5">
                  <Kbd>
                    <Kbd.Abbr keyValue="up" />
                  </Kbd>
                  <Kbd>
                    <Kbd.Abbr keyValue="down" />
                  </Kbd>
                  选择
                </span>
                <span className="flex items-center gap-0.5">
                  <Kbd>
                    <Kbd.Abbr keyValue="enter" />
                  </Kbd>
                  打开
                </span>
                <span className="flex items-center gap-0.5">
                  <Kbd>
                    <Kbd.Abbr keyValue="escape" />
                  </Kbd>
                  关闭
                </span>
              </span>
            </div>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
