import { useEffect, useMemo, useRef, useState } from "react";
import {
  errorText,
  indexVault,
  notesByTag,
  searchVault,
  vaultTags,
  vaultTree,
  type VaultEntry,
  type VaultHit,
  type VaultTag,
} from "../../lib/api";
import { buildTree, noteTitle } from "./shared";

type UseVaultTreeOptions = {
  selectedVaultId: string;
  setNotice: (message: string) => void;
  setActionError: (message: string | null) => void;
  bumpGraphSeed: () => void;
};

export function useVaultTree({
  selectedVaultId,
  setNotice,
  setActionError,
  bumpGraphSeed,
}: UseVaultTreeOptions) {
  const [entries, setEntries] = useState<VaultEntry[]>([]);
  const [treeLoading, setTreeLoading] = useState(false);
  const [treeError, setTreeError] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const treeRequest = useRef(0);
  const vaultRef = useRef(selectedVaultId);
  vaultRef.current = selectedVaultId;
  const treeLoadedVaultRef = useRef<string | null>(null);
  const treeErrorVaultRef = useRef<string | null>(null);

  const [searchQuery, setSearchQuery] = useState("");
  const [searchHits, setSearchHits] = useState<VaultHit[]>([]);
  const [searchLoading, setSearchLoading] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);

  const [tags, setTags] = useState<VaultTag[]>([]);
  const [tagsLoading, setTagsLoading] = useState(false);
  const [tagsOpen, setTagsOpen] = useState(true);
  const [activeTag, setActiveTag] = useState<string | null>(null);
  const [tagEntries, setTagEntries] = useState<VaultEntry[]>([]);
  const [tagEntriesLoading, setTagEntriesLoading] = useState(false);
  const [tagEntriesError, setTagEntriesError] = useState<string | null>(null);
  const [tagsVersion, setTagsVersion] = useState(0);
  const tagRequest = useRef(0);
  const tagEntriesRequest = useRef(0);

  const tree = useMemo(() => buildTree(entries), [entries]);
  const fileCount = entries.filter((entry) => !entry.is_dir).length;
  const searching = searchQuery.trim().length > 0;
  const quickNotes = useMemo(
    () =>
      entries
        .filter((entry) => !entry.is_dir && entry.id)
        .map((entry) => ({
          id: entry.id as string,
          path: entry.path,
          title: entry.title || noteTitle(entry.path),
        })),
    [entries],
  );

  async function refreshTree(vaultId: string, reindex: boolean) {
    const request = ++treeRequest.current;
    if (vaultId === vaultRef.current) treeErrorVaultRef.current = null;
    setTreeLoading(true);
    setTreeError(null);
    try {
      if (reindex) {
        await indexVault(vaultId);
        if (vaultId === vaultRef.current) bumpGraphSeed();
      }
      const loaded = await vaultTree(vaultId);
      if (request !== treeRequest.current || vaultId !== vaultRef.current) return;
      setEntries(loaded);
      treeLoadedVaultRef.current = vaultId;
      if (reindex) {
        setNotice(
          `已索引 ${loaded.filter((entry) => !entry.is_dir).length} 条笔记`,
        );
        void refreshTags(vaultId);
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

  async function refreshTags(vaultId: string) {
    const request = ++tagRequest.current;
    if (vaultId === vaultRef.current) setTagsLoading(true);
    try {
      const loaded = await vaultTags(vaultId);
      if (request !== tagRequest.current || vaultId !== vaultRef.current) return;
      setTags(loaded);
      setActiveTag((previous) =>
        previous && !loaded.some((tag) => tag.tag === previous)
          ? null
          : previous,
      );
      setTagsVersion((version) => version + 1);
    } catch (error) {
      if (request === tagRequest.current && vaultId === vaultRef.current) {
        setActionError(errorText(error));
      }
    } finally {
      if (request === tagRequest.current) setTagsLoading(false);
    }
  }

  useEffect(() => {
    if (!selectedVaultId) {
      treeRequest.current += 1;
      tagEntriesRequest.current += 1;
      treeLoadedVaultRef.current = null;
      treeErrorVaultRef.current = null;
      setEntries([]);
      setTreeError(null);
      setTreeLoading(false);
      setActiveTag(null);
      setTagEntries([]);
      return;
    }

    tagEntriesRequest.current += 1;
    treeLoadedVaultRef.current = null;
    treeErrorVaultRef.current = null;
    setEntries([]);
    setCollapsed(new Set());
    setSearchQuery("");
    setSearchHits([]);
    setSearchError(null);
    setActiveTag(null);
    setTagEntries([]);
    setTagEntriesError(null);
    setTagEntriesLoading(false);

    void refreshTree(selectedVaultId, true);
  }, [selectedVaultId]);

  useEffect(() => {
    if (!selectedVaultId) {
      tagRequest.current += 1;
      setTags([]);
      setTagsLoading(false);
      return;
    }
    setTags([]);
    void refreshTags(selectedVaultId);
  }, [selectedVaultId]);

  useEffect(() => {
    if (!selectedVaultId || !activeTag) {
      tagEntriesRequest.current += 1;
      setTagEntries([]);
      setTagEntriesLoading(false);
      setTagEntriesError(null);
      return;
    }
    const request = ++tagEntriesRequest.current;
    let cancelled = false;
    setTagEntriesLoading(true);
    setTagEntriesError(null);
    notesByTag(selectedVaultId, activeTag)
      .then((loaded) => {
        if (!cancelled && request === tagEntriesRequest.current) {
          setTagEntries(loaded);
        }
      })
      .catch((error) => {
        if (!cancelled && request === tagEntriesRequest.current) {
          setTagEntries([]);
          const message = errorText(error);
          setTagEntriesError(message);
          setActionError(message);
        }
      })
      .finally(() => {
        if (!cancelled && request === tagEntriesRequest.current) {
          setTagEntriesLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [selectedVaultId, activeTag, tagsVersion]);

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

  function selectTag(tag: string | null) {
    setActiveTag(tag);
    setSearchQuery("");
  }

  function toggleFolder(path: string) {
    setCollapsed((previous) => {
      const next = new Set(previous);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  return {
    entries,
    treeLoading,
    treeError,
    collapsed,
    tree,
    fileCount,
    searching,
    searchQuery,
    setSearchQuery,
    searchHits,
    searchLoading,
    searchError,
    tags,
    tagsLoading,
    tagsOpen,
    setTagsOpen,
    activeTag,
    tagEntries,
    tagEntriesLoading,
    tagEntriesError,
    refreshTree,
    refreshTags,
    selectTag,
    toggleFolder,
    treeLoadedVaultRef,
    treeErrorVaultRef,
    quickNotes,
  };
}
