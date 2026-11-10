import {
  useMemo,
  type ChangeEvent,
  type ClipboardEvent,
  type ComponentProps,
  type DragEvent as ReactDragEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type RefObject,
} from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Button, Input, TextArea } from "@heroui/react";
import {
  FileText,
  NotebookPen,
  Paperclip,
  Pencil,
  Save,
  Trash2,
  X,
} from "lucide-react";
import { formatDate, type VaultEntry, type VaultNote } from "../../lib/api";
import { VaultImage } from "./VaultImage";
import { MARKDOWN_CLASS, noteTitle, type CompletionState } from "./shared";

export function NoteEditor({
  note,
  noteLoading,
  noteError,
  draftTitle,
  onDraftTitleChange,
  draftBody,
  onDraftBodyChange,
  saving,
  deleting,
  confirmingDelete,
  renaming,
  renamePath,
  onRenamePathChange,
  renameBusy,
  onStartRename,
  onCancelRename,
  onSubmitRename,
  onSave,
  onRemove,
  actionError,
  completion,
  completionItems,
  completionActiveIndex,
  onPickCompletion,
  onBodyKeyDown,
  bodyRef,
  attachmentInputRef,
  onAttachmentClick,
  onAttachmentInput,
  onBodyPaste,
  onBodyDragEnter,
  onBodyDragOver,
  onBodyDragLeave,
  onBodyDrop,
  attachmentDragActive,
  uploading,
  vaultId,
}: {
  note: VaultNote | null;
  noteLoading: boolean;
  noteError: string | null;
  draftTitle: string;
  onDraftTitleChange: (title: string) => void;
  draftBody: string;
  onDraftBodyChange: (body: string, caret: number | null) => void;
  saving: boolean;
  deleting: boolean;
  confirmingDelete: boolean;
  renaming: boolean;
  renamePath: string;
  onRenamePathChange: (path: string) => void;
  renameBusy: boolean;
  onStartRename: () => void;
  onCancelRename: () => void;
  onSubmitRename: () => void;
  onSave: () => void;
  onRemove: () => void;
  actionError: string | null;
  completion: CompletionState | null;
  completionItems: VaultEntry[];
  completionActiveIndex: number;
  onPickCompletion: (entry: VaultEntry) => void;
  onBodyKeyDown: (event: ReactKeyboardEvent<HTMLTextAreaElement>) => void;
  bodyRef: RefObject<HTMLTextAreaElement | null>;
  attachmentInputRef: RefObject<HTMLInputElement | null>;
  onAttachmentClick: () => void;
  onAttachmentInput: (event: ChangeEvent<HTMLInputElement>) => void;
  onBodyPaste: (event: ClipboardEvent<HTMLTextAreaElement>) => void;
  onBodyDragEnter: (event: ReactDragEvent<HTMLTextAreaElement>) => void;
  onBodyDragOver: (event: ReactDragEvent<HTMLTextAreaElement>) => void;
  onBodyDragLeave: (event: ReactDragEvent<HTMLTextAreaElement>) => void;
  onBodyDrop: (event: ReactDragEvent<HTMLTextAreaElement>) => void;
  attachmentDragActive: boolean;
  uploading: boolean;
  vaultId: string;
}) {
  const markdownComponents = useMemo(
    () => ({
      img: ({ src, alt, title }: ComponentProps<"img">) => (
        <VaultImage src={src} alt={alt} title={title} vaultId={vaultId} />
      ),
    }),
    [vaultId],
  );

  return (
    <>
      <header className="flex min-h-12 shrink-0 flex-wrap items-center gap-2 border-b border-[var(--border)] px-3 py-1.5">
        <div className="min-w-0 flex-1">
          {note && !noteLoading ? (
            <>
              <Input
                aria-label="笔记标题"
                placeholder="笔记标题"
                value={draftTitle}
                onChange={(e) => onDraftTitleChange(e.target.value)}
                disabled={saving || deleting}
                className="text-ui"
              />
              <p
                className="mt-0.5 truncate font-mono text-label text-[var(--muted)]"
                title={note.path}
              >
                {note.path} · 更新于 {formatDate(note.updated_at)}
              </p>
              {renaming && (
                <div className="mt-1 flex items-center gap-1">
                  <Input
                    aria-label="笔记新路径"
                    placeholder="文件夹/笔记.md"
                    value={renamePath}
                    onChange={(e) => onRenamePathChange(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") onSubmitRename();
                      if (e.key === "Escape") onCancelRename();
                    }}
                    disabled={renameBusy}
                    className="min-w-0 flex-1"
                    autoFocus
                  />
                  <Button
                    variant="primary"
                    size="sm"
                    className="h-7 shrink-0 px-2 text-ui"
                    onPress={onSubmitRename}
                    isDisabled={renameBusy}
                  >
                    {renameBusy ? "移动中…" : "确定"}
                  </Button>
                  <Button
                    isIconOnly
                    variant="ghost"
                    size="sm"
                    className="h-7 w-7 shrink-0"
                    aria-label="取消重命名"
                    onPress={onCancelRename}
                    isDisabled={renameBusy}
                  >
                    <X size={14} />
                  </Button>
                </div>
              )}
            </>
          ) : noteLoading ? (
            <p className="text-ui text-[var(--muted)]">加载中…</p>
          ) : (
            <p className="text-ui text-[var(--muted)]">
              选择或新建一条笔记
            </p>
          )}
        </div>

        {note && !noteLoading && (
          <>
            <Button
              isIconOnly
              variant="ghost"
              size="sm"
              className="h-7 w-7 shrink-0"
              aria-label="添加附件"
              onPress={onAttachmentClick}
              isDisabled={saving || deleting || uploading}
            >
              <Paperclip size={14} />
            </Button>
            <Button
              isIconOnly
              variant="ghost"
              size="sm"
              className="h-7 w-7 shrink-0"
              aria-label="重命名笔记"
              onPress={onStartRename}
              isDisabled={saving || deleting || renameBusy}
            >
              <Pencil size={14} />
            </Button>
            <Button
              variant="secondary"
              size="sm"
              className="h-7 shrink-0 gap-1 px-2.5 text-ui"
              onPress={onSave}
              isDisabled={saving || deleting}
            >
              <Save size={14} />
              {saving ? "保存中…" : "保存"}
            </Button>
            <Button
              variant="danger-soft"
              size="sm"
              className="h-7 shrink-0 gap-1 px-2.5 text-ui"
              onPress={onRemove}
              isDisabled={saving || deleting}
              aria-label="删除笔记文件"
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
      </header>

      <input
        ref={attachmentInputRef}
        type="file"
        className="hidden"
        aria-label="选择附件文件"
        onChange={onAttachmentInput}
      />

      {actionError && (
        <p
          role="alert"
          className="shrink-0 border-b border-[var(--border)] px-3 py-1.5 text-ui-sm text-[var(--danger)]"
        >
          {actionError}
        </p>
      )}

      {noteLoading ? (
        <p className="p-3 text-ui-sm text-[var(--muted)]">加载中…</p>
      ) : noteError ? (
        <p role="alert" className="p-3 text-ui-sm text-[var(--danger)]">
          {noteError}
        </p>
      ) : note ? (
        <div className="grid min-h-0 flex-1 grid-rows-[minmax(0,1fr)_minmax(0,1fr)] gap-2 p-3">
          <div className="flex min-h-0 flex-col gap-1">
            {completion && completionItems.length > 0 && (
              <div className="max-h-[140px] shrink-0 overflow-auto rounded-md border border-[var(--border)] bg-[var(--background-secondary)]">
                {completionItems.map((entry, index) => (
                  <button
                    key={entry.path}
                    type="button"
                    onMouseDown={(event) => {
                      event.preventDefault();
                      onPickCompletion(entry);
                    }}
                    className={`anim-fast flex w-full items-center gap-1.5 px-2 py-1 text-left text-ui-sm ${
                      completionActiveIndex === index
                        ? "bg-[var(--background-tertiary)] text-[var(--foreground)]"
                        : "text-[var(--muted)] hover:bg-[var(--background-tertiary)]"
                    }`}
                  >
                    <FileText size={12} className="shrink-0" />
                    <span className="truncate">
                      {entry.title || noteTitle(entry.path)}
                    </span>
                    <span className="ml-auto max-w-[45%] shrink-0 truncate font-mono text-label opacity-70">
                      {entry.path}
                    </span>
                  </button>
                ))}
              </div>
            )}
            <TextArea
              ref={bodyRef}
              aria-label="笔记正文"
              placeholder="用 Markdown 记录想法，[[另一条笔记]] 建立双链…"
              value={draftBody}
              onChange={(e) => {
                onDraftBodyChange(e.target.value, e.target.selectionStart);
              }}
              onKeyDown={onBodyKeyDown}
              onPaste={onBodyPaste}
              onDragEnter={onBodyDragEnter}
              onDragOver={onBodyDragOver}
              onDragLeave={onBodyDragLeave}
              onDrop={onBodyDrop}
              disabled={saving || deleting}
              className={`min-h-0 w-full flex-1 resize-none font-mono text-ui-sm ${
                attachmentDragActive
                  ? "border-[var(--accent)] ring-1 ring-[var(--accent)]"
                  : ""
              }`}
            />
          </div>
          <div className="min-h-0 overflow-auto rounded-md border border-[var(--border)] bg-[var(--background-secondary)] p-3">
            {draftBody.trim() ? (
              <div className={MARKDOWN_CLASS}>
                <Markdown
                  remarkPlugins={[remarkGfm]}
                  components={markdownComponents}
                >
                  {draftBody}
                </Markdown>
              </div>
            ) : (
              <p className="text-ui-sm text-[var(--muted)]">
                预览会显示在这里。
              </p>
            )}
          </div>
        </div>
      ) : (
        <div className="flex flex-1 flex-col items-center justify-center gap-2 text-center">
          <NotebookPen
            size={20}
            className="text-[var(--muted)] opacity-70"
          />
          <p className="text-ui text-[var(--muted)]">
            选择左侧笔记开始编辑，或新建一条 Markdown 笔记。
          </p>
        </div>
      )}
    </>
  );
}
