import { useEffect, useRef, useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Button, Card, Chip, Input, TextArea } from "@heroui/react";
import {
  FileText,
  ListPlus,
  NotebookPen,
  Plus,
  RefreshCw,
  Save,
  Trash2,
  X,
} from "lucide-react";
import {
  createNote,
  createTaskFromNote,
  deleteNote,
  errorText,
  formatDate,
  getDoc,
  getNote,
  listDocs,
  listNotes,
  reindexWorkspace,
  searchWorkspace,
  updateNote,
  type Doc,
  type DocMeta,
  type Note,
  type NoteMeta,
  type SearchHit,
} from "../lib/api";

export type WorkspaceMode = "notes" | "docs";

type EntryKind = "note" | "doc";

type EntryRef = { kind: EntryKind; id: string };

type OpenedEntry =
  | { kind: "note"; note: Note }
  | { kind: "doc"; doc: Doc };

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
  mode,
  focusDocId,
  onFocusConsumed,
}: {
  projectId: string;
  mode: WorkspaceMode;
  focusDocId?: string;
  onFocusConsumed?: () => void;
}) {
  const [notes, setNotes] = useState<NoteMeta[]>([]);
  const [docs, setDocs] = useState<DocMeta[]>([]);
  const [listLoading, setListLoading] = useState(true);
  const [listError, setListError] = useState<string | null>(null);

  const [selected, setSelected] = useState<EntryRef | null>(null);
  const [entry, setEntry] = useState<OpenedEntry | null>(null);
  const [entryLoading, setEntryLoading] = useState(false);
  const [entryError, setEntryError] = useState<string | null>(null);
  const [newNote, setNewNote] = useState(false);

  const [draftTitle, setDraftTitle] = useState("");
  const [draftBody, setDraftBody] = useState("");
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<SearchHit[]>([]);
  const [searchLoading, setSearchLoading] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);

  const [taskFormOpen, setTaskFormOpen] = useState(false);
  const [taskTitle, setTaskTitle] = useState("");
  const [taskPrompt, setTaskPrompt] = useState("");
  const [taskBusy, setTaskBusy] = useState(false);
  const [taskError, setTaskError] = useState<string | null>(null);

  const loadRequest = useRef(0);
  const focusConsumedRef = useRef<string | null>(null);
  const projectRef = useRef(projectId);
  projectRef.current = projectId;

  useEffect(() => {
    let cancelled = false;
    loadRequest.current += 1;
    setNotes([]);
    setDocs([]);
    setListLoading(true);
    setListError(null);
    setSelected(null);
    setEntry(null);
    setEntryLoading(false);
    setEntryError(null);
    setNewNote(false);
    setDraftTitle("");
    setDraftBody("");
    setSaving(false);
    setDeleting(false);
    setConfirmingDelete(false);
    setRefreshing(false);
    setActionError(null);
    setNotice(null);
    setSearchQuery("");
    setSearchResults([]);
    setSearchLoading(false);
    setSearchError(null);
    setTaskFormOpen(false);
    setTaskTitle("");
    setTaskPrompt("");
    setTaskBusy(false);
    setTaskError(null);

    if (mode === "notes") {
      listNotes(projectId)
        .then((next) => {
          if (!cancelled) setNotes(next);
        })
        .catch((e) => {
          if (!cancelled) setListError(errorText(e));
        })
        .finally(() => {
          if (!cancelled) setListLoading(false);
        });
    } else {
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
    }
    return () => {
      cancelled = true;
    };
  }, [projectId, mode]);

  // 搜索面板深链：文档模式下直接打开目标文档并消费一次。
  useEffect(() => {
    if (!focusDocId) {
      focusConsumedRef.current = null;
      return;
    }
    if (mode !== "docs") return;
    if (focusConsumedRef.current === focusDocId) return;
    focusConsumedRef.current = focusDocId;
    void openEntry("doc", focusDocId);
    onFocusConsumed?.();
  }, [focusDocId, mode, projectId, onFocusConsumed]);

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
    if (mode === "notes") {
      const next = await listNotes(forProject);
      if (forProject !== projectRef.current) return;
      setNotes(next);
    } else {
      const next = await listDocs(forProject);
      if (forProject !== projectRef.current) return;
      setDocs(next);
    }
  }

  async function openEntry(kind: EntryKind, id: string) {
    const request = ++loadRequest.current;
    setSelected({ kind, id });
    setNewNote(false);
    setEntry(null);
    setEntryLoading(true);
    setEntryError(null);
    setConfirmingDelete(false);
    setActionError(null);
    setNotice(null);
    setTaskFormOpen(false);
    setTaskError(null);
    try {
      if (kind === "note") {
        const note = await getNote(id);
        if (request !== loadRequest.current) return;
        setEntry({ kind: "note", note });
        setDraftTitle(note.title);
        setDraftBody(note.body);
      } else {
        const doc = await getDoc(id);
        if (request !== loadRequest.current) return;
        setEntry({ kind: "doc", doc });
        setDraftTitle(doc.title);
        setDraftBody("");
      }
    } catch (e) {
      if (request === loadRequest.current) setEntryError(errorText(e));
    } finally {
      if (request === loadRequest.current) setEntryLoading(false);
    }
  }

  function startNewNote() {
    loadRequest.current += 1;
    setSelected(null);
    setEntry(null);
    setEntryLoading(false);
    setEntryError(null);
    setNewNote(true);
    setConfirmingDelete(false);
    setDraftTitle("");
    setDraftBody("");
    setActionError(null);
    setNotice(null);
    setTaskFormOpen(false);
    setTaskError(null);
  }

  function cancelNewNote() {
    loadRequest.current += 1;
    setNewNote(false);
    setDraftTitle("");
    setDraftBody("");
    setConfirmingDelete(false);
    setActionError(null);
    setNotice(null);
    setTaskFormOpen(false);
    setTaskError(null);
  }

  async function saveNote() {
    const title = draftTitle.trim();
    if (saving) return;
    if (!title) {
      setActionError("笔记标题不能为空。");
      return;
    }
    const openNote = entry && entry.kind === "note" ? entry.note : null;
    if (!openNote && !newNote) return;
    const request = loadRequest.current;
    const project = projectRef.current;
    setSaving(true);
    setConfirmingDelete(false);
    setActionError(null);
    setNotice(null);
    try {
      if (openNote) {
        const updated = await updateNote({
          id: openNote.id,
          title,
          body: draftBody,
        });
        if (request !== loadRequest.current || projectRef.current !== project) return;
        setEntry({ kind: "note", note: updated });
        setDraftTitle(updated.title);
        setDraftBody(updated.body);
        setNotice("已保存");
      } else {
        const created = await createNote({
          project_id: project,
          title,
          body: draftBody,
        });
        if (request !== loadRequest.current || projectRef.current !== project) return;
        setEntry({ kind: "note", note: created });
        setNewNote(false);
        setSelected({ kind: "note", id: created.id });
        setDraftTitle(created.title);
        setDraftBody(created.body);
        setNotice("已创建笔记");
      }
      await refreshList(project);
    } catch (e) {
      if (request === loadRequest.current && projectRef.current === project) {
        setActionError(errorText(e));
      }
    } finally {
      if (request === loadRequest.current) setSaving(false);
    }
  }

  async function removeNote() {
    const note = entry && entry.kind === "note" ? entry.note : null;
    if (!note || deleting) return;
    if (!confirmingDelete) {
      setConfirmingDelete(true);
      setActionError(null);
      setNotice("再次点击「确认删除」将永久删除这条笔记。");
      return;
    }
    const request = loadRequest.current;
    const project = projectRef.current;
    setConfirmingDelete(false);
    setDeleting(true);
    setActionError(null);
    setNotice(null);
    try {
      await deleteNote(note.id);
      if (request !== loadRequest.current || projectRef.current !== project) return;
      loadRequest.current += 1;
      setEntry(null);
      setSelected(null);
      setDraftTitle("");
      setDraftBody("");
      await refreshList(project);
      setNotice("笔记已删除");
    } catch (e) {
      if (request === loadRequest.current && projectRef.current === project) {
        setActionError(errorText(e));
      }
    } finally {
      if (request === loadRequest.current) setDeleting(false);
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

  function openTaskForm() {
    const note = entry && entry.kind === "note" ? entry.note : null;
    if (!note) return;
    setTaskFormOpen(true);
    setTaskTitle(draftTitle.trim() || note.title);
    setTaskPrompt(draftBody);
    setTaskError(null);
    setNotice(null);
  }

  async function submitTask() {
    const note = entry && entry.kind === "note" ? entry.note : null;
    if (!note || taskBusy) return;
    const title = taskTitle.trim();
    const prompt = taskPrompt.trim();
    setTaskBusy(true);
    setTaskError(null);
    try {
      const created = await createTaskFromNote({
        note_id: note.id,
        ...(title ? { title } : {}),
        ...(prompt ? { prompt } : {}),
      });
      setTaskFormOpen(false);
      setNotice(`已创建任务「${created.task.title}」`);
    } catch (e) {
      setTaskError(errorText(e));
    } finally {
      setTaskBusy(false);
    }
  }

  function openHit(hit: SearchHit) {
    void openEntry(hit.kind === "doc" ? "doc" : "note", hit.id);
  }

  const openNote = entry && entry.kind === "note" ? entry.note : null;
  const openDoc = entry && entry.kind === "doc" ? entry.doc : null;
  const editingNote = openNote !== null || newNote;
  const previewBody =
    openNote !== null
      ? draftBody
      : openDoc !== null
        ? openDoc.body
        : draftBody;
  const selectedNoteId =
    selected && selected.kind === "note" ? selected.id : null;
  const selectedDocId =
    selected && selected.kind === "doc" ? selected.id : null;
  const searching = searchQuery.trim().length > 0;

  return (
    <div className="flex h-full min-h-0 gap-3">
      <div className="flex w-[252px] min-w-0 shrink-0 flex-col gap-2">
        <div className="flex items-center gap-1.5">
          <Input
            aria-label="搜索知识库"
            placeholder={mode === "notes" ? "搜索笔记…" : "搜索文档…"}
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
            {mode === "notes"
              ? `${notes.length} 条笔记`
              : `${docs.length} 篇文档`}
          </span>
          <div className="flex items-center gap-1">
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
            {mode === "notes" && (
              <Button
                variant="secondary"
                size="sm"
                className="h-7 gap-1 px-2 text-ui"
                onPress={startNewNote}
              >
                <Plus size={14} />
                新建笔记
              </Button>
            )}
          </div>
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
              searchResults.map((hit) => {
                const isSelected =
                  (hit.kind === "doc" && selectedDocId === hit.id) ||
                  (hit.kind !== "doc" && selectedNoteId === hit.id);
                return (
                  <button
                    key={`${hit.kind}:${hit.id}`}
                    type="button"
                    onClick={() => openHit(hit)}
                    className={`anim-fast block w-full border-b border-[var(--border)] px-3 py-2 text-left last:border-b-0 hover:bg-[var(--background-secondary)] ${
                      isSelected ? "bg-[var(--background-secondary)]" : ""
                    }`}
                  >
                    <span className="flex items-center gap-1.5">
                      <Chip
                        size="sm"
                        variant="tertiary"
                        color="default"
                        className="shrink-0"
                      >
                        {hit.kind === "doc" ? "文档" : "笔记"}
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
                );
              })
            )
          ) : listLoading ? (
            <p className="px-3 py-2 text-ui-sm text-[var(--muted)]">
              加载中…
            </p>
          ) : mode === "notes" ? (
            notes.length === 0 ? (
              <p className="px-3 py-2 text-ui-sm text-[var(--muted)]">
                还没有笔记。新建一条记录想法。
              </p>
            ) : (
              notes.map((note) => (
                <button
                  key={note.id}
                  type="button"
                  onClick={() => void openEntry("note", note.id)}
                  className={`anim-fast block w-full border-b border-[var(--border)] px-3 py-2 text-left last:border-b-0 hover:bg-[var(--background-secondary)] ${
                    selectedNoteId === note.id
                      ? "bg-[var(--background-secondary)]"
                      : ""
                  }`}
                >
                  <span className="block truncate text-ui">
                    {note.title || "无标题笔记"}
                  </span>
                  <span className="block text-ui-sm text-[var(--muted)]">
                    {formatDate(note.updated_at)}
                  </span>
                </button>
              ))
            )
          ) : docs.length === 0 ? (
            <p className="px-3 py-2 text-ui-sm text-[var(--muted)]">
              仓库 docs/ 下没有 Markdown 文档。
            </p>
          ) : (
            docs.map((doc) => (
              <button
                key={doc.id}
                type="button"
                onClick={() => void openEntry("doc", doc.id)}
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
            {editingNote ? (
              <Input
                aria-label="笔记标题"
                placeholder="笔记标题"
                value={draftTitle}
                onChange={(e) => setDraftTitle(e.target.value)}
                disabled={saving || deleting}
                className="text-ui"
              />
            ) : openDoc ? (
              <>
                <p className="truncate text-ui font-medium">
                  {openDoc.title || openDoc.path}
                </p>
                <p
                  className="truncate font-mono text-label text-[var(--muted)]"
                  title={openDoc.path}
                >
                  {openDoc.path} · 只读
                </p>
              </>
            ) : (
              <p className="text-ui text-[var(--muted)]">
                {mode === "notes"
                  ? "选择或新建一条笔记"
                  : "选择一篇文档"}
              </p>
            )}
          </div>

          {openNote && (
            <>
              <Button
                variant="secondary"
                size="sm"
                className="h-7 shrink-0 gap-1 px-2.5 text-ui"
                onPress={openTaskForm}
                isDisabled={saving || deleting || taskBusy}
              >
                <ListPlus size={14} />
                从笔记创建任务
              </Button>
              <Button
                variant="secondary"
                size="sm"
                className="h-7 shrink-0 gap-1 px-2.5 text-ui"
                onPress={() => void saveNote()}
                isDisabled={saving || deleting || !draftTitle.trim()}
              >
                <Save size={14} />
                {saving ? "保存中…" : "保存"}
              </Button>
              <Button
                variant="danger-soft"
                size="sm"
                className="h-7 shrink-0 gap-1 px-2.5 text-ui"
                onPress={() => void removeNote()}
                isDisabled={saving || deleting}
              >
                <Trash2 size={14} />
                {deleting
                  ? "删除中…"
                  : confirmingDelete
                    ? "确认删除"
                    : "删除"}
              </Button>
            </>
          )}

          {newNote && (
            <>
              <Button
                variant="ghost"
                size="sm"
                className="h-7 shrink-0 px-2.5 text-ui"
                onPress={cancelNewNote}
                isDisabled={saving}
              >
                取消
              </Button>
              <Button
                variant="primary"
                size="sm"
                className="h-7 shrink-0 gap-1 px-2.5 text-ui"
                onPress={() => void saveNote()}
                isDisabled={saving || !draftTitle.trim()}
              >
                <Save size={14} />
                {saving ? "创建中…" : "保存"}
              </Button>
            </>
          )}
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

        {taskFormOpen && openNote && (
          <Card className="m-3 mb-0 shrink-0">
            <Card.Header>
              <Card.Title className="text-ui font-medium">
                从笔记创建任务
              </Card.Title>
              <Card.Description className="text-ui-sm">
                标题与意图已从笔记预填，可在创建前调整。
              </Card.Description>
            </Card.Header>
            <Card.Content className="grid gap-2">
              <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
                标题
                <Input
                  aria-label="任务标题"
                  value={taskTitle}
                  onChange={(e) => setTaskTitle(e.target.value)}
                  disabled={taskBusy}
                />
              </label>
              <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
                意图
                <TextArea
                  aria-label="任务意图"
                  rows={3}
                  value={taskPrompt}
                  onChange={(e) => setTaskPrompt(e.target.value)}
                  disabled={taskBusy}
                />
              </label>
              {taskError && (
                <p role="alert" className="text-ui-sm text-[var(--danger)]">
                  {taskError}
                </p>
              )}
            </Card.Content>
            <Card.Footer className="flex justify-end gap-1.5">
              <Button
                variant="ghost"
                size="sm"
                className="h-7 px-2.5 text-ui"
                onPress={() => setTaskFormOpen(false)}
                isDisabled={taskBusy}
              >
                取消
              </Button>
              <Button
                variant="primary"
                size="sm"
                className="h-7 px-2.5 text-ui"
                onPress={() => void submitTask()}
                isDisabled={taskBusy}
              >
                {taskBusy ? "创建中…" : "创建任务"}
              </Button>
            </Card.Footer>
          </Card>
        )}

        {entryLoading ? (
          <p className="p-3 text-ui-sm text-[var(--muted)]">加载中…</p>
        ) : entryError ? (
          <p role="alert" className="p-3 text-ui-sm text-[var(--danger)]">
            {entryError}
          </p>
        ) : editingNote ? (
          <div className="grid min-h-0 flex-1 grid-rows-[minmax(0,1fr)_minmax(0,1fr)] gap-2 p-3">
            <TextArea
              aria-label="笔记正文"
              placeholder="用 Markdown 记录想法…"
              value={draftBody}
              onChange={(e) => setDraftBody(e.target.value)}
              disabled={saving || deleting}
              className="min-h-0 h-full w-full resize-none font-mono text-ui-sm"
            />
            <div className="min-h-0 overflow-auto rounded-md border border-[var(--border)] bg-[var(--background-secondary)] p-3">
              {previewBody.trim() ? (
                <div className={MARKDOWN_CLASS}>
                  <Markdown remarkPlugins={[remarkGfm]}>
                    {previewBody}
                  </Markdown>
                </div>
              ) : (
                <p className="text-ui-sm text-[var(--muted)]">
                  预览会显示在这里。
                </p>
              )}
            </div>
          </div>
        ) : openDoc ? (
          <div className="min-h-0 flex-1 overflow-auto p-4">
            {openDoc.body.trim() ? (
              <div className={MARKDOWN_CLASS}>
                <Markdown remarkPlugins={[remarkGfm]}>
                  {openDoc.body}
                </Markdown>
              </div>
            ) : (
              <p className="text-ui-sm text-[var(--muted)]">文档为空。</p>
            )}
          </div>
        ) : (
          <div className="flex flex-1 flex-col items-center justify-center gap-2 text-center">
            {mode === "notes" ? (
              <NotebookPen
                size={20}
                className="text-[var(--muted)] opacity-70"
              />
            ) : (
              <FileText
                size={20}
                className="text-[var(--muted)] opacity-70"
              />
            )}
            <p className="text-ui text-[var(--muted)]">
              {mode === "notes"
                ? "选择一条笔记，或新建笔记开始记录。"
                : "选择一篇文档查看内容；仓库文档在应用内只读。"}
            </p>
          </div>
        )}
      </section>
    </div>
  );
}
