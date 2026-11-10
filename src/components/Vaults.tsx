import { useEffect, useRef, useState } from "react";
import { Button, ListBox, Select } from "@heroui/react";
import {
  Network,
  NotebookPen,
  Plus,
  RefreshCw,
  Trash2,
} from "lucide-react";
import {
  addVault,
  errorText,
  listVaults,
  removeVault,
  vaultGraph,
  type Vault,
  type VaultGraph,
} from "../lib/api";
import { VaultQuickSwitcher } from "./VaultQuickSwitcher";
import { AddVaultCard } from "./vaults/AddVaultCard";
import { NoteEditor } from "./vaults/NoteEditor";
import { RightRail } from "./vaults/RightRail";
import { VaultGraphPane } from "./vaults/VaultGraphPane";
import { VaultSidebar } from "./vaults/VaultSidebar";
import { noteTitle, type VaultMode } from "./vaults/shared";
import { useVaultAttachments } from "./vaults/useVaultAttachments";
import { useVaultNotes } from "./vaults/useVaultNotes";
import { useVaultTree } from "./vaults/useVaultTree";

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

  const [mode, setMode] = useState<VaultMode>("editor");
  const [graphData, setGraphData] = useState<VaultGraph | null>(null);
  const [graphLoading, setGraphLoading] = useState(false);
  const [graphError, setGraphError] = useState<string | null>(null);
  const [graphSeed, setGraphSeed] = useState(0);

  const [actionError, setActionError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const focusConsumedRef = useRef<string | null>(null);

  const tree = useVaultTree({
    selectedVaultId,
    setNotice,
    setActionError,
    bumpGraphSeed: () => setGraphSeed((seed) => seed + 1),
  });

  const notes = useVaultNotes({
    selectedVaultId,
    entries: tree.entries,
    refreshTree: tree.refreshTree,
    refreshTags: tree.refreshTags,
    setNotice,
    setActionError,
    setMode,
    bumpGraphSeed: () => setGraphSeed((seed) => seed + 1),
    setSearchQuery: tree.setSearchQuery,
  });

  const attachments = useVaultAttachments({
    vaultId: selectedVaultId,
    note: notes.note,
    openRequest: notes.openRequest,
    bodyRef: notes.bodyRef,
    insertAtCursor: notes.insertAtCursor,
    setActionError,
    setNotice,
  });

  const currentVault =
    vaults.find((vault) => vault.id === selectedVaultId) ?? null;

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

  useEffect(() => {
    if (!selectedVaultId) {
      setGraphData(null);
      return;
    }
    setMode("editor");
    setGraphData(null);
    setGraphError(null);
    setConfirmingRemoveVault(false);
    setActionError(null);
    setNotice(null);
  }, [selectedVaultId]);

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

    if (tree.treeLoadedVaultRef.current !== selectedVaultId) {
      // 树加载失败时仍尝试直接打开，笔记可能只是没进索引。
      if (
        tree.treeErrorVaultRef.current === selectedVaultId &&
        !tree.treeLoading
      ) {
        focusConsumedRef.current = focusKey;
        void notes.openNote(focusNoteId);
        onFocusConsumed?.();
      }
      return;
    }

    if (!tree.entries.some((entry) => entry.id === focusNoteId)) {
      focusConsumedRef.current = focusKey;
      setNotice("未找到要打开的笔记，可能已被移动或删除。");
      onFocusConsumed?.();
      return;
    }

    focusConsumedRef.current = focusKey;
    void notes.openNote(focusNoteId);
    onFocusConsumed?.();
  }, [
    focusKey,
    focusNoteId,
    focusVaultId,
    vaults,
    vaultsLoading,
    selectedVaultId,
    tree.entries,
    tree.treeLoading,
    tree.treeError,
    onFocusConsumed,
  ]);

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
              onPress={() => void tree.refreshTree(selectedVaultId, true)}
              isDisabled={tree.treeLoading}
            >
              <RefreshCw
                size={14}
                className={tree.treeLoading ? "animate-spin" : undefined}
              />
              {tree.treeLoading ? "索引中…" : "索引"}
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
        <AddVaultCard
          name={vaultName}
          onNameChange={setVaultName}
          root={vaultRoot}
          onRootChange={setVaultRoot}
          busy={addingVaultBusy}
          error={addVaultError}
          onSubmit={() => void submitVault()}
          onCancel={() => setAddingVault(false)}
        />
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
          <VaultSidebar
            searchQuery={tree.searchQuery}
            onSearchQueryChange={tree.setSearchQuery}
            searching={tree.searching}
            searchLoading={tree.searchLoading}
            searchError={tree.searchError}
            searchHits={tree.searchHits}
            fileCount={tree.fileCount}
            treeLoading={tree.treeLoading}
            creating={notes.creating}
            diaryBusy={notes.diaryBusy}
            onOpenDaily={() => void notes.openDaily()}
            onStartCreate={notes.startCreate}
            creatingPath={notes.creatingPath}
            onCreatingPathChange={notes.setCreatingPath}
            createError={notes.createError}
            onCreateSubmit={() => void notes.submitCreate()}
            onCancelCreate={() => {
              notes.setCreatingPath(null);
              notes.setCreateError(null);
            }}
            templates={notes.templates}
            templatePath={notes.templatePath}
            onTemplatePathChange={notes.setTemplatePath}
            tagsOpen={tree.tagsOpen}
            onToggleTags={() => tree.setTagsOpen((open) => !open)}
            tags={tree.tags}
            tagsLoading={tree.tagsLoading}
            activeTag={tree.activeTag}
            onSelectTag={tree.selectTag}
            treeError={tree.treeError}
            tagEntries={tree.tagEntries}
            tagEntriesLoading={tree.tagEntriesLoading}
            tagEntriesError={tree.tagEntriesError}
            entries={tree.entries}
            tree={tree.tree}
            collapsed={tree.collapsed}
            selectedNoteId={notes.selectedNoteId}
            onOpenNote={(id) => void notes.openNote(id)}
            onToggleFolder={tree.toggleFolder}
          />

          <section className="flex min-w-0 flex-1 flex-col overflow-hidden rounded-md border border-[var(--border)]">
            {mode === "graph" ? (
              <VaultGraphPane
                graphData={graphData}
                graphLoading={graphLoading}
                graphError={graphError}
                onOpenNote={(id) => {
                  setMode("editor");
                  void notes.openNote(id);
                }}
              />
            ) : (
              <NoteEditor
                note={notes.note}
                noteLoading={notes.noteLoading}
                noteError={notes.noteError}
                draftTitle={notes.draftTitle}
                onDraftTitleChange={notes.setDraftTitle}
                draftBody={notes.draftBody}
                onDraftBodyChange={(body, caret) => {
                  notes.setDraftBody(body);
                  notes.syncCompletion(body, caret);
                }}
                saving={notes.saving}
                deleting={notes.deleting}
                confirmingDelete={notes.confirmingDelete}
                renaming={notes.renaming}
                renamePath={notes.renamePath}
                onRenamePathChange={notes.setRenamePath}
                renameBusy={notes.renameBusy}
                onStartRename={() => {
                  if (!notes.note) return;
                  notes.setRenamePath(notes.note.path);
                  notes.setRenaming(true);
                }}
                onCancelRename={() => notes.setRenaming(false)}
                onSubmitRename={() => void notes.submitRename()}
                onSave={() => void notes.saveNote()}
                onRemove={() => void notes.removeNote()}
                actionError={actionError}
                completion={notes.completion}
                completionItems={notes.completionItems}
                completionActiveIndex={notes.completionActiveIndex}
                onPickCompletion={notes.pickCompletion}
                onBodyKeyDown={notes.handleBodyKeyDown}
                bodyRef={notes.bodyRef}
                attachmentInputRef={attachments.attachmentInputRef}
                onAttachmentClick={() =>
                  attachments.attachmentInputRef.current?.click()
                }
                onAttachmentInput={attachments.handleAttachmentInput}
                onBodyPaste={attachments.handleBodyPaste}
                onBodyDragEnter={attachments.handleBodyDragEnter}
                onBodyDragOver={attachments.handleBodyDragOver}
                onBodyDragLeave={attachments.handleBodyDragLeave}
                onBodyDrop={attachments.handleBodyDrop}
                attachmentDragActive={attachments.attachmentDragActive}
                uploading={attachments.uploading}
                vaultId={selectedVaultId}
              />
            )}
          </section>

          {mode === "editor" && notes.note && !notes.noteLoading && (
            <RightRail
              asideTab={notes.asideTab}
              onAsideTabChange={notes.setAsideTab}
              backlinks={notes.backlinks}
              backlinksLoading={notes.backlinksLoading}
              backlinksError={notes.backlinksError}
              selectedNoteId={notes.selectedNoteId}
              onOpenNote={(id) => void notes.openNote(id)}
              draftBody={notes.draftBody}
              onJumpLine={notes.jumpToLine}
              propertiesSaving={notes.propertiesSaving}
              onSaveProperties={notes.saveProperties}
            />
          )}
        </div>
      ) : null}

      <VaultQuickSwitcher
        isOpen={notes.quickOpen}
        notes={tree.quickNotes}
        onClose={() => notes.setQuickOpen(false)}
        onPick={(id) => {
          notes.setQuickOpen(false);
          setMode("editor");
          void notes.openNote(id);
        }}
      />
    </div>
  );
}
