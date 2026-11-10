import { Link2, ListTree, SlidersHorizontal } from "lucide-react";
import type { Backlink } from "../../lib/api";
import { VaultOutlinePanel } from "../VaultOutlinePanel";
import { VaultPropertiesPanel } from "../VaultPropertiesPanel";
import { noteTitle, type AsideTab } from "./shared";

export function RightRail({
  asideTab,
  onAsideTabChange,
  backlinks,
  backlinksLoading,
  backlinksError,
  selectedNoteId,
  onOpenNote,
  draftBody,
  onJumpLine,
  propertiesSaving,
  onSaveProperties,
}: {
  asideTab: AsideTab;
  onAsideTabChange: (tab: AsideTab) => void;
  backlinks: Backlink[];
  backlinksLoading: boolean;
  backlinksError: string | null;
  selectedNoteId: string | null;
  onOpenNote: (id: string) => void;
  draftBody: string;
  onJumpLine: (line: number) => void;
  propertiesSaving: boolean;
  onSaveProperties: (props: Record<string, string>) => Promise<void>;
}) {
  return (
    <aside className="flex w-[240px] shrink-0 flex-col overflow-hidden rounded-md border border-[var(--border)]">
      <div className="flex h-9 shrink-0 items-stretch border-b border-[var(--border)]">
        <button
          type="button"
          aria-pressed={asideTab === "backlinks"}
          onClick={() => onAsideTabChange("backlinks")}
          className={`anim-fast flex flex-1 items-center justify-center gap-1 border-b-2 text-ui-sm ${
            asideTab === "backlinks"
              ? "border-[var(--accent)] text-[var(--foreground)]"
              : "border-transparent text-[var(--muted)] hover:bg-[var(--background-secondary)]"
          }`}
        >
          <Link2 size={12} />
          反链
          <span className="text-label">
            {backlinksLoading ? "…" : backlinks.length}
          </span>
        </button>
        <button
          type="button"
          aria-pressed={asideTab === "outline"}
          onClick={() => onAsideTabChange("outline")}
          className={`anim-fast flex flex-1 items-center justify-center gap-1 border-b-2 text-ui-sm ${
            asideTab === "outline"
              ? "border-[var(--accent)] text-[var(--foreground)]"
              : "border-transparent text-[var(--muted)] hover:bg-[var(--background-secondary)]"
          }`}
        >
          <ListTree size={12} />
          大纲
        </button>
        <button
          type="button"
          aria-pressed={asideTab === "properties"}
          onClick={() => onAsideTabChange("properties")}
          className={`anim-fast flex flex-1 items-center justify-center gap-1 border-b-2 text-ui-sm ${
            asideTab === "properties"
              ? "border-[var(--accent)] text-[var(--foreground)]"
              : "border-transparent text-[var(--muted)] hover:bg-[var(--background-secondary)]"
          }`}
        >
          <SlidersHorizontal size={12} />
          属性
        </button>
      </div>

      {asideTab === "backlinks" ? (
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
                onClick={() => onOpenNote(link.id)}
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
      ) : asideTab === "outline" ? (
        <div className="flex min-h-0 flex-1 flex-col overflow-hidden [&>section]:min-h-0 [&>section]:flex-1">
          <VaultOutlinePanel body={draftBody} onJump={onJumpLine} />
        </div>
      ) : (
        <div className="flex min-h-0 flex-1 flex-col overflow-hidden [&>section]:min-h-0 [&>section]:flex-1">
          <VaultPropertiesPanel
            body={draftBody}
            saving={propertiesSaving}
            onSave={onSaveProperties}
          />
        </div>
      )}
    </aside>
  );
}
