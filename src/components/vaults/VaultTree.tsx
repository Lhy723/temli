import {
  ChevronDown,
  ChevronRight,
  FileText,
  Folder,
  FolderOpen,
  Plus,
} from "lucide-react";
import { baseName, noteTitle, type TreeNode } from "./shared";

export function TreeRows({
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
