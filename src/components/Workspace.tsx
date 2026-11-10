import { useEffect, useRef, useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Button, Chip, Input } from "@heroui/react";
import { FileText, RefreshCw, X } from "lucide-react";
import {
  errorText,
  getDoc,
  listDocs,
  reindexWorkspace,
  searchWorkspace,
  type Doc,
  type DocMeta,
  type SearchHit,
} from "../lib/api";

const MARKDOWN_CLASS = [
  "text-ui-sm leading-relaxed text-[var(--foreground)]",
  "[&_h1]:mb-2 [&_h1]:mt-3 [&_h1]:text-[15px] [&_h1]:font-semibold [&_h1:first-child]:mt-0",
  "[&_h2]:mb-2 [&_h2]:mt-3 [&_h2]:text-ui [&_h2]:font-semibold",
  "[&_h3]:mb-1 [&_h3]:mt-2 [&_h3]:text-ui [&_h3]:font-medium",
  "[&_p]:mb-2 [&_ul]:mb-2 [&_ul]:list-disc [&_ul]:pl-4 [&_ol]:mb-2 [&_ol]:list-decimal [&_ol]:pl-4 [&_li]:mb-0.5",
  "[&_a]:text-[var(--accent)] [&_a]:underline",
  "[&_code]:rounded [&_code]:bg-[var(--background-tertiary)] [&_code]:px-1 [&_code]:py-0.5 [&_code]:font-mono [&_code]:text-ui-sm",
  "[&_pre]:mb-2 [&_pre]:overflow-auto [&_pre]:rounded-md [&_pre]:border [&_pre]:border-[var(--border)] [&_pre]:bg-[var(--background-tertiary)] [&_pre]:p-2",
  "[&_pre_code]:bg-transparent [&_pre_code]:p-0",
  "[&_blockquote]:mb-2 [&_blockquote]:border-l-2 [&_blockquote]:border-[var(--border)] [&_blockquote]:pl-2 [&_blockquote]:text-[var(--muted)]",
  "[&_table]:mb-2 [&_table]:w-full [&_table]:border-collapse",
  "[&_th]:border [&_th]:border-[var(--border)] [&_th]:px-1.5 [&_th]:py-1 [&_th]:text-left",
  "[&_td]:border [&_td]:border-[var(--border)] [&_td]:px-1.5 [&_td]:py-1",
  "[&_hr]:my-3 [&_hr]:border-[var(--border)]",
  "[&_input]:mr-1 [&_input]:align-middle",
  "[&_img]:max-w-full",
].join(" ");

export function Workspace({
  projectId,
  focusDocId,
  onFocusConsumed,
}: {
  projectId: string;
  focusDocId?: string;
  onFocusConsumed?: () => void;
}) {
  const [docs, setDocs] = useState<DocMeta[]>([]);
  const [listLoading, setListLoading] = useState(true);
  const [listError, setListError] = useState<string | null>(null);

  const [selectedDocId, setSelectedDocId] = useState<string | null>(null);
  const [entry, setEntry] = useState<Doc | null>(null);
  const [entryLoading, setEntryLoading] = useState(false);
  const [entryError, setEntryError] = useState<string | null>(null);

  const [refreshing, setRefreshing] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<SearchHit[]>([]);
  const [searchLoading, setSearchLoading] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);

  const loadRequest = useRef(0);
  const focusConsumedRef = useRef<string | null>(null);
  const projectRef = useRef(projectId);
  projectRef.current = projectId;

  useEffect(() => {
    let cancelled = false;
    loadRequest.current += 1;
    setDocs([]);
    setListLoading(true);
    setListError(null);
    setSelectedDocId(null);
    setEntry(null);
    setEntryLoading(false);
    setEntryError(null);
    setRefreshing(false);
    setActionError(null);
    setNotice(null);
    setSearchQuery("");
    setSearchResults([]);
    setSearchLoading(false);
    setSearchError(null);

    reindexWorkspace(projectId)
      .catch(() => null)
      .then(() => listDocs(projectId))
      .then((next) => {
        if (!cancelled) setDocs(next);
      })
      .catch((e) => {
        if (!cancelled) setListError(errorText(e));
      })
      .finally(() => {
        if (!cancelled) setListLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  // 搜索面板深链：直接打开目标文档并消费一次。
  useEffect(() => {
    if (!focusDocId) {
      focusConsumedRef.current = null;
      return;
    }
    if (focusConsumedRef.current === focusDocId) return;
    focusConsumedRef.current = focusDocId;
    void openDoc(focusDocId);
    onFocusConsumed?.();
  }, [focusDocId, projectId, onFocusConsumed]);

  useEffect(() => {
    const query = searchQuery.trim();
    if (!query) {
      setSearchResults([]);
      setSearchLoading(false);
      setSearchError(null);
      return;
    }
    let cancelled = false;
    setSearchLoading(true);
    setSearchError(null);
    setSearchResults([]);
    const timer = window.setTimeout(() => {
      searchWorkspace(projectId, query)
        .then((next) => {
          if (!cancelled) setSearchResults(next);
        })
        .catch((e) => {
          if (!cancelled) setSearchError(errorText(e));
        })
        .finally(() => {
          if (!cancelled) setSearchLoading(false);
        });
    }, 220);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [projectId, searchQuery]);

  async function refreshList(forProject: string = projectRef.current) {
    if (forProject !== projectRef.current) return;
    const next = await listDocs(forProject);
    if (forProject !== projectRef.current) return;
    setDocs(next);
  }

  async function openDoc(id: string) {
    const request = ++loadRequest.current;
    setSelectedDocId(id);
    setEntry(null);
    setEntryLoading(true);
    setEntryError(null);
    setActionError(null);
    setNotice(null);
    try {
      const doc = await getDoc(id);
      if (request !== loadRequest.current) return;
      setEntry(doc);
    } catch (e) {
      if (request === loadRequest.current) setEntryError(errorText(e));
    } finally {
      if (request === loadRequest.current) setEntryLoading(false);
    }
  }

  async function refreshIndex() {
    if (refreshing) return;
    setRefreshing(true);
    setActionError(null);
    setNotice(null);
    try {
      const count = await reindexWorkspace(projectId);
      await refreshList();
      setNotice(`索引已刷新：${count} 篇文档`);
    } catch (e) {
      setActionError(errorText(e));
    } finally {
      setRefreshing(false);
    }
  }

  const searching = searchQuery.trim().length > 0;

  return (
    <div className="flex h-full min-h-0 gap-3">
      <div className="flex w-[252px] min-w-0 shrink-0 flex-col gap-2">
        <div className="flex items-center gap-1.5">
          <Input
            aria-label="搜索知识库"
            placeholder="搜索文档…"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="min-w-0 flex-1"
          />
          {searching && (
            <Button
              isIconOnly
              variant="ghost"
              size="sm"
              className="h-7 w-7 shrink-0"
              aria-label="清空搜索"
              onPress={() => setSearchQuery("")}
            >
              <X size={14} />
            </Button>
          )}
        </div>

        <div className="flex flex-wrap items-center justify-between gap-2 px-1">
          <span className="text-ui-sm text-[var(--muted)]">
            {docs.length} 篇文档
          </span>
          <Button
            variant="secondary"
            size="sm"
            className="h-7 gap-1 px-2 text-ui"
            onPress={() => void refreshIndex()}
            isDisabled={refreshing}
          >
            <RefreshCw
              size={14}
              className={refreshing ? "animate-spin" : undefined}
            />
            {refreshing ? "刷新中…" : "刷新索引"}
          </Button>
        </div>

        {listError && (
          <p role="alert" className="px-1 text-ui-sm text-[var(--danger)]">
            {listError}
          </p>
        )}

        <div className="min-h-0 flex-1 overflow-auto rounded-md border border-[var(--border)]">
          {searching ? (
            searchLoading ? (
              <p className="px-3 py-2 text-ui-sm text-[var(--muted)]">
                搜索中…
              </p>
            ) : searchError ? (
              <p role="alert" className="px-3 py-2 text-ui-sm text-[var(--danger)]">
                {searchError}
              </p>
            ) : searchResults.length === 0 ? (
              <p className="px-3 py-2 text-ui-sm text-[var(--muted)]">
                没有匹配的内容。
              </p>
            ) : (
              searchResults.map((hit) => (
                <button
                  key={`${hit.kind}:${hit.id}`}
                  type="button"
                  onClick={() => void openDoc(hit.id)}
                  className={`anim-fast block w-full border-b border-[var(--border)] px-3 py-2 text-left last:border-b-0 hover:bg-[var(--background-secondary)] ${
                    selectedDocId === hit.id
                      ? "bg-[var(--background-secondary)]"
                      : ""
                  }`}
                >
                  <span className="flex items-center gap-1.5">
                    <Chip
                      size="sm"
                      variant="tertiary"
                      color="default"
                      className="shrink-0"
                    >
                      文档
                    </Chip>
                    <span className="min-w-0 flex-1 truncate text-ui">
                      {hit.title || "无标题"}
                    </span>
                  </span>
                  {hit.snippet && (
                    <span className="mt-0.5 line-clamp-2 text-ui-sm text-[var(--muted)]">
                      {hit.snippet}
                    </span>
                  )}
                </button>
              ))
            )
          ) : listLoading ? (
            <p className="px-3 py-2 text-ui-sm text-[var(--muted)]">
              加载中…
            </p>
          ) : docs.length === 0 ? (
            <p className="px-3 py-2 text-ui-sm text-[var(--muted)]">
              仓库 docs/ 下没有 Markdown 文档。
            </p>
          ) : (
            docs.map((doc) => (
              <button
                key={doc.id}
                type="button"
                onClick={() => void openDoc(doc.id)}
                className={`anim-fast block w-full border-b border-[var(--border)] px-3 py-2 text-left last:border-b-0 hover:bg-[var(--background-secondary)] ${
                  selectedDocId === doc.id
                    ? "bg-[var(--background-secondary)]"
                    : ""
                }`}
              >
                <span className="block truncate text-ui">
                  {doc.title || doc.path}
                </span>
                <span
                  className="block truncate font-mono text-ui-sm text-[var(--muted)]"
                  title={doc.path}
                >
                  {doc.path}
                </span>
              </button>
            ))
          )}
        </div>
      </div>

      <section className="flex min-w-0 flex-1 flex-col overflow-hidden rounded-md border border-[var(--border)]">
        <header className="flex min-h-12 shrink-0 flex-wrap items-center gap-2 border-b border-[var(--border)] px-3 py-1.5">
          <div className="min-w-0 flex-1">
            {entry ? (
              <>
                <p className="truncate text-ui font-medium">
                  {entry.title || entry.path}
                </p>
                <p
                  className="truncate font-mono text-label text-[var(--muted)]"
                  title={entry.path}
                >
                  {entry.path} · 只读
                </p>
              </>
            ) : (
              <p className="text-ui text-[var(--muted)]">选择一篇文档</p>
            )}
          </div>
        </header>

        {actionError && (
          <p
            role="alert"
            className="shrink-0 border-b border-[var(--border)] px-3 py-1.5 text-ui-sm text-[var(--danger)]"
          >
            {actionError}
          </p>
        )}

        {notice && (
          <p className="shrink-0 border-b border-[var(--border)] px-3 py-1.5 text-ui-sm text-[var(--muted)]">
            {notice}
          </p>
        )}

        {entryLoading ? (
          <p className="p-3 text-ui-sm text-[var(--muted)]">加载中…</p>
        ) : entryError ? (
          <p role="alert" className="p-3 text-ui-sm text-[var(--danger)]">
            {entryError}
          </p>
        ) : entry ? (
          <div className="min-h-0 flex-1 overflow-auto p-4">
            {entry.body.trim() ? (
              <div className={MARKDOWN_CLASS}>
                <Markdown remarkPlugins={[remarkGfm]}>{entry.body}</Markdown>
              </div>
            ) : (
              <p className="text-ui-sm text-[var(--muted)]">文档为空。</p>
            )}
          </div>
        ) : (
          <div className="flex flex-1 flex-col items-center justify-center gap-2 text-center">
            <FileText size={20} className="text-[var(--muted)] opacity-70" />
            <p className="text-ui text-[var(--muted)]">
              选择一篇文档查看内容；仓库文档在应用内只读。
            </p>
          </div>
        )}
      </section>
    </div>
  );
}
