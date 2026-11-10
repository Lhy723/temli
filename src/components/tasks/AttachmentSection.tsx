import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ChangeEvent,
} from "react";
import { Button } from "@heroui/react";
import { FileText, Paperclip, Trash2, Upload, X } from "lucide-react";
import {
  addTaskAttachment,
  deleteTaskAttachment,
  errorText,
  formatDate,
  listTaskAttachments,
  readTaskAttachment,
  type Attachment,
} from "../../lib/api";
import { Muted, Section } from "./shared";

const MAX_TASK_ATTACHMENT_BYTES = 20 * 1024 * 1024;

function formatBytes(size: number): string {
  if (!Number.isFinite(size) || size < 0) return "—";
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}

function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = typeof reader.result === "string" ? reader.result : "";
      const comma = result.indexOf(",");
      resolve(comma >= 0 ? result.slice(comma + 1) : result);
    };
    reader.onerror = () => reject(reader.error ?? new Error("读取文件失败"));
    reader.readAsDataURL(file);
  });
}

/** 任务详情里的证据附件：上传、缩略图预览、删除与状态反馈。 */
function AttachmentSection({
  taskId,
  revision,
  onChanged,
}: {
  taskId: string;
  revision: number;
  onChanged: () => void;
}) {
  const [items, setItems] = useState<Attachment[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [uploading, setUploading] = useState(false);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [confirmingId, setConfirmingId] = useState<string | null>(null);
  const [previews, setPreviews] = useState<Record<string, string>>({});
  const [previewErrors, setPreviewErrors] = useState<Record<string, true>>({});
  const [preview, setPreview] = useState<{
    id: string;
    url: string;
    name: string;
  } | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const request = useRef(0);
  const loadedOnce = useRef(false);

  const load = useCallback(
    async (showLoading = true): Promise<void> => {
      const current = ++request.current;
      if (showLoading) setLoading(true);
      setError(null);
      try {
        const rows = await listTaskAttachments(taskId);
        if (request.current !== current) return;
        loadedOnce.current = true;
        setItems(rows);
        setPreviews({});
        setPreviewErrors({});
        setLoading(false);
        await Promise.all(
          rows.filter((item) =>
            item.mime.toLowerCase().startsWith("image/"),
          ).map(async (item) => {
            try {
              const base64 = await readTaskAttachment(item.id);
              if (request.current !== current) return;
              setPreviews((prev) => ({
                ...prev,
                [item.id]: `data:${item.mime || "application/octet-stream"};base64,${base64}`,
              }));
            } catch {
              if (request.current === current) {
                setPreviewErrors((prev) => ({ ...prev, [item.id]: true }));
              }
            }
          }),
        );
      } catch (e) {
        if (request.current !== current) return;
        setError(errorText(e));
        setLoading(false);
      }
    },
    [taskId],
  );

  // revision 由父级在上传/删除成功后递增；即使完成时本实例已被卸载，
  // 任务重新挂载后也会因 revision 变化重新拉取。已加载过就不再显示
  // “加载中”，避免刷新时列表闪烁。
  useEffect(() => {
    void load(!loadedOnce.current);
    return () => {
      request.current += 1;
    };
  }, [load, revision]);

  useEffect(() => {
    if (!preview) return;
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") setPreview(null);
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [preview]);

  async function upload(file: File) {
    if (uploading) return;
    if (file.size > MAX_TASK_ATTACHMENT_BYTES) {
      setError(`附件超过 20 MB，无法上传（${formatBytes(file.size)}）。`);
      return;
    }
    setUploading(true);
    setError(null);
    try {
      const dataBase64 = await fileToBase64(file);
      await addTaskAttachment(
        taskId,
        file.name || `附件-${Date.now()}`,
        dataBase64,
      );
      onChanged();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setUploading(false);
    }
  }

  function handleInput(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (file) void upload(file);
  }

  async function remove(item: Attachment) {
    if (deletingId !== null) return;
    if (confirmingId !== item.id) {
      setConfirmingId(item.id);
      return;
    }
    setDeletingId(item.id);
    setError(null);
    try {
      await deleteTaskAttachment(item.id);
      setConfirmingId(null);
      setItems((prev) =>
        prev === null ? prev : prev.filter((row) => row.id !== item.id),
      );
      if (preview?.id === item.id) setPreview(null);
      onChanged();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setDeletingId(null);
    }
  }

  return (
    <>
      <Section
        title="证据附件"
        icon={<Paperclip size={14} />}
        count={items?.length ?? 0}
      >
        <div className="flex flex-wrap items-center gap-1.5">
          <Button
            variant="secondary"
            size="sm"
            className="h-7 gap-1 px-2.5 text-ui"
            onPress={() => inputRef.current?.click()}
            isDisabled={uploading || loading || deletingId !== null}
          >
            <Upload size={14} />
            {uploading ? "上传中…" : "上传附件"}
          </Button>
          <span className="text-label text-[var(--muted)]">
            单个文件不超过 20 MB；图片会显示缩略图。
          </span>
        </div>
        <input
          ref={inputRef}
          type="file"
          className="hidden"
          aria-label="选择证据附件"
          onChange={handleInput}
          disabled={uploading}
        />
        {error && (
          <p role="alert" className="text-ui-sm text-[var(--danger)]">
            {error}
          </p>
        )}
        {loading ? (
          <Muted>加载中…</Muted>
        ) : items === null ? (
          <div className="flex items-center gap-2">
            <Muted>无法加载证据附件。</Muted>
            <Button
              variant="secondary"
              size="sm"
              className="h-6 px-2 text-label"
              onPress={() => void load()}
            >
              重试
            </Button>
          </div>
        ) : items.length === 0 ? (
          <Muted>暂无证据附件。</Muted>
        ) : (
          <div className="overflow-hidden rounded-[10px] border border-[var(--border)]">
            {items.map((item) => {
              const image = item.mime.toLowerCase().startsWith("image/");
              const thumbnail = previews[item.id];
              const confirming = confirmingId === item.id;
              const deleting = deletingId === item.id;
              return (
                <div
                  key={item.id}
                  className="flex items-center gap-2 border-b border-[var(--border)] px-2 py-1.5 last:border-b-0"
                >
                  {image ? (
                    <button
                      type="button"
                      className="flex size-9 shrink-0 items-center justify-center overflow-hidden rounded-md border border-[var(--border)] bg-[var(--background-secondary)]"
                      aria-label={`预览附件：${item.file_name}`}
                      title={thumbnail ? "点击放大" : undefined}
                      onClick={() => {
                        if (!thumbnail) return;
                        setPreview({
                          id: item.id,
                          url: thumbnail,
                          name: item.file_name,
                        });
                      }}
                      disabled={!thumbnail}
                    >
                      {thumbnail ? (
                        <img
                          src={thumbnail}
                          alt={item.file_name}
                          className="h-full w-full object-cover"
                        />
                      ) : (
                        <span className="text-label text-[var(--muted)]">
                          {previewErrors[item.id] ? "无法预览" : "…"}
                        </span>
                      )}
                    </button>
                  ) : (
                    <span className="flex size-9 shrink-0 items-center justify-center rounded-md border border-[var(--border)] bg-[var(--background-secondary)] text-[var(--muted)]">
                      <FileText size={15} />
                    </span>
                  )}
                  <div className="min-w-0 flex-1">
                    <p
                      className="truncate text-ui-sm"
                      title={item.file_name}
                    >
                      {item.file_name}
                    </p>
                    <p className="text-label text-[var(--muted)]">
                      {formatBytes(item.size)}
                      {image && previewErrors[item.id] ? " · 无法预览" : ""}
                      {item.created_at
                        ? ` · ${formatDate(item.created_at)}`
                        : ""}
                    </p>
                  </div>
                  <Button
                    variant={confirming ? "danger-soft" : "ghost"}
                    size="sm"
                    className={`h-6 shrink-0 px-2 text-label ${
                      confirming ? "" : "text-[var(--muted)]"
                    }`}
                    aria-label={
                      confirming
                        ? `确认删除附件：${item.file_name}`
                        : `删除附件：${item.file_name}`
                    }
                    onPress={() => void remove(item)}
                    isDisabled={deletingId !== null || uploading}
                  >
                    {deleting ? (
                      "删除中…"
                    ) : confirming ? (
                      "确认删除"
                    ) : (
                      <Trash2 size={13} />
                    )}
                  </Button>
                </div>
              );
            })}
          </div>
        )}
      </Section>

      {preview && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-6"
          role="dialog"
          aria-modal="true"
          aria-label={`预览 ${preview.name}`}
          onClick={() => setPreview(null)}
        >
          <button
            type="button"
            className="absolute right-4 top-4 flex size-8 items-center justify-center rounded-full bg-black/50 text-white"
            aria-label="关闭预览"
            onClick={() => setPreview(null)}
          >
            <X size={16} />
          </button>
          <img
            src={preview.url}
            alt={preview.name}
            className="max-h-full max-w-full rounded-lg object-contain"
            onClick={(event) => event.stopPropagation()}
          />
        </div>
      )}
    </>
  );
}


export { AttachmentSection };
