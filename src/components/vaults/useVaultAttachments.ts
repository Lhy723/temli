import {
  useEffect,
  useRef,
  useState,
  type ChangeEvent,
  type ClipboardEvent,
  type DragEvent as ReactDragEvent,
  type RefObject,
} from "react";
import { errorText, saveVaultAttachment, type VaultNote } from "../../lib/api";
import { fileToBase64, MAX_ATTACHMENT_BYTES } from "./shared";

type UseVaultAttachmentsOptions = {
  vaultId: string;
  note: VaultNote | null;
  openRequest: RefObject<number>;
  bodyRef: RefObject<HTMLTextAreaElement | null>;
  insertAtCursor: (text: string, at?: number) => void;
  setActionError: (message: string | null) => void;
  setNotice: (message: string) => void;
};

export function useVaultAttachments({
  vaultId,
  note,
  openRequest,
  bodyRef,
  insertAtCursor,
  setActionError,
  setNotice,
}: UseVaultAttachmentsOptions) {
  const [uploading, setUploading] = useState(false);
  const [attachmentDragActive, setAttachmentDragActive] = useState(false);
  const attachmentInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    setUploading(false);
  }, [vaultId]);

  async function saveAttachmentFile(
    file: File,
    noteId: string,
    vaultId: string,
  ) {
    const extension = file.type.startsWith("image/")
      ? file.type.slice("image/".length).replace("jpeg", "jpg").split("+")[0]
      : "bin";
    const fileName = file.name || `附件-${Date.now()}.${extension || "bin"}`;
    const dataBase64 = await fileToBase64(file);
    const relativePath = await saveVaultAttachment(
      vaultId,
      noteId,
      fileName,
      dataBase64,
    );
    return {
      relativePath,
      marker: file.type.startsWith("image/")
        ? `![${fileName}](${relativePath})`
        : `[${fileName}](${relativePath})`,
    };
  }

  async function uploadAttachment(file: File) {
    if (!note || uploading) return;
    if (file.size > MAX_ATTACHMENT_BYTES) {
      setActionError("附件超过 25 MB，无法插入。");
      return;
    }
    const request = openRequest.current;
    setUploading(true);
    setActionError(null);
    try {
      const { relativePath, marker } = await saveAttachmentFile(
        file,
        note.id,
        note.vault_id,
      );
      if (request !== openRequest.current) return;
      insertAtCursor(marker);
      setNotice(`已插入附件 ${relativePath}`);
    } catch (error) {
      if (request === openRequest.current) setActionError(errorText(error));
    } finally {
      setUploading(false);
    }
  }

  async function uploadAttachments(files: File[]) {
    if (!note || uploading || files.length === 0) return;
    const request = openRequest.current;
    const noteId = note.id;
    const vaultId = note.vault_id;
    const textarea = bodyRef.current;
    let caret = textarea?.selectionStart ?? null;
    const inserted: string[] = [];
    const failures: string[] = [];
    setUploading(true);
    setActionError(null);
    try {
      for (const file of files) {
        if (request !== openRequest.current) break;
        if (file.size > MAX_ATTACHMENT_BYTES) {
          failures.push(
            `${file.name || "未命名文件"} 超过 25 MB，无法插入。`,
          );
          if (request === openRequest.current) {
            setActionError(failures.join("；"));
          }
          continue;
        }
        try {
          const { relativePath, marker } = await saveAttachmentFile(
            file,
            noteId,
            vaultId,
          );
          if (request !== openRequest.current) break;
          insertAtCursor(marker, caret ?? undefined);
          if (caret !== null) caret += marker.length;
          inserted.push(relativePath);
        } catch (error) {
          failures.push(errorText(error));
          if (request === openRequest.current) {
            setActionError(failures.join("；"));
          }
        }
      }
    } finally {
      setUploading(false);
      if (request === openRequest.current) {
        if (inserted.length === 1) {
          setNotice(`已插入附件 ${inserted[0]}`);
        } else if (inserted.length > 1) {
          setNotice(`已插入 ${inserted.length} 个附件`);
        }
      }
    }
  }

  function handleAttachmentInput(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (file) void uploadAttachment(file);
  }

  function handleBodyPaste(event: ClipboardEvent<HTMLTextAreaElement>) {
    const item = Array.from(event.clipboardData.items).find(
      (entry) => entry.kind === "file" && entry.type.startsWith("image/"),
    );
    const file = item?.getAsFile();
    if (!file) return;
    event.preventDefault();
    void uploadAttachment(file);
  }

  function handleBodyDragEnter(event: ReactDragEvent<HTMLTextAreaElement>) {
    if (!Array.from(event.dataTransfer.types).includes("Files")) return;
    event.preventDefault();
    setAttachmentDragActive(true);
  }

  function handleBodyDragOver(event: ReactDragEvent<HTMLTextAreaElement>) {
    if (!Array.from(event.dataTransfer.types).includes("Files")) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
    if (!attachmentDragActive) setAttachmentDragActive(true);
  }

  function handleBodyDragLeave(event: ReactDragEvent<HTMLTextAreaElement>) {
    const next = event.relatedTarget as Node | null;
    if (next && event.currentTarget.contains(next)) return;
    setAttachmentDragActive(false);
  }

  function handleBodyDrop(event: ReactDragEvent<HTMLTextAreaElement>) {
    const files = Array.from(event.dataTransfer.files);
    if (files.length === 0) return;
    event.preventDefault();
    setAttachmentDragActive(false);
    void uploadAttachments(files);
  }

  return {
    uploading,
    attachmentDragActive,
    attachmentInputRef,
    handleAttachmentInput,
    handleBodyPaste,
    handleBodyDragEnter,
    handleBodyDragOver,
    handleBodyDragLeave,
    handleBodyDrop,
  };
}
