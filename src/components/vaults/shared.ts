import type { VaultEntry } from "../../lib/api";

export const MARKDOWN_CLASS = [
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

export type VaultMode = "editor" | "graph";

export type AsideTab = "backlinks" | "outline" | "properties";

export type CompletionState = {
  start: number;
  query: string;
  active: number;
};

export type TreeNode = {
  entry: VaultEntry;
  children: TreeNode[];
};

export function baseName(path: string): string {
  const index = path.lastIndexOf("/");
  return index >= 0 ? path.slice(index + 1) : path;
}

export function noteTitle(path: string): string {
  const name = baseName(path);
  return name.toLowerCase().endsWith(".md") ? name.slice(0, -3) : name;
}

function sortNodes(nodes: TreeNode[]) {
  nodes.sort((left, right) => {
    if (left.entry.is_dir !== right.entry.is_dir) {
      return left.entry.is_dir ? -1 : 1;
    }
    const leftTitle = left.entry.title || noteTitle(left.entry.path);
    const rightTitle = right.entry.title || noteTitle(right.entry.path);
    return leftTitle.localeCompare(rightTitle, "zh-CN");
  });
  for (const node of nodes) sortNodes(node.children);
}

/** 后端返回的是含目录的扁平列表；这里组装成可折叠树。 */
export function buildTree(entries: VaultEntry[]): TreeNode[] {
  const roots: TreeNode[] = [];
  const nodes = new Map<string, TreeNode>();

  function ensureDir(path: string): TreeNode {
    const existing = nodes.get(path);
    if (existing) return existing;
    const dir: TreeNode = {
      entry: { path, title: baseName(path), is_dir: true },
      children: [],
    };
    nodes.set(path, dir);
    const parentIndex = path.lastIndexOf("/");
    if (parentIndex < 0) {
      roots.push(dir);
    } else {
      ensureDir(path.slice(0, parentIndex)).children.push(dir);
    }
    return dir;
  }

  for (const entry of entries) {
    if (entry.is_dir) {
      ensureDir(entry.path);
      continue;
    }
    const node: TreeNode = { entry, children: [] };
    nodes.set(entry.path, node);
    const parentIndex = entry.path.lastIndexOf("/");
    if (parentIndex < 0) roots.push(node);
    else ensureDir(entry.path.slice(0, parentIndex)).children.push(node);
  }

  sortNodes(roots);
  return roots;
}

export function uniqueNotePath(entries: VaultEntry[], folder: string): string {
  const prefix = folder.replace(/^\/+|\/+$/g, "");
  const taken = new Set(
    entries.filter((entry) => !entry.is_dir).map((entry) => entry.path.toLowerCase()),
  );
  let attempt = 0;
  for (;;) {
    const name = attempt === 0 ? "新笔记.md" : `新笔记 ${attempt}.md`;
    const path = prefix ? `${prefix}/${name}` : name;
    if (!taken.has(path.toLowerCase())) return path;
    attempt += 1;
  }
}

export function normalizeNotePath(raw: string): string | null {
  const cleaned = raw.trim().replace(/^\/+/, "").replace(/\/+/g, "/");
  if (!cleaned) return null;
  const segments = cleaned.split("/");
  if (segments.some((segment) => !segment || segment === "." || segment === "..")) {
    return null;
  }
  return cleaned.toLowerCase().endsWith(".md") ? cleaned : `${cleaned}.md`;
}

export const MAX_ATTACHMENT_BYTES = 25 * 1024 * 1024;

function pad2(value: number): string {
  return String(value).padStart(2, "0");
}

export function localDate(date = new Date()): string {
  return `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())}`;
}

function localTime(date = new Date()): string {
  return `${pad2(date.getHours())}:${pad2(date.getMinutes())}`;
}

/** 模板变量：{{date}} / {{time}} / {{title}}（标题取路径文件名去 .md）。 */
export function applyTemplateVariables(body: string, path: string): string {
  return body
    .replace(/\{\{date\}\}/g, localDate())
    .replace(/\{\{time\}\}/g, localTime())
    .replace(/\{\{title\}\}/g, noteTitle(path));
}

/** 把 1-based 行号换算成正文中的字符区间。 */
export function offsetForLine(
  body: string,
  line: number,
): { start: number; end: number } {
  let start = 0;
  for (let index = 1; index < line; index += 1) {
    const newline = body.indexOf("\n", start);
    if (newline < 0) return { start: body.length, end: body.length };
    start = newline + 1;
  }
  const newline = body.indexOf("\n", start);
  return { start, end: newline < 0 ? body.length : newline };
}

export function attachmentPathFromSrc(src: string): string {
  const withoutQuery = src.split(/[?#]/, 1)[0].replace(/^\.\//, "");
  try {
    return decodeURIComponent(withoutQuery);
  } catch {
    return withoutQuery;
  }
}

export function imageMime(path: string): string {
  const extension = path.slice(path.lastIndexOf(".") + 1).toLowerCase();
  const known: Record<string, string> = {
    png: "image/png",
    jpg: "image/jpeg",
    jpeg: "image/jpeg",
    gif: "image/gif",
    webp: "image/webp",
    svg: "image/svg+xml",
    bmp: "image/bmp",
    avif: "image/avif",
  };
  return known[extension] ?? "application/octet-stream";
}

export function fileToBase64(file: File): Promise<string> {
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
