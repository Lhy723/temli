import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import {
  createVaultNote,
  createVaultNoteWithContent,
  deleteVaultNote,
  errorText,
  getVaultNote,
  indexVault,
  listVaultTemplates,
  noteBacklinks,
  renameVaultNote,
  saveVaultNote,
  setNoteProperties,
  type Backlink,
  type VaultEntry,
  type VaultNote,
} from "../../lib/api";
import {
  applyTemplateVariables,
  localDate,
  normalizeNotePath,
  noteTitle,
  offsetForLine,
  uniqueNotePath,
  type AsideTab,
  type CompletionState,
  type VaultMode,
} from "./shared";

type UseVaultNotesOptions = {
  selectedVaultId: string;
  entries: VaultEntry[];
  refreshTree: (vaultId: string, reindex: boolean) => Promise<void>;
  refreshTags: (vaultId: string) => Promise<void>;
  setNotice: (message: string | null) => void;
  setActionError: (message: string | null) => void;
  setMode: (mode: VaultMode) => void;
  bumpGraphSeed: () => void;
  setSearchQuery: (query: string) => void;
};

export function useVaultNotes({
  selectedVaultId,
  entries,
  refreshTree,
  refreshTags,
  setNotice,
  setActionError,
  setMode,
  bumpGraphSeed,
  setSearchQuery,
}: UseVaultNotesOptions) {
  const [selectedNoteId, setSelectedNoteId] = useState<string | null>(null);
  const [note, setNote] = useState<VaultNote | null>(null);
  const [noteLoading, setNoteLoading] = useState(false);
  const [noteError, setNoteError] = useState<string | null>(null);
  const [draftTitle, setDraftTitle] = useState("");
  const [draftBody, setDraftBody] = useState("");
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const openRequest = useRef(0);
  const vaultRef = useRef(selectedVaultId);
  vaultRef.current = selectedVaultId;
  const bodyRef = useRef<HTMLTextAreaElement>(null);

  const [renaming, setRenaming] = useState(false);
  const [renamePath, setRenamePath] = useState("");
  const [renameBusy, setRenameBusy] = useState(false);
  const [asideTab, setAsideTab] = useState<AsideTab>("backlinks");
  const [propertiesSaving, setPropertiesSaving] = useState(false);
  const [diaryBusy, setDiaryBusy] = useState(false);
  const [quickOpen, setQuickOpen] = useState(false);
  const [completion, setCompletion] = useState<CompletionState | null>(null);

  const [backlinks, setBacklinks] = useState<Backlink[]>([]);
  const [backlinksLoading, setBacklinksLoading] = useState(false);
  const [backlinksError, setBacklinksError] = useState<string | null>(null);

  const [templates, setTemplates] = useState<VaultEntry[]>([]);
  const [templatePath, setTemplatePath] = useState("");

  const [creatingPath, setCreatingPath] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  const completionItems = useMemo(() => {
    if (!completion) return [];
    const needle = completion.query.trim().toLowerCase();
    return entries
      .filter((entry) => !entry.is_dir && entry.id)
      .filter((entry) => {
        if (!needle) return true;
        const title = (entry.title || noteTitle(entry.path)).toLowerCase();
        return title.includes(needle);
      })
      .slice(0, 8);
  }, [completion, entries]);

  const completionActiveIndex =
    completionItems.length === 0
      ? 0
      : Math.min(completion?.active ?? 0, completionItems.length - 1);

  useEffect(() => {
    if (!selectedVaultId) {
      openRequest.current += 1;
      setNote(null);
      setSelectedNoteId(null);
      setNoteLoading(false);
      setNoteError(null);
      setBacklinks([]);
      setBacklinksError(null);
      setBacklinksLoading(false);
      setTemplates([]);
      setTemplatePath("");
      setQuickOpen(false);
      setCompletion(null);
      setRenaming(false);
      return;
    }

    openRequest.current += 1;
    setNote(null);
    setSelectedNoteId(null);
    setNoteLoading(false);
    setNoteError(null);
    setDraftTitle("");
    setDraftBody("");
    setBacklinks([]);
    setBacklinksError(null);
    setBacklinksLoading(false);
    setTemplates([]);
    setTemplatePath("");
    setConfirmingDelete(false);
    setQuickOpen(false);
    setCompletion(null);
    setRenaming(false);
    setRenamePath("");
    setPropertiesSaving(false);
    setDiaryBusy(false);
    setAsideTab("backlinks");
  }, [selectedVaultId]);

  useEffect(() => {
    if (!selectedVaultId) {
      setTemplates([]);
      setTemplatePath("");
      return;
    }
    let cancelled = false;
    setTemplates([]);
    setTemplatePath("");
    listVaultTemplates(selectedVaultId)
      .then((loaded) => {
        if (!cancelled) setTemplates(loaded);
      })
      .catch((error) => {
        if (!cancelled) setActionError(errorText(error));
      });
    return () => {
      cancelled = true;
    };
  }, [selectedVaultId]);

  useEffect(() => {
    function handleShortcut(event: KeyboardEvent) {
      if (!event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) {
        return;
      }
      if (event.key.toLowerCase() !== "o") return;
      if (!vaultRef.current) return;
      event.preventDefault();
      setQuickOpen(true);
    }
    window.addEventListener("keydown", handleShortcut);
    return () => window.removeEventListener("keydown", handleShortcut);
  }, []);

  async function loadBacklinks(noteId: string, request: number) {
    setBacklinksLoading(true);
    setBacklinksError(null);
    try {
      const links = await noteBacklinks(noteId);
      if (request === openRequest.current) setBacklinks(links);
    } catch (error) {
      if (request === openRequest.current) {
        setBacklinksError(errorText(error));
      }
    } finally {
      if (request === openRequest.current) setBacklinksLoading(false);
    }
  }

  async function openNote(noteId: string) {
    const request = ++openRequest.current;
    setSelectedNoteId(noteId);
    setNoteLoading(true);
    setNoteError(null);
    setActionError(null);
    setConfirmingDelete(false);
    setNote(null);
    setDraftTitle("");
    setDraftBody("");
    setBacklinks([]);
    setBacklinksError(null);
    setCompletion(null);
    setRenaming(false);
    setRenamePath("");
    try {
      const loaded = await getVaultNote(noteId);
      if (request !== openRequest.current) return;
      setNote(loaded);
      setSelectedNoteId(loaded.id);
      setDraftTitle(loaded.title);
      setDraftBody(loaded.body);
      void loadBacklinks(loaded.id, request);
    } catch (error) {
      if (request === openRequest.current) setNoteError(errorText(error));
    } finally {
      if (request === openRequest.current) setNoteLoading(false);
    }
  }

  function insertAtCursor(text: string, at?: number) {
    const textarea = bodyRef.current;
    const start = at ?? textarea?.selectionStart ?? null;
    const end = at ?? textarea?.selectionEnd ?? start;
    setCompletion(null);
    setDraftBody((previous) => {
      const from = start ?? previous.length;
      const to = end ?? from;
      return previous.slice(0, from) + text + previous.slice(to);
    });
    if (start === null) return;
    window.requestAnimationFrame(() => {
      const target = bodyRef.current;
      if (!target) return;
      target.focus();
      const caret = start + text.length;
      target.setSelectionRange(caret, caret);
    });
  }

  function syncCompletion(value: string, caret: number | null) {
    if (caret == null) {
      setCompletion(null);
      return;
    }
    const match = /\[\[([^\[\]\n]*)$/.exec(value.slice(0, caret));
    if (!match) {
      setCompletion(null);
      return;
    }
    setCompletion({
      start: caret - match[0].length,
      query: match[1],
      active: 0,
    });
  }

  function pickCompletion(entry: VaultEntry) {
    if (!completion) return;
    const start = completion.start;
    const trigger = `[[${completion.query}`;
    const end = start + trigger.length;
    if (draftBody.slice(start, end) !== trigger) {
      setCompletion(null);
      return;
    }
    const title = entry.title || noteTitle(entry.path);
    const duplicate = entries.some(
      (other) =>
        !other.is_dir &&
        other.id !== entry.id &&
        (other.title || noteTitle(other.path)).trim().toLowerCase() ===
          title.trim().toLowerCase(),
    );
    const linkText = duplicate ? entry.path.replace(/\.md$/i, "") : title;
    const inserted = `[[${linkText}]]`;
    setDraftBody(draftBody.slice(0, start) + inserted + draftBody.slice(end));
    setCompletion(null);
    window.requestAnimationFrame(() => {
      const target = bodyRef.current;
      if (!target) return;
      target.focus();
      const position = start + inserted.length;
      target.setSelectionRange(position, position);
    });
  }

  function handleBodyKeyDown(event: ReactKeyboardEvent<HTMLTextAreaElement>) {
    if (event.nativeEvent.isComposing) return;
    if (!completion) return;
    if (event.key === "Escape") {
      event.preventDefault();
      setCompletion(null);
      return;
    }
    if (completionItems.length === 0) return;
    const activeIndex = Math.min(
      completion.active,
      completionItems.length - 1,
    );
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setCompletion({
        ...completion,
        active: (activeIndex + 1) % completionItems.length,
      });
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      setCompletion({
        ...completion,
        active:
          (activeIndex - 1 + completionItems.length) %
          completionItems.length,
      });
      return;
    }
    if (event.key === "Enter" || event.key === "Tab") {
      event.preventDefault();
      pickCompletion(completionItems[activeIndex]);
    }
  }

  function jumpToLine(line: number) {
    const textarea = bodyRef.current;
    if (!textarea) return;
    const { start, end } = offsetForLine(draftBody, line);
    textarea.focus();
    textarea.setSelectionRange(start, end);
    const style = window.getComputedStyle(textarea);
    const lineHeight =
      Number.parseFloat(style.lineHeight) ||
      Number.parseFloat(style.fontSize) * 1.6;
    textarea.scrollTop = Math.max(
      0,
      (line - 1) * lineHeight - textarea.clientHeight / 3,
    );
  }

  async function saveProperties(
    props: Record<string, string>,
  ): Promise<void> {
    if (!note || propertiesSaving) return;
    const request = openRequest.current;
    const vaultId = note.vault_id;
    const noteId = note.id;
    const nextTitle = draftTitle.trim() || noteTitle(note.path);
    const dirty = nextTitle !== note.title || draftBody !== note.body;
    setPropertiesSaving(true);
    setActionError(null);
    try {
      if (dirty) {
        const saved = await saveVaultNote({
          note_id: noteId,
          title: nextTitle,
          body: draftBody,
        });
        if (request !== openRequest.current) return;
        setNote(saved);
        setDraftTitle(saved.title);
        setDraftBody(saved.body);
        await refreshTree(vaultId, false);
        await refreshTags(vaultId);
        if (request !== openRequest.current) return;
        bumpGraphSeed();
      }
      const updated = await setNoteProperties(noteId, props);
      if (request !== openRequest.current) return;
      setNote(updated);
      setDraftTitle(updated.title);
      setDraftBody(updated.body);
      setCompletion(null);
      setNotice("已更新属性");
      await refreshTags(vaultId);
      if (request !== openRequest.current) return;
      void loadBacklinks(updated.id, request);
    } catch (error) {
      if (request === openRequest.current) setActionError(errorText(error));
      throw error;
    } finally {
      setPropertiesSaving(false);
    }
  }

  async function submitRename() {
    if (!note || renameBusy) return;
    const newPath = normalizeNotePath(renamePath);
    if (!newPath) {
      setActionError("请输入合法的笔记路径，例如 文件夹/笔记.md。");
      return;
    }
    if (newPath === note.path) {
      setRenaming(false);
      return;
    }
    const noteId = note.id;
    const vaultId = note.vault_id;
    setRenameBusy(true);
    setActionError(null);
    try {
      const renamed = await renameVaultNote(noteId, newPath);
      if (vaultId !== vaultRef.current) return;
      setRenaming(false);
      setRenamePath("");
      await refreshTree(vaultId, false);
      await refreshTags(vaultId);
      if (vaultId !== vaultRef.current) return;
      bumpGraphSeed();
      await openNote(renamed.id);
      setNotice(`已重命名为 ${renamed.path}`);
    } catch (error) {
      setActionError(errorText(error));
    } finally {
      setRenameBusy(false);
    }
  }

  async function openDaily() {
    if (!selectedVaultId || diaryBusy) return;
    const vaultId = selectedVaultId;
    const day = localDate();
    const dailyPath = `Daily/${day}.md`;
    setDiaryBusy(true);
    setActionError(null);
    try {
      const existing = entries.find(
        (entry) =>
          !entry.is_dir &&
          entry.path.toLowerCase() === dailyPath.toLowerCase(),
      );
      if (existing?.id) {
        setMode("editor");
        await openNote(existing.id);
        return;
      }
      const template = templates.find(
        (entry) => entry.path.toLowerCase() === "templates/daily.md",
      );
      let dailyTemplate = template;
      if (!dailyTemplate && templates.length === 0) {
        const loadedTemplates = await listVaultTemplates(vaultId);
        if (vaultId !== vaultRef.current) return;
        setTemplates(loadedTemplates);
        dailyTemplate = loadedTemplates.find(
          (entry) => entry.path.toLowerCase() === "templates/daily.md",
        );
      }
      let content = `# ${day}`;
      const dailyTemplateId = dailyTemplate?.id;
      if (dailyTemplateId) {
        let templateBody: string;
        try {
          templateBody = (await getVaultNote(dailyTemplateId)).body;
        } catch {
          if (vaultId !== vaultRef.current) return;
          await indexVault(vaultId);
          if (vaultId !== vaultRef.current) return;
          templateBody = (await getVaultNote(dailyTemplateId)).body;
        }
        content = applyTemplateVariables(templateBody, dailyPath);
      }
      const created = await createVaultNoteWithContent(
        vaultId,
        dailyPath,
        content,
      );
      if (vaultId !== vaultRef.current) return;
      setMode("editor");
      await refreshTree(vaultId, false);
      await refreshTags(vaultId);
      if (vaultId !== vaultRef.current) return;
      await openNote(created.id);
      setNotice(`已打开 ${created.path}`);
    } catch (error) {
      if (vaultId === vaultRef.current) setActionError(errorText(error));
    } finally {
      setDiaryBusy(false);
    }
  }

  function startCreate(folder: string) {
    setCreatingPath(uniqueNotePath(entries, folder));
    setCreateError(null);
    setSearchQuery("");
    setCompletion(null);
  }

  async function submitCreate() {
    if (!selectedVaultId || creatingPath === null || creating) return;
    const path = normalizeNotePath(creatingPath);
    if (!path) {
      setCreateError("请输入合法的笔记路径，例如 灵感/想法.md。");
      return;
    }
    const vaultId = selectedVaultId;
    setCreating(true);
    setCreateError(null);
    try {
      const template = templates.find(
        (entry) => entry.path === templatePath && entry.id,
      );
      let created: VaultNote;
      if (template?.id) {
        const loaded = await getVaultNote(template.id);
        created = await createVaultNoteWithContent(
          vaultId,
          path,
          applyTemplateVariables(loaded.body, path),
        );
      } else {
        created = await createVaultNote({
          vault_id: vaultId,
          path,
          title: noteTitle(path),
          body: "",
        });
      }
      if (vaultId !== vaultRef.current) return;
      setCreatingPath(null);
      setMode("editor");
      await refreshTree(vaultId, false);
      await refreshTags(vaultId);
      if (vaultId !== vaultRef.current) return;
      await openNote(created.id);
      setNotice(`已创建 ${created.path}`);
    } catch (error) {
      const message = errorText(error);
      setCreateError(message);
      setActionError(message);
    } finally {
      setCreating(false);
    }
  }

  async function saveNote() {
    if (!note || saving) return;
    const request = openRequest.current;
    const vaultId = note.vault_id;
    setSaving(true);
    setActionError(null);
    try {
      const saved = await saveVaultNote({
        note_id: note.id,
        title: draftTitle.trim() || noteTitle(note.path),
        body: draftBody,
      });
      await refreshTree(vaultId, false);
      await refreshTags(vaultId);
      if (request !== openRequest.current) return;
      setNote(saved);
      setDraftTitle(saved.title);
      setDraftBody(saved.body);
      setNotice(`已保存 ${saved.path}`);
      bumpGraphSeed();
      void loadBacklinks(saved.id, request);
    } catch (error) {
      if (request === openRequest.current) {
        setActionError(errorText(error));
      }
    } finally {
      setSaving(false);
    }
  }

  async function removeNote() {
    if (!note || deleting) return;
    if (!confirmingDelete) {
      setConfirmingDelete(true);
      return;
    }
    const request = openRequest.current;
    const vaultId = note.vault_id;
    const removedPath = note.path;
    setDeleting(true);
    setActionError(null);
    try {
      await deleteVaultNote(note.id);
      await refreshTree(vaultId, false);
      await refreshTags(vaultId);
      if (vaultId === vaultRef.current) {
        bumpGraphSeed();
      }
      if (request === openRequest.current) {
        openRequest.current += 1;
        setNote(null);
        setSelectedNoteId(null);
        setDraftTitle("");
        setDraftBody("");
        setBacklinks([]);
        setCompletion(null);
        setRenaming(false);
        setConfirmingDelete(false);
      }
      if (vaultId === vaultRef.current) {
        setNotice(`已删除 ${removedPath}`);
      }
    } catch (error) {
      if (request === openRequest.current) {
        setActionError(errorText(error));
      }
    } finally {
      setDeleting(false);
    }
  }

  return {
    selectedNoteId,
    note,
    noteLoading,
    noteError,
    draftTitle,
    setDraftTitle,
    draftBody,
    setDraftBody,
    saving,
    deleting,
    confirmingDelete,
    renaming,
    setRenaming,
    renamePath,
    setRenamePath,
    renameBusy,
    asideTab,
    setAsideTab,
    propertiesSaving,
    diaryBusy,
    quickOpen,
    setQuickOpen,
    completion,
    completionItems,
    completionActiveIndex,
    backlinks,
    backlinksLoading,
    backlinksError,
    templates,
    templatePath,
    setTemplatePath,
    creatingPath,
    setCreatingPath,
    creating,
    createError,
    setCreateError,
    openRequest,
    bodyRef,
    openNote,
    insertAtCursor,
    syncCompletion,
    pickCompletion,
    handleBodyKeyDown,
    jumpToLine,
    saveProperties,
    submitRename,
    openDaily,
    startCreate,
    submitCreate,
    saveNote,
    removeNote,
  };
}
