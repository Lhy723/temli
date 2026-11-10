import { useEffect, useState } from "react";
import { readVaultAttachment } from "../../lib/api";
import { attachmentPathFromSrc, imageMime } from "./shared";

/** 预览里的图片：http/data 直接显示，其余相对路径经 Tauri 读取 attachments。 */
export function VaultImage({
  src,
  alt,
  title,
  vaultId,
}: {
  src?: string;
  alt?: string;
  title?: string;
  vaultId: string;
}) {
  const external = !src || /^(?:https?:|\/\/|data:|blob:)/i.test(src);
  const [image, setImage] = useState<{ url: string | null; error: boolean }>({
    url: null,
    error: false,
  });

  useEffect(() => {
    if (!src || external || !vaultId) return;
    let cancelled = false;
    const relativePath = attachmentPathFromSrc(src);
    setImage({ url: null, error: false });
    readVaultAttachment(vaultId, relativePath)
      .then((base64) => {
        if (!cancelled) {
          setImage({
            url: `data:${imageMime(relativePath)};base64,${base64}`,
            error: false,
          });
        }
      })
      .catch(() => {
        if (!cancelled) setImage({ url: null, error: true });
      });
    return () => {
      cancelled = true;
    };
  }, [src, external, vaultId]);

  if (!src) return null;
  if (external) return <img src={src} alt={alt ?? ""} title={title} />;
  if (image.error) {
    return (
      <span className="text-ui-sm text-[var(--danger)]" title={title ?? src}>
        图片加载失败：{src}
      </span>
    );
  }
  if (!image.url) {
    return (
      <span
        className="inline-block rounded bg-[var(--background-tertiary)] px-2 py-1 text-ui-sm text-[var(--muted)]"
        title={title ?? src}
      >
        图片加载中…
      </span>
    );
  }
  return <img src={image.url} alt={alt ?? ""} title={title} />;
}
