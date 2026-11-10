import { Button, Input } from "@heroui/react";
import {
  CalendarDays,
  ChevronDown,
  ChevronRight,
  Plus,
  Tag,
  X,
} from "lucide-react";
import type { VaultEntry, VaultHit, VaultTag } from "../../lib/api";
import { VaultTagsPanel } from "../VaultTagsPanel";
import { CreateNoteForm } from "./CreateNoteForm";
import { type TreeNode } from "./shared";
import { VaultTreePane } from "./VaultTreePane";

export function VaultSidebar({
  searchQuery,
  onSearchQueryChange,
  searching,
  searchLoading,
  searchError,
  searchHits,
  fileCount,
  treeLoading,
  creating,
  diaryBusy,
  onOpenDaily,
  onStartCreate,
  creatingPath,
  onCreatingPathChange,
  createError,
  onCreateSubmit,
  onCancelCreate,
  templates,
  templatePath,
  onTemplatePathChange,
  tagsOpen,
  onToggleTags,
  tags,
  tagsLoading,
  activeTag,
  onSelectTag,
  treeError,
  tagEntries,
  tagEntriesLoading,
  tagEntriesError,
  entries,
  tree,
  collapsed,
  selectedNoteId,
  onOpenNote,
  onToggleFolder,
}: {
  searchQuery: string;
  onSearchQueryChange: (query: string) => void;
  searching: boolean;
  searchLoading: boolean;
  searchError: string | null;
  searchHits: VaultHit[];
  fileCount: number;
  treeLoading: boolean;
  creating: boolean;
  diaryBusy: boolean;
  onOpenDaily: () => void;
  onStartCreate: (folder: string) => void;
  creatingPath: string | null;
  onCreatingPathChange: (path: string) => void;
  createError: string | null;
  onCreateSubmit: () => void;
  onCancelCreate: () => void;
  templates: VaultEntry[];
  templatePath: string;
  onTemplatePathChange: (path: string) => void;
  tagsOpen: boolean;
  onToggleTags: () => void;
  tags: VaultTag[];
  tagsLoading: boolean;
  activeTag: string | null;
  onSelectTag: (tag: string | null) => void;
  treeError: string | null;
  tagEntries: VaultEntry[];
  tagEntriesLoading: boolean;
  tagEntriesError: string | null;
  entries: VaultEntry[];
  tree: TreeNode[];
  collapsed: Set<string>;
  selectedNoteId: string | null;
  onOpenNote: (id: string) => void;
  onToggleFolder: (path: string) => void;
}) {
  return (
    <div className="flex w-[252px] min-w-0 shrink-0 flex-col gap-2">
      <div className="flex items-center gap-1.5">
        <Input
          aria-label="搜索笔记"
          placeholder="搜索笔记…"
          value={searchQuery}
          onChange={(e) => onSearchQueryChange(e.target.value)}
          className="min-w-0 flex-1"
        />
        {searching && (
          <Button
            isIconOnly
            variant="ghost"
            size="sm"
            className="h-7 w-7 shrink-0"
            aria-label="清空搜索"
            onPress={() => onSearchQueryChange("")}
          >
            <X size={14} />
          </Button>
        )}
      </div>

      <div className="flex flex-wrap items-center justify-between gap-1.5 px-1">
        <span className="shrink-0 text-ui-sm text-[var(--muted)]">
          {fileCount} 条笔记
        </span>
        <div className="flex items-center gap-1">
          <Button
            variant="ghost"
            size="sm"
            className="h-7 gap-1 px-2 text-ui"
            onPress={onOpenDaily}
            isDisabled={treeLoading || creating || diaryBusy}
            aria-label="打开今天的日记"
          >
            <CalendarDays size={14} />
            今天
          </Button>
          <Button
            variant="secondary"
            size="sm"
            className="h-7 gap-1 px-2 text-ui"
            onPress={() => onStartCreate("")}
            isDisabled={treeLoading || creating}
          >
            <Plus size={14} />
            新建笔记
          </Button>
        </div>
      </div>

      {creatingPath !== null && (
        <CreateNoteForm
          path={creatingPath}
          onPathChange={onCreatingPathChange}
          error={createError}
          creating={creating}
          onSubmit={onCreateSubmit}
          onCancel={onCancelCreate}
          templates={templates}
          templatePath={templatePath}
          onTemplatePathChange={onTemplatePathChange}
        />
      )}

      <div className="shrink-0 overflow-hidden rounded-md border border-[var(--border)]">
        <button
          type="button"
          onClick={onToggleTags}
          aria-expanded={tagsOpen}
          className="anim-fast flex h-8 w-full items-center gap-1.5 px-2 text-ui-sm text-[var(--muted)] hover:bg-[var(--background-secondary)]"
        >
          {tagsOpen ? (
            <ChevronDown size={12} className="shrink-0 opacity-60" />
          ) : (
            <ChevronRight size={12} className="shrink-0 opacity-60" />
          )}
          <Tag size={13} className="shrink-0" />
          <span>标签</span>
          <span className="ml-auto text-label">
            {tagsLoading ? "…" : tags.length}
          </span>
        </button>
        {tagsOpen && (
          <div className="flex h-[176px] min-h-0 flex-col overflow-hidden border-t border-[var(--border)] [&>section]:min-h-0 [&>section]:flex-1">
            <VaultTagsPanel
              tags={tags}
              activeTag={activeTag}
              loading={tagsLoading}
              onSelect={onSelectTag}
            />
          </div>
        )}
      </div>

      {treeError && (
        <p role="alert" className="px-1 text-ui-sm text-[var(--danger)]">
          {treeError}
        </p>
      )}

      <VaultTreePane
        searching={searching}
        searchLoading={searchLoading}
        searchError={searchError}
        searchHits={searchHits}
        selectedNoteId={selectedNoteId}
        onOpenNote={onOpenNote}
        activeTag={activeTag}
        tagEntries={tagEntries}
        tagEntriesLoading={tagEntriesLoading}
        tagEntriesError={tagEntriesError}
        onClearTag={() => onSelectTag(null)}
        treeLoading={treeLoading}
        entries={entries}
        tree={tree}
        collapsed={collapsed}
        onToggleFolder={onToggleFolder}
        onStartCreate={onStartCreate}
      />
    </div>
  );
}
