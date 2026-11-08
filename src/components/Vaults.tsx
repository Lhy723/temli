import { useEffect, useMemo, useRef, useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Button, Card, Input, ListBox, Select, TextArea } from "@heroui/react";
import {
  ChevronDown,
  ChevronRight,
  FileText,
  Folder,
  FolderOpen,
  Link2,
  Network,
  NotebookPen,
  Plus,
  RefreshCw,
  Save,
  Trash2,
  X,
} from "lucide-react";
import {
  addVault,
  createVaultNote,
  deleteVaultNote,
  errorText,
  formatDate,
  getVaultNote,
  indexVault,
  listVaults,
  noteBacklinks,
  removeVault,
  saveVaultNote,
  searchVault,
  vaultGraph,
  vaultTree,
  type Backlink,
  type Vault,
  type VaultEntry,
  type VaultGraph,
  type VaultHit,
  type VaultNote,
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

type TreeNode = {
  entry: VaultEntry;
  children: TreeNode[];
};

function baseName(path: string): string {
  const index = path.lastIndexOf("/");
  return index >= 0 ? path.slice(index + 1) : path;
}

function noteTitle(path: string): string {
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
function buildTree(entries: VaultEntry[]): TreeNode[] {
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

function uniqueNotePath(entries: VaultEntry[], folder: string): string {
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

function normalizeNotePath(raw: string): string | null {
  const cleaned = raw.trim().replace(/^\/+/, "").replace(/\/+/g, "/");
  if (!cleaned) return null;
  const segments = cleaned.split("/");
  if (segments.some((segment) => !segment || segment === "." || segment === "..")) {
    return null;
  }
  return cleaned.toLowerCase().endsWith(".md") ? cleaned : `${cleaned}.md`;
}

function TreeRows({
  nodes,
  depth,
  collapsed,
  selectedId,
  onToggle,
  onOpen,
  onCreate,
}: {
  nodes: TreeNode[];
  depth: number;
  collapsed: Set<string>;
  selectedId: string | null;
  onToggle: (path: string) => void;
  onOpen: (id: string) => void;
  onCreate: (folder: string) => void;
}) {
  return (
    <>
      {nodes.map((node) => {
        const { entry, children } = node;
        if (entry.is_dir) {
          const isCollapsed = collapsed.has(entry.path);
          return (
            <div key={entry.path}>
              <div
                className="group flex h-7 items-center pr-1"
                style={{ paddingLeft: 4 + depth * 12 }}
              >
                <button
                  type="button"
                  onClick={() => onToggle(entry.path)}
                  className="anim-fast flex h-7 min-w-0 flex-1 items-center gap-1 rounded px-1 text-left text-ui hover:bg-[var(--background-secondary)]"
                >
                  {isCollapsed ? (
                    <ChevronRight size={12} className="shrink-0 opacity-60" />
                  ) : (
                    <ChevronDown size={12} className="shrink-0 opacity-60" />
                  )}
                  {isCollapsed ? (
                    <Folder size={13} className="shrink-0 text-[var(--muted)]" />
                  ) : (
                    <FolderOpen
                      size={13}
                      className="shrink-0 text-[var(--muted)]"
                    />
                  )}
                  <span className="truncate">
                    {entry.title || baseName(entry.path)}
                  </span>
                </button>
                <button
                  type="button"
                  aria-label={`在 ${baseName(entry.path)} 中新建笔记`}
                  title="在此文件夹新建笔记"
                  onClick={() => onCreate(entry.path)}
                  className="anim-fast flex size-5 shrink-0 items-center justify-center rounded text-[var(--muted)] opacity-0 hover:bg-[var(--background-tertiary)] group-hover:opacity-80"
                >
                  <Plus size={12} />
                </button>
              </div>
              {!isCollapsed && children.length > 0 && (
                <TreeRows
                  nodes={children}
                  depth={depth + 1}
                  collapsed={collapsed}
                  selectedId={selectedId}
                  onToggle={onToggle}
                  onOpen={onOpen}
                  onCreate={onCreate}
                />
              )}
            </div>
          );
        }

        const isSelected = selectedId === entry.id;
        return (
          <button
            key={entry.path}
            type="button"
            onClick={() => entry.id && onOpen(entry.id)}
            disabled={!entry.id}
            className={`anim-fast flex h-7 w-full items-center gap-1.5 pr-2 text-left text-ui ${
              isSelected
                ? "bg-[var(--background-secondary)] font-medium"
                : "hover:bg-[var(--background-secondary)]"
            }`}
            style={{ paddingLeft: 8 + depth * 12 + 12 }}
            title={entry.path}
          >
            <FileText size={13} className="shrink-0 text-[var(--muted)]" />
            <span className="truncate">{entry.title || noteTitle(entry.path)}</span>
          </button>
        );
      })}
    </>
  );
}

function VaultGraphView({
  graph,
  onOpen,
}: {
  graph: VaultGraph;
  onOpen: (id: string) => void;
}) {
  const [hovered, setHovered] = useState<string | null>(null);
  const width = 760;
  const height = 560;
  const centerX = width / 2;
  const centerY = height / 2;
  const radius = Math.min(width, height) * 0.36;
  const count = graph.nodes.length;

  const positions = new Map<string, { x: number; y: number }>();
  graph.nodes.forEach((node, index) => {
    if (count === 1) {
      positions.set(node.id, { x: centerX, y: centerY });
      return;
    }
    const angle = (Math.PI * 2 * index) / count - Math.PI / 2;
    positions.set(node.id, {
      x: centerX + Math.cos(angle) * radius,
      y: centerY + Math.sin(angle) * radius,
    });
  });

  const activeNode =
    hovered && graph.nodes.some((node) => node.id === hovered) ? hovered : null;

  const highlighted = new Set<string>();
  if (activeNode) {
    highlighted.add(activeNode);
    for (const edge of graph.edges) {
      if (edge.from === activeNode) highlighted.add(edge.to);
      if (edge.to === activeNode) highlighted.add(edge.from);
    }
  }

  return (
    <svg
      viewBox={`0 0 ${width} ${height}`}
      className="h-full w-full"
      onMouseLeave={() => setHovered(null)}
    >
      <g>
        {graph.edges.map((edge, index) => {
          const from = positions.get(edge.from);
          const to = positions.get(edge.to);
          if (!from || !to) return null;
          const active =
            activeNode !== null &&
            (edge.from === activeNode || edge.to === activeNode);
          return (
            <line
              key={`${edge.from}->${edge.to}:${index}`}
              x1={from.x}
              y1={from.y}
              x2={to.x}
              y2={to.y}
              stroke="var(--border)"
              strokeWidth={active ? 1.4 : 0.8}
              opacity={activeNode ? (active ? 0.9 : 0.2) : 0.55}
            />
          );
        })}
        {graph.nodes.map((node) => {
          const position = positions.get(node.id);
          if (!position) return null;
          const isHovered = activeNode === node.id;
          const dimmed = activeNode !== null && !highlighted.has(node.id);
          const label =
            activeNode === null || count <= 60 || highlighted.has(node.id);
          const name = node.title || noteTitle(node.path);
          const shortName = name.length > 18 ? `${name.slice(0, 18)}…` : name;
          return (
            <g
              key={node.id}
              transform={`translate(${position.x} ${position.y})`}
              opacity={dimmed ? 0.3 : 1}
              style={{ cursor: "pointer" }}
              onMouseEnter={() => setHovered(node.id)}
              onClick={() => onOpen(node.id)}
            >
              <title>{node.path}</title>
              <circle r={isHovered ? 10 : 7} fill="var(--accent)" fillOpacity={0.15} />
              <circle r={isHovered ? 6 : 4.5} fill="var(--accent)" />
              {label && (
                <text
                  y={isHovered ? -14 : 15}
                  textAnchor="middle"
                  style={{ fill: "var(--muted)", fontSize: 10 }}
                  className="select-none"
                >
                  {shortName}
                </text>
              )}
            </g>
          );
        })}
      </g>
    </svg>
  );
}

export function Vaults({
  focusNoteId,
  focusVaultId,
  onFocusConsumed,
}: {
  focusNoteId?: string;
  focusVaultId?: string;
  onFocusConsumed?: () => void;
}) {
  const [vaults, setVaults] = useState<Vault[]>([]);
  const [vaultsLoading, setVaultsLoading] = useState(true);
  const [vaultError, setVaultError] = useState<string | null>(null);
  const [selectedVaultId, setSelectedVaultId] = useState("");

  const [addingVault, setAddingVault] = useState(false);
  const [vaultName, setVaultName] = useState("");
  const [vaultRoot, setVaultRoot] = useState("");
  const [addingVaultBusy, setAddingVaultBusy] = useState(false);
  const [addVaultError, setAddVaultError] = useState<string | null>(null);
  const [confirmingRemoveVault, setConfirmingRemoveVault] = useState(false);
  const [removingVault, setRemovingVault] = useState(false);

  const [entries, setEntries] = useState<VaultEntry[]>([]);
  const [treeLoading, setTreeLoading] = useState(false);
  const [treeError, setTreeError] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const treeRequest = useRef(0);
  const vaultRef = useRef(selectedVaultId);
  vaultRef.current = selectedVaultId;

  const [creatingPath, setCreatingPath] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  const [searchQuery, setSearchQuery] = useState("");
  const [searchHits, setSearchHits] = useState<VaultHit[]>([]);
  const [searchLoading, setSearchLoading] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);

  const [mode, setMode] = useState<"editor" | "graph">("editor");
  const [graphData, setGraphData] = useState<VaultGraph | null>(null);
  const [graphLoading, setGraphLoading] = useState(false);
  const [graphError, setGraphError] = useState<string | null>(null);
  const [graphSeed, setGraphSeed] = useState(0);

  const [selectedNoteId, setSelectedNoteId] = useState<string | null>(null);
  const [note, setNote] = useState<VaultNote | null>(null);
  const [noteLoading, setNoteLoading] = useState(false);
  const [noteError, setNoteError] = useState<string | null>(null);
  const [draftTitle, setDraftTitle] = useState("");
  const [draftBody, setDraftBody] = useState("");
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const openRequest = useRef(0);
  const focusConsumedRef = useRef<string | null>(null);
  const treeLoadedVaultRef = useRef<string | null>(null);
  const treeErrorVaultRef = useRef<string | null>(null);

  const [backlinks, setBacklinks] = useState<Backlink[]>([]);
  const [backlinksLoading, setBacklinksLoading] = useState(false);
  const [backlinksError, setBacklinksError] = useState<string | null>(null);

  const currentVault =
    vaults.find((vault) => vault.id === selectedVaultId) ?? null;
  const tree = useMemo(() => buildTree(entries), [entries]);
  const fileCount = entries.filter((entry) => !entry.is_dir).length;
  const searching = searchQuery.trim().length > 0;

  useEffect(() => {
    let cancelled = false;
    setVaultsLoading(true);
    listVaults()
      .then((loaded) => {
        if (cancelled) return;
        setVaults(loaded);
        setSelectedVaultId((previous) => previous || loaded[0]?.id || "");
      })
      .catch((error) => {
        if (!cancelled) setVaultError(errorText(error));
      })
      .finally(() => {
        if (!cancelled) setVaultsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function refreshTree(vaultId: string, reindex: boolean) {
    const request = ++treeRequest.current;
    if (vaultId === vaultRef.current) treeErrorVaultRef.current = null;
    setTreeLoading(true);
    setTreeError(null);
    try {
      if (reindex) {
        await indexVault(vaultId);
        if (vaultId === vaultRef.current) setGraphSeed((seed) => seed + 1);
      }
      const loaded = await vaultTree(vaultId);
      if (request !== treeRequest.current || vaultId !== vaultRef.current) return;
      setEntries(loaded);
      treeLoadedVaultRef.current = vaultId;
      if (reindex) {
        setNotice(
          `已索引 ${loaded.filter((entry) => !entry.is_dir).length} 条笔记`,
        );
      }
    } catch (error) {
      if (request === treeRequest.current) {
        setTreeError(errorText(error));
        treeErrorVaultRef.current = vaultId;
      }
    } finally {
      if (request === treeRequest.current) setTreeLoading(false);
    }
  }

  useEffect(() => {
    if (!selectedVaultId) {
      treeRequest.current += 1;
      openRequest.current += 1;
      treeLoadedVaultRef.current = null;
      treeErrorVaultRef.current = null;
      setEntries([]);
      setTreeError(null);
      setTreeLoading(false);
      setNote(null);
      setSelectedNoteId(null);
      setNoteLoading(false);
      setNoteError(null);
      setBacklinks([]);
      setBacklinksError(null);
      setBacklinksLoading(false);
      setGraphData(null);
      return;
    }

    openRequest.current += 1;
    treeLoadedVaultRef.current = null;
    treeErrorVaultRef.current = null;
    setEntries([]);
    setCollapsed(new Set());
    setCreatingPath(null);
    setCreateError(null);
    setSearchQuery("");
    setSearchHits([]);
    setSearchError(null);
    setMode("editor");
    setGraphData(null);
    setGraphError(null);
    setNote(null);
    setSelectedNoteId(null);
    setNoteLoading(false);
    setNoteError(null);
    setDraftTitle("");
    setDraftBody("");
    setBacklinks([]);
    setBacklinksError(null);
    setBacklinksLoading(false);
    setConfirmingDelete(false);
    setConfirmingRemoveVault(false);
    setActionError(null);
    setNotice(null);

    void refreshTree(selectedVaultId, true);
  }, [selectedVaultId]);

  useEffect(() => {
    const query = searchQuery.trim();
    if (!selectedVaultId || !query) {
      setSearchHits([]);
      setSearchLoading(false);
      setSearchError(null);
      return;
    }
    let cancelled = false;
    setSearchLoading(true);
    setSearchError(null);
    const timer = window.setTimeout(() => {
      searchVault(selectedVaultId, query)
        .then((hits) => {
          if (!cancelled) setSearchHits(hits);
        })
        .catch((error) => {
          if (!cancelled) {
            setSearchHits([]);
            setSearchError(errorText(error));
          }
        })
        .finally(() => {
          if (!cancelled) setSearchLoading(false);
        });
    }, 220);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [selectedVaultId, searchQuery]);

  useEffect(() => {
    if (mode !== "graph" || !selectedVaultId) return;
    let cancelled = false;
    setGraphLoading(true);
    setGraphError(null);
    vaultGraph(selectedVaultId)
      .then((loaded) => {
        if (!cancelled) setGraphData(loaded);
      })
      .catch((error) => {
        if (!cancelled) setGraphError(errorText(error));
      })
      .finally(() => {
        if (!cancelled) setGraphLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [mode, selectedVaultId, graphSeed]);

  const focusKey =
    focusNoteId || focusVaultId
      ? `${focusVaultId ?? ""}\u0000${focusNoteId ?? ""}`
      : null;

  // 搜索面板深链：先切到目标笔记库，等树加载完成后再打开笔记并消费一次。
  useEffect(() => {
    if (!focusKey) {
      focusConsumedRef.current = null;
      return;
    }
    if (focusConsumedRef.current === focusKey) return;

    if (focusVaultId) {
      if (vaultsLoading) return;
      if (!vaults.some((vault) => vault.id === focusVaultId)) {
        focusConsumedRef.current = focusKey;
        setNotice("未找到对应的笔记库，无法打开笔记。");
        onFocusConsumed?.();
        return;
      }
      if (selectedVaultId !== focusVaultId) {
        setSelectedVaultId(focusVaultId);
        return;
      }
    } else if (!selectedVaultId) {
      if (vaultsLoading) return;
      const fallbackVaultId = vaults[0]?.id;
      if (fallbackVaultId) {
        setSelectedVaultId(fallbackVaultId);
        return;
      }
      focusConsumedRef.current = focusKey;
      setNotice("还没有可用的笔记库。");
      onFocusConsumed?.();
      return;
    }

    if (!focusNoteId) {
      focusConsumedRef.current = focusKey;
      onFocusConsumed?.();
      return;
    }

    if (treeLoadedVaultRef.current !== selectedVaultId) {
      // 树加载失败时仍尝试直接打开，笔记可能只是没进索引。
      if (treeErrorVaultRef.current === selectedVaultId && !treeLoading) {
        focusConsumedRef.current = focusKey;
        void openNote(focusNoteId);
        onFocusConsumed?.();
      }
      return;
    }

    if (!entries.some((entry) => entry.id === focusNoteId)) {
      focusConsumedRef.current = focusKey;
      setNotice("未找到要打开的笔记，可能已被移动或删除。");
      onFocusConsumed?.();
      return;
    }

    focusConsumedRef.current = focusKey;
    void openNote(focusNoteId);
    onFocusConsumed?.();
  }, [
    focusKey,
    focusNoteId,
    focusVaultId,
    vaults,
    vaultsLoading,
    selectedVaultId,
    entries,
    treeLoading,
    treeError,
    onFocusConsumed,
  ]);

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

  function startCreate(folder: string) {
    setCreatingPath(uniqueNotePath(entries, folder));
    setCreateError(null);
    setSearchQuery("");
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
      const created = await createVaultNote({
        vault_id: vaultId,
        path,
        title: noteTitle(path),
        body: "",
      });
      if (vaultId !== vaultRef.current) return;
      setCreatingPath(null);
      setMode("editor");
      await refreshTree(vaultId, false);
      if (vaultId !== vaultRef.current) return;
      await openNote(created.id);
      setNotice(`已创建 ${created.path}`);
    } catch (error) {
      setCreateError(errorText(error));
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
      if (request !== openRequest.current) return;
      setNote(saved);
      setDraftTitle(saved.title);
      setDraftBody(saved.body);
      setNotice(`已保存 ${saved.path}`);
      setGraphSeed((seed) => seed + 1);
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
      if (vaultId === vaultRef.current) {
        setGraphSeed((seed) => seed + 1);
      }
      if (request === openRequest.current) {
        openRequest.current += 1;
        setNote(null);
        setSelectedNoteId(null);
        setDraftTitle("");
        setDraftBody("");
        setBacklinks([]);
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

  async function submitVault() {
    if (addingVaultBusy) return;
    const rootPath = vaultRoot.trim();
    if (!rootPath) {
      setAddVaultError("请输入笔记库目录。");
      return;
    }
    setAddingVaultBusy(true);
    setAddVaultError(null);
    setVaultError(null);
    try {
      const created = await addVault({
        name:
          vaultName.trim() ||
          noteTitle(rootPath.replace(/[\\/]+$/, "")) ||
          "笔记库",
        root_path: rootPath,
        create: true,
      });
      setVaults((previous) => [...previous, created]);
      setSelectedVaultId(created.id);
      setAddingVault(false);
      setVaultName("");
      setVaultRoot("");
    } catch (error) {
      setAddVaultError(errorText(error));
    } finally {
      setAddingVaultBusy(false);
    }
  }

  async function removeCurrentVault() {
    if (!selectedVaultId || removingVault) return;
    if (!confirmingRemoveVault) {
      setConfirmingRemoveVault(true);
      return;
    }
    setRemovingVault(true);
    setVaultError(null);
    try {
      await removeVault(selectedVaultId);
      const remaining = vaults.filter((vault) => vault.id !== selectedVaultId);
      setVaults(remaining);
      setSelectedVaultId(remaining[0]?.id ?? "");
      setConfirmingRemoveVault(false);
      setNotice("已移除笔记库");
    } catch (error) {
      setVaultError(errorText(error));
    } finally {
      setRemovingVault(false);
    }
  }

  function toggleFolder(path: string) {
    setCollapsed((previous) => {
      const next = new Set(previous);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  return (
    <div className="flex h-full min-h-0 flex-col gap-2">
      <div className="flex shrink-0 flex-wrap items-center gap-1.5">
        <Select
          aria-label="选择笔记库"
          className="w-[190px]"
          placeholder={vaultsLoading ? "加载中…" : "选择笔记库"}
          selectedKey={selectedVaultId || null}
          onSelectionChange={(key) =>
            setSelectedVaultId(key == null ? "" : String(key))
          }
          isDisabled={vaultsLoading || vaults.length === 0}
        >
          <Select.Trigger className="h-7 text-ui">
            <Select.Value />
            <Select.Indicator />
          </Select.Trigger>
          <Select.Popover>
            <ListBox>
              {vaults.map((vault) => (
                <ListBox.Item key={vault.id} id={vault.id} textValue={vault.name}>
                  {vault.name}
                </ListBox.Item>
              ))}
            </ListBox>
          </Select.Popover>
        </Select>

        <Button
          variant="secondary"
          size="sm"
          className="h-7 gap-1 px-2.5 text-ui"
          onPress={() => {
            setAddingVault((open) => !open);
            setAddVaultError(null);
          }}
        >
          <Plus size={14} />
          添加笔记库
        </Button>

        {currentVault && (
          <span
            className="min-w-0 max-w-[280px] truncate font-mono text-label text-[var(--muted)]"
            title={currentVault.root_path}
          >
            {currentVault.root_path}
          </span>
        )}

        {selectedVaultId && (
          <div className="ml-auto flex items-center gap-1">
            <Button
              variant="secondary"
              size="sm"
              className="h-7 gap-1 px-2.5 text-ui"
              onPress={() => void refreshTree(selectedVaultId, true)}
              isDisabled={treeLoading}
            >
              <RefreshCw
                size={14}
                className={treeLoading ? "animate-spin" : undefined}
              />
              {treeLoading ? "索引中…" : "索引"}
            </Button>
            <Button
              variant={mode === "editor" ? "tertiary" : "ghost"}
              size="sm"
              className="h-7 gap-1 px-2.5 text-ui"
              onPress={() => setMode("editor")}
            >
              <NotebookPen size={14} />
              编辑
            </Button>
            <Button
              variant={mode === "graph" ? "tertiary" : "ghost"}
              size="sm"
              className="h-7 gap-1 px-2.5 text-ui"
              onPress={() => setMode("graph")}
            >
              <Network size={14} />
              图谱
            </Button>
            <Button
              variant="danger-soft"
              size="sm"
              className="h-7 gap-1 px-2.5 text-ui"
              onPress={() => void removeCurrentVault()}
              isDisabled={removingVault}
              aria-label="从应用移除笔记库，不删除磁盘文件"
            >
              <Trash2 size={14} />
              {removingVault
                ? "移除中…"
                : confirmingRemoveVault
                  ? "确认移除"
                  : "移除"}
            </Button>
          </div>
        )}
      </div>

      {vaultError && (
        <p role="alert" className="shrink-0 text-ui-sm text-[var(--danger)]">
          {vaultError}
        </p>
      )}
      {notice && (
        <p className="shrink-0 text-ui-sm text-[var(--muted)]">{notice}</p>
      )}

      {addingVault && (
        <Card className="max-w-[680px] shrink-0">
          <Card.Header>
            <Card.Title className="text-ui font-medium">添加笔记库</Card.Title>
            <Card.Description className="text-ui-sm">
              笔记以目录下的 Markdown 文件为准，与应用内的项目互相独立；目录不存在时会自动创建。
            </Card.Description>
          </Card.Header>
          <Card.Content className="grid gap-2">
            <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
              名称
              <Input
                aria-label="笔记库名称"
                placeholder="个人知识库"
                value={vaultName}
                onChange={(e) => setVaultName(e.target.value)}
                disabled={addingVaultBusy}
              />
            </label>
            <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
              目录
              <Input
                aria-label="笔记库目录"
                placeholder="/Users/lhy/Notes"
                value={vaultRoot}
                onChange={(e) => setVaultRoot(e.target.value)}
                disabled={addingVaultBusy}
                onKeyDown={(e) => {
                  if (e.key === "Enter") void submitVault();
                }}
              />
            </label>
            {addVaultError && (
              <p role="alert" className="text-ui-sm text-[var(--danger)]">
                {addVaultError}
              </p>
            )}
          </Card.Content>
          <Card.Footer className="flex justify-end gap-1.5">
            <Button
              variant="ghost"
              size="sm"
              className="h-7 px-2.5 text-ui"
              onPress={() => setAddingVault(false)}
              isDisabled={addingVaultBusy}
            >
              取消
            </Button>
            <Button
              variant="primary"
              size="sm"
              className="h-7 px-2.5 text-ui"
              onPress={() => void submitVault()}
              isDisabled={addingVaultBusy || !vaultRoot.trim()}
            >
              {addingVaultBusy ? "添加中…" : "添加"}
            </Button>
          </Card.Footer>
        </Card>
      )}

      {vaultsLoading ? (
        <p className="text-ui-sm text-[var(--muted)]">加载笔记库…</p>
      ) : vaults.length === 0 && !addingVault ? (
        <div className="flex flex-1 flex-col items-center justify-center gap-2.5 text-center">
          <Network size={20} className="text-[var(--muted)] opacity-70" />
          <p className="text-ui text-[var(--muted)]">
            还没有笔记库。添加一个本地目录，把 Markdown 笔记变成独立的知识空间。
          </p>
          <Button
            variant="primary"
            size="sm"
            className="h-7 gap-1 px-3 text-ui"
            onPress={() => setAddingVault(true)}
          >
            <Plus size={14} />
            添加笔记库
          </Button>
        </div>
      ) : selectedVaultId ? (
        <div className="flex min-h-0 flex-1 gap-3">
          <div className="flex w-[252px] min-w-0 shrink-0 flex-col gap-2">
            <div className="flex items-center gap-1.5">
              <Input
                aria-label="搜索笔记"
                placeholder="搜索笔记…"
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

            <div className="flex items-center justify-between gap-2 px-1">
              <span className="text-ui-sm text-[var(--muted)]">
                {fileCount} 条笔记
              </span>
              <Button
                variant="secondary"
                size="sm"
                className="h-7 gap-1 px-2 text-ui"
                onPress={() => startCreate("")}
                isDisabled={treeLoading || creating}
              >
                <Plus size={14} />
                新建笔记
              </Button>
            </div>

            {creatingPath !== null && (
              <div className="grid gap-1">
                <div className="flex items-center gap-1">
                  <Input
                    aria-label="新笔记路径"
                    placeholder="文件夹/笔记.md"
                    value={creatingPath}
                    onChange={(e) => setCreatingPath(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") void submitCreate();
                    }}
                    className="min-w-0 flex-1"
                    disabled={creating}
                    autoFocus
                  />
                  <Button
                    variant="primary"
                    size="sm"
                    className="h-7 shrink-0 px-2 text-ui"
                    onPress={() => void submitCreate()}
                    isDisabled={creating}
                  >
                    {creating ? "创建中…" : "创建"}
                  </Button>
                  <Button
                    isIconOnly
                    variant="ghost"
                    size="sm"
                    className="h-7 w-7 shrink-0"
                    aria-label="取消新建"
                    onPress={() => {
                      setCreatingPath(null);
                      setCreateError(null);
                    }}
                    isDisabled={creating}
                  >
                    <X size={14} />
                  </Button>
                </div>
                {createError && (
                  <p role="alert" className="text-ui-sm text-[var(--danger)]">
                    {createError}
                  </p>
                )}
              </div>
            )}

            {treeError && (
              <p role="alert" className="px-1 text-ui-sm text-[var(--danger)]">
                {treeError}
              </p>
            )}

            <div className="min-h-0 flex-1 overflow-auto rounded-md border border-[var(--border)]">
              {searching ? (
                searchLoading ? (
                  <p className="px-3 py-2 text-ui-sm text-[var(--muted)]">
                    搜索中…
                  </p>
                ) : searchError ? (
                  <p
                    role="alert"
                    className="px-3 py-2 text-ui-sm text-[var(--danger)]"
                  >
                    {searchError}
                  </p>
                ) : searchHits.length === 0 ? (
                  <p className="px-3 py-2 text-ui-sm text-[var(--muted)]">
                    没有匹配的笔记。
                  </p>
                ) : (
                  searchHits.map((hit) => (
                    <button
                      key={hit.id}
                      type="button"
                      onClick={() => void openNote(hit.id)}
                      className={`anim-fast block w-full border-b border-[var(--border)] px-3 py-2 text-left last:border-b-0 hover:bg-[var(--background-secondary)] ${
                        selectedNoteId === hit.id
                          ? "bg-[var(--background-secondary)]"
                          : ""
                      }`}
                    >
                      <span className="block truncate text-ui">
                        {hit.title || noteTitle(hit.path)}
                      </span>
                      <span className="block truncate font-mono text-label text-[var(--muted)]">
                        {hit.path}
                      </span>
                      {hit.snippet && (
                        <span className="mt-0.5 line-clamp-2 text-ui-sm text-[var(--muted)]">
                          {hit.snippet}
                        </span>
                      )}
                    </button>
                  ))
                )
              ) : treeLoading && entries.length === 0 ? (
                <p className="px-3 py-2 text-ui-sm text-[var(--muted)]">
                  索引中…
                </p>
              ) : entries.length === 0 ? (
                <p className="px-3 py-2 text-ui-sm text-[var(--muted)]">
                  目录里还没有 Markdown 笔记。
                </p>
              ) : (
                <TreeRows
                  nodes={tree}
                  depth={0}
                  collapsed={collapsed}
                  selectedId={selectedNoteId}
                  onToggle={toggleFolder}
                  onOpen={(id) => void openNote(id)}
                  onCreate={startCreate}
                />
              )}
            </div>
          </div>

          <section className="flex min-w-0 flex-1 flex-col overflow-hidden rounded-md border border-[var(--border)]">
            {mode === "graph" ? (
              <>
                <header className="flex min-h-12 shrink-0 flex-wrap items-center gap-2 border-b border-[var(--border)] px-3 py-1.5">
                  <div className="min-w-0 flex-1">
                    <p className="text-ui font-medium">知识图谱</p>
                    <p className="text-label text-[var(--muted)]">
                      {graphData
                        ? `${graphData.nodes.length} 个节点 · ${graphData.edges.length} 条链接`
                        : "按 [[双链]] 构建"}
                    </p>
                  </div>
                </header>
                {graphError ? (
                  <p role="alert" className="p-3 text-ui-sm text-[var(--danger)]">
                    {graphError}
                  </p>
                ) : graphLoading && !graphData ? (
                  <p className="p-3 text-ui-sm text-[var(--muted)]">加载中…</p>
                ) : graphData && graphData.nodes.length > 0 ? (
                  <div className="min-h-0 flex-1 p-3">
                    <VaultGraphView
                      graph={graphData}
                      onOpen={(id) => {
                        setMode("editor");
                        void openNote(id);
                      }}
                    />
                  </div>
                ) : (
                  <div className="flex flex-1 flex-col items-center justify-center gap-2 text-center">
                    <Network
                      size={20}
                      className="text-[var(--muted)] opacity-70"
                    />
                    <p className="text-ui text-[var(--muted)]">
                      笔记里用 [[标题]] 互链后会在这里生成图谱。
                    </p>
                  </div>
                )}
              </>
            ) : (
              <>
                <header className="flex min-h-12 shrink-0 flex-wrap items-center gap-2 border-b border-[var(--border)] px-3 py-1.5">
                  <div className="min-w-0 flex-1">
                    {note && !noteLoading ? (
                      <>
                        <Input
                          aria-label="笔记标题"
                          placeholder="笔记标题"
                          value={draftTitle}
                          onChange={(e) => setDraftTitle(e.target.value)}
                          disabled={saving || deleting}
                          className="text-ui"
                        />
                        <p
                          className="mt-0.5 truncate font-mono text-label text-[var(--muted)]"
                          title={note.path}
                        >
                          {note.path} · 更新于 {formatDate(note.updated_at)}
                        </p>
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
                        variant="secondary"
                        size="sm"
                        className="h-7 shrink-0 gap-1 px-2.5 text-ui"
                        onPress={() => void saveNote()}
                        isDisabled={saving || deleting}
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
                    <TextArea
                      aria-label="笔记正文"
                      placeholder="用 Markdown 记录想法，[[另一条笔记]] 建立双链…"
                      value={draftBody}
                      onChange={(e) => setDraftBody(e.target.value)}
                      disabled={saving || deleting}
                      className="min-h-0 h-full w-full resize-none font-mono text-ui-sm"
                    />
                    <div className="min-h-0 overflow-auto rounded-md border border-[var(--border)] bg-[var(--background-secondary)] p-3">
                      {draftBody.trim() ? (
                        <div className={MARKDOWN_CLASS}>
                          <Markdown remarkPlugins={[remarkGfm]}>
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
            )}
          </section>

          {mode === "editor" && note && !noteLoading && (
            <aside className="flex w-[200px] shrink-0 flex-col overflow-hidden rounded-md border border-[var(--border)]">
              <header className="flex h-9 shrink-0 items-center gap-1.5 border-b border-[var(--border)] px-2.5 text-ui-sm text-[var(--muted)]">
                <Link2 size={13} />
                反向链接
                <span className="ml-auto text-label">
                  {backlinksLoading ? "…" : backlinks.length}
                </span>
              </header>
              <div className="min-h-0 flex-1 overflow-auto">
                {backlinksLoading ? (
                  <p className="px-2.5 py-2 text-ui-sm text-[var(--muted)]">
                    加载中…
                  </p>
                ) : backlinksError ? (
                  <p
                    role="alert"
                    className="px-2.5 py-2 text-ui-sm text-[var(--danger)]"
                  >
                    {backlinksError}
                  </p>
                ) : backlinks.length === 0 ? (
                  <p className="px-2.5 py-2 text-ui-sm text-[var(--muted)]">
                    还没有其他笔记链接到这里。
                  </p>
                ) : (
                  backlinks.map((link) => (
                    <button
                      key={`${link.id}:${link.target}`}
                      type="button"
                      onClick={() => void openNote(link.id)}
                      className={`anim-fast block w-full border-b border-[var(--border)] px-2.5 py-2 text-left last:border-b-0 hover:bg-[var(--background-secondary)] ${
                        selectedNoteId === link.id
                          ? "bg-[var(--background-secondary)]"
                          : ""
                      }`}
                    >
                      <span className="block truncate text-ui">
                        {link.title || noteTitle(link.path)}
                      </span>
                      <span className="block truncate font-mono text-label text-[var(--muted)]">
                        {link.path}
                      </span>
                    </button>
                  ))
                )}
              </div>
            </aside>
          )}
        </div>
      ) : null}
    </div>
  );
}
