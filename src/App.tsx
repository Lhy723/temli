import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Button,
  Card,
  Chip,
  Description,
  Dropdown,
  Header,
  Input,
  Label,
  Separator,
} from "@heroui/react";
import { CommandPalette } from "./components/CommandPalette";
import { DecisionInbox } from "./components/DecisionInbox";
import { TasksPanel } from "./components/TasksPanel";
import { Vaults } from "./components/Vaults";
import { Workspace } from "./components/Workspace";
import { ReviewsPanel } from "./components/ReviewsPanel";
import { AgentsPanel } from "./components/AgentsPanel";
import { SettingsPanel } from "./components/SettingsPanel";
import { getTask, type GlobalHit } from "./lib/api";
import {
  Bot,
  ChevronDown,
  CircleDot,
  FileText,
  FolderKanban,
  GitPullRequest,
  Inbox,
  LayoutList,
  NotebookPen,
  Plus,
  Search,
  Settings2,
  X,
} from "lucide-react";

type Project = {
  id: string;
  name: string;
  repo_path: string;
  github_owner: string | null;
  github_repo: string | null;
  default_branch: string;
  created_at: string;
};

type ViewId =
  | "overview"
  | "my"
  | "reviews"
  | "agent"
  | "projects"
  | "views"
  | "docs"
  | "notes"
  | "settings";

const NAV: { id: ViewId; label: string; icon: typeof Inbox }[] = [
  { id: "overview", label: "概览", icon: Inbox },
  { id: "my", label: "我的任务", icon: CircleDot },
  { id: "reviews", label: "审查", icon: GitPullRequest },
  { id: "agent", label: "Agent", icon: Bot },
];

const SPACE: { id: ViewId; label: string; icon: typeof Inbox }[] = [
  { id: "projects", label: "项目", icon: FolderKanban },
  { id: "views", label: "视图", icon: LayoutList },
  { id: "docs", label: "文档", icon: FileText },
  { id: "notes", label: "知识", icon: NotebookPen },
];

const TITLES: Record<ViewId, string> = {
  overview: "概览",
  my: "我的任务",
  reviews: "审查",
  agent: "Agent",
  projects: "项目",
  views: "视图",
  docs: "文档",
  notes: "知识",
  settings: "设置",
};

function emptyCopy(view: ViewId) {
  switch (view) {
    case "views":
      return "保存的过滤视图会出现在这里。";
    default:
      return "先注册一个项目，之后任务、证据与决策都在这里汇总。";
  }
}

const SELECTED_PROJECT_KEY = "temli.selectedProjectId";

function readStoredProjectId(): string | null {
  try {
    return localStorage.getItem(SELECTED_PROJECT_KEY) || null;
  } catch {
    return null;
  }
}

function storeProjectId(projectId: string | null) {
  try {
    if (projectId) localStorage.setItem(SELECTED_PROJECT_KEY, projectId);
    else localStorage.removeItem(SELECTED_PROJECT_KEY);
  } catch {
    // localStorage may be unavailable; selection still works in memory.
  }
}

export default function App() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [projectsLoaded, setProjectsLoaded] = useState(false);
  const [selectedProjectId, setSelectedProjectId] = useState<string | null>(
    readStoredProjectId,
  );
  const [tabs, setTabs] = useState<{ id: ViewId; label: string }[]>([
    { id: "overview", label: "概览" },
  ]);
  const [active, setActive] = useState<ViewId>("overview");
  const [showRegister, setShowRegister] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [focusTaskId, setFocusTaskId] = useState<string | null>(null);
  const [focusDocId, setFocusDocId] = useState<string | null>(null);
  const [focusNoteId, setFocusNoteId] = useState<string | null>(null);
  const [focusVaultId, setFocusVaultId] = useState<string | null>(null);

  useEffect(() => {
    invoke<Project[]>("list_projects")
      .then((list) => {
        setProjects(list);
        setProjectsLoaded(true);
      })
      .catch((e) => {
        setProjectsLoaded(true);
        setError(String(e));
      });
  }, []);

  useEffect(() => {
    if (!projectsLoaded) return;
    if (selectedProjectId && projects.some((p) => p.id === selectedProjectId)) {
      return;
    }
    const fallback = projects[0]?.id ?? null;
    if (fallback !== selectedProjectId) setSelectedProjectId(fallback);
  }, [projects, projectsLoaded, selectedProjectId]);

  useEffect(() => {
    storeProjectId(selectedProjectId);
  }, [selectedProjectId]);

  useEffect(() => {
    function handleShortcut(event: KeyboardEvent) {
      if (event.repeat) return;
      if (!(event.metaKey || event.ctrlKey)) return;
      if (event.altKey || event.key.toLowerCase() !== "k") return;
      event.preventDefault();
      setPaletteOpen((open) => !open);
    }
    window.addEventListener("keydown", handleShortcut);
    return () => window.removeEventListener("keydown", handleShortcut);
  }, []);

  // 点击系统通知：解析任务所属项目，切到“我的任务”并打开对应任务。
  useEffect(() => {
    let disposed = false;
    let request = 0;
    let unlisten: (() => void) | null = null;
    listen<{ task_id?: string }>("navigate-task", (event) => {
      const taskId = event.payload?.task_id;
      if (!taskId) return;
      const seq = ++request;
      void getTask(taskId)
        .then((detail) => {
          if (disposed || seq !== request) return;
          switchProject(detail.task.project_id);
          setFocusTaskId(taskId);
          open("my");
        })
        .catch((e) => {
          if (!disposed && seq === request) setError(String(e));
        });
    })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch((e) => {
        if (!disposed) setError(String(e));
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
    // 监听只注册一次；状态更新通过 setState 与组件内函数完成，不依赖当次渲染的闭包值。
  }, []);

  const current =
    projects.find((project) => project.id === selectedProjectId) ??
    projects[0] ??
    null;

  function clearFocus() {
    setFocusTaskId(null);
    setFocusDocId(null);
    setFocusNoteId(null);
    setFocusVaultId(null);
  }

  function switchProject(projectId: string | null) {
    setSelectedProjectId(projectId);
    clearFocus();
  }

  function openGlobalHit(hit: GlobalHit) {
    if (
      hit.kind !== "note" &&
      hit.project_id &&
      hit.project_id !== current?.id
    ) {
      switchProject(hit.project_id);
    }
    if (hit.kind === "task") {
      setFocusTaskId(hit.id);
      open("my");
    } else if (hit.kind === "doc") {
      setFocusDocId(hit.id);
      open("docs");
    } else {
      setFocusNoteId(hit.id);
      setFocusVaultId(hit.vault_id ?? null);
      open("notes");
    }
  }

  function consumeNoteFocus() {
    setFocusNoteId(null);
    setFocusVaultId(null);
  }

  function open(id: ViewId) {
    setTabs((prev) =>
      prev.some((t) => t.id === id) ? prev : [...prev, { id, label: TITLES[id] }],
    );
    setActive(id);
  }

  function close(id: ViewId) {
    setTabs((prev) => {
      const next = prev.filter((t) => t.id !== id);
      if (next.length === 0) {
        setActive("overview");
        return [{ id: "overview", label: TITLES.overview }];
      }
      if (active === id) setActive(next[next.length - 1].id);
      return next;
    });
  }

  function startRegister() {
    open("projects");
    setShowRegister(true);
  }

  return (
    <div className="text-ui flex h-full overflow-hidden bg-[var(--shell-bg)] text-[var(--foreground)]">
      <aside className="flex w-[232px] shrink-0 flex-col px-2 pb-2 pt-9">
        <div className="flex items-center gap-0.5">
          <Dropdown>
            <Button
              variant="ghost"
              size="sm"
              className="anim-fast h-8 min-w-0 flex-1 justify-start gap-1.5 rounded-lg px-1.5 text-ui font-medium"
              aria-label={`切换项目：${current?.name ?? "temli"}`}
            >
              <FolderKanban size={15} className="shrink-0 opacity-70" />
              <span className="truncate">{current?.name ?? "temli"}</span>
              <ChevronDown size={13} className="shrink-0 opacity-50" />
            </Button>
            <Dropdown.Popover className="min-w-[260px]">
              <Dropdown.Menu
                selectionMode="single"
                selectedKeys={current ? [current.id] : []}
                onAction={(key) => {
                  const value = String(key);
                  if (value === "register-project") {
                    startRegister();
                    return;
                  }
                  switchProject(value);
                }}
              >
                {projects.length > 0 && (
                  <Dropdown.Section>
                    <Header>切换项目</Header>
                    {projects.map((project) => (
                      <Dropdown.Item
                        key={project.id}
                        id={project.id}
                        textValue={`${project.name} ${project.repo_path}`}
                      >
                        <FolderKanban
                          size={15}
                          className="shrink-0 text-[var(--muted)]"
                        />
                        <div className="flex min-w-0 flex-1 flex-col">
                          <Label>{project.name}</Label>
                          <Description className="truncate">
                            {project.repo_path}
                          </Description>
                        </div>
                        {project.id === current?.id && (
                          <Dropdown.ItemIndicator className="ms-auto" />
                        )}
                      </Dropdown.Item>
                    ))}
                  </Dropdown.Section>
                )}
                {projects.length > 0 && <Separator />}
                <Dropdown.Item id="register-project" textValue="注册项目">
                  <Plus size={15} className="shrink-0 text-[var(--muted)]" />
                  <Label>注册项目</Label>
                </Dropdown.Item>
              </Dropdown.Menu>
            </Dropdown.Popover>
          </Dropdown>
          <Button
            isIconOnly
            variant="ghost"
            size="sm"
            className="anim-fast h-8 w-8 rounded-lg"
            aria-label="全局搜索（⌘K）"
            onPress={() => setPaletteOpen(true)}
          >
            <Search size={15} />
          </Button>
        </div>

        <nav className="mt-3 flex flex-col gap-0.5">
          {NAV.map((item) => (
            <Button
              key={item.id}
              variant="ghost"
              size="sm"
              className={`anim-fast h-8 justify-start gap-2 rounded-lg px-2 text-ui font-normal ${
                active === item.id
                  ? "bg-[var(--surface)] text-[var(--foreground)]"
                  : "text-[var(--muted)] hover:bg-[var(--background-tertiary)] hover:text-[var(--foreground)]"
              }`}
              onPress={() => open(item.id)}
            >
              <item.icon size={16} className="opacity-70" />
              {item.label}
            </Button>
          ))}
        </nav>

        <p className="text-label px-2 pb-1.5 pt-4 font-medium uppercase tracking-[0.06em] text-[var(--muted)]">
          空间
        </p>
        <nav className="flex flex-col gap-0.5">
          {SPACE.map((item) => (
            <Button
              key={item.id}
              variant="ghost"
              size="sm"
              className={`anim-fast h-8 justify-start gap-2 rounded-lg px-2 text-ui font-normal ${
                active === item.id
                  ? "bg-[var(--surface)] text-[var(--foreground)]"
                  : "text-[var(--muted)] hover:bg-[var(--background-tertiary)] hover:text-[var(--foreground)]"
              }`}
              onPress={() => open(item.id)}
            >
              <item.icon size={16} className="opacity-70" />
              {item.label}
            </Button>
          ))}
        </nav>

        <div className="mt-auto flex flex-col gap-0.5 pt-3">
          <Button
            variant="ghost"
            size="sm"
            className={`anim-fast h-8 justify-start gap-2 rounded-lg px-2 text-ui font-normal ${
              active === "settings"
                ? "bg-[var(--surface)] text-[var(--foreground)]"
                : "text-[var(--muted)] hover:bg-[var(--background-tertiary)] hover:text-[var(--foreground)]"
            }`}
            onPress={() => open("settings")}
          >
            <Settings2 size={16} className="opacity-70" />
            设置
          </Button>
          <div className="flex items-center gap-2 px-2 pt-2">
            <div className="flex size-5 items-center justify-center rounded-full bg-[var(--surface-secondary)] text-label font-medium text-[var(--muted)]">
              L
            </div>
            <span className="text-ui-sm text-[var(--muted)]">本地工作区</span>
          </div>
        </div>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <div
          data-tauri-drag-region
          className="flex h-11 shrink-0 items-center gap-1 px-3"
        >
          {tabs.map((tab) => {
            const isActive = active === tab.id;
            return (
              <div
                key={tab.id}
                className={`anim-fast group flex h-8 items-center rounded-lg border ${
                  isActive
                    ? "border-[var(--border)] bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)]"
                    : "border-transparent hover:bg-[var(--background-tertiary)]"
                }`}
              >
                <button
                  type="button"
                  className={`tab-btn h-full rounded-lg pl-2.5 pr-2 text-ui ${
                    isActive
                      ? "text-[var(--foreground)]"
                      : "text-[var(--muted)] hover:text-[var(--foreground)]"
                  }`}
                  onClick={() => setActive(tab.id)}
                >
                  {tab.label}
                </button>
                <button
                  type="button"
                  aria-label={`关闭 ${tab.label}`}
                  className={`icon-btn mr-1 flex size-5 items-center justify-center rounded-lg opacity-0 transition-opacity duration-150 motion-reduce:transition-none group-hover:opacity-60 hover:bg-[var(--surface-secondary)] hover:opacity-100 ${
                    isActive ? "" : "text-[var(--muted)]"
                  }`}
                  onClick={() => close(tab.id)}
                >
                  <X size={11} />
                </button>
              </div>
            );
          })}
          <Button
            isIconOnly
            variant="ghost"
            size="sm"
            className="h-8 w-8 rounded-lg"
            aria-label="新建标签"
          >
            <Plus size={14} />
          </Button>
        </div>

        <main className="min-h-0 flex-1 overflow-hidden px-3 pb-3 pt-1">
          <section className="mx-auto flex h-full min-h-0 w-full max-w-[1360px] flex-col overflow-hidden rounded-[14px] border border-[var(--border)] bg-[var(--background)] shadow-[0_1px_3px_rgba(0,0,0,0.04)]">
            <header className="flex h-12 shrink-0 items-center justify-between gap-3 border-b border-[var(--border)] px-6">
              <div className="flex items-center gap-2">
                <span className="text-ui-sm text-[var(--muted)]">
                  {current ? current.name : "未选择项目"}
                </span>
              </div>
              <div className="flex items-center gap-2">
                <Button
                  isIconOnly
                  variant="ghost"
                  size="sm"
                  className="h-8 w-8 rounded-lg"
                  aria-label="全局搜索（⌘K）"
                  onPress={() => setPaletteOpen(true)}
                >
                  <Search size={15} />
                </Button>
                <Button
                  variant="primary"
                  size="sm"
                  className="h-8 gap-1 px-3 text-ui"
                  onPress={startRegister}
                >
                  <Plus size={14} />
                  注册项目
                </Button>
              </div>
            </header>

            <div
              key={active}
              className={`page-fade min-h-0 flex-1 px-6 pb-6 pt-4 ${
                active === "my" || active === "docs" || active === "notes"
                  ? "overflow-hidden"
                  : "overflow-auto"
              }`}
            >
              {error && (
                <p role="alert" className="mb-3 text-ui-sm text-[var(--danger)]">
                  {error}
                </p>
              )}

              {active === "overview" && (
                <>
                  {!current && <EmptyProject onRegister={startRegister} />}
                  {current && (
                    <div className="flex flex-col gap-3">
                      <ProjectList
                        projects={projects}
                        selectedId={current.id}
                      />
                      <DecisionInbox projectId={current.id} />
                    </div>
                  )}
                </>
              )}

              {active === "my" && (
                <>
                  {!current && <EmptyProject onRegister={startRegister} />}
                  {current && (
                    <TasksPanel
                      key={current.id}
                      projectId={current.id}
                      focusTaskId={focusTaskId ?? undefined}
                      onFocusConsumed={() => setFocusTaskId(null)}
                    />
                  )}
                </>
              )}

              {active === "projects" && (
                <>
                  {showRegister && (
                    <RegisterProject
                      onCancel={() => setShowRegister(false)}
                      onRegistered={(project) => {
                        setProjects((prev) => [...prev, project]);
                        switchProject(project.id);
                        setShowRegister(false);
                        setError(null);
                      }}
                      onError={setError}
                    />
                  )}
                  {projects.length > 0 && (
                    <ProjectList projects={projects} selectedId={current?.id} />
                  )}
                  {projects.length === 0 && !showRegister && (
                    <EmptyProject onRegister={startRegister} />
                  )}
                </>
              )}

              {active === "docs" && (
                <>
                  {!current && <EmptyProject onRegister={startRegister} />}
                  {current && (
                    <Workspace
                      projectId={current.id}
                      mode="docs"
                      focusDocId={focusDocId ?? undefined}
                      onFocusConsumed={() => setFocusDocId(null)}
                    />
                  )}
                </>
              )}

              {active === "notes" && (
                <Vaults
                  focusNoteId={focusNoteId ?? undefined}
                  focusVaultId={focusVaultId ?? undefined}
                  onFocusConsumed={consumeNoteFocus}
                />
              )}

              {active === "reviews" &&
                (current ? (
                  <ReviewsPanel projectId={current.id} />
                ) : (
                  <EmptyProject onRegister={startRegister} />
                ))}

              {active === "agent" && (
                <AgentsPanel projectId={current?.id ?? ""} />
              )}

              {active === "views" && (
                <EmptyState
                  icon={LayoutList}
                  title="暂无视图"
                  hint={emptyCopy("views")}
                />
              )}

              {active === "settings" && (
                <SettingsPanel projectId={current?.id ?? null} />
              )}
            </div>
          </section>
        </main>
      </div>

      <CommandPalette
        isOpen={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        projectId={current?.id ?? null}
        projectName={current?.name ?? null}
        onSelect={openGlobalHit}
      />
    </div>
  );
}

function EmptyState({
  icon: Icon,
  title,
  hint,
  actionLabel,
  onAction,
}: {
  icon: typeof Inbox;
  title: string;
  hint: string;
  actionLabel?: string;
  onAction?: () => void;
}) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 text-center">
      <Icon size={20} className="text-[var(--muted)] opacity-70" />
      <p className="text-ui font-medium">{title}</p>
      <p className="max-w-[320px] text-ui-sm text-[var(--muted)]">{hint}</p>
      {actionLabel && onAction && (
        <Button
          variant="primary"
          size="sm"
          className="mt-1 h-8 px-3 text-ui"
          onPress={onAction}
        >
          {actionLabel}
        </Button>
      )}
    </div>
  );
}

function EmptyProject({ onRegister }: { onRegister: () => void }) {
  return (
    <EmptyState
      icon={Inbox}
      title="还没有项目"
      hint={emptyCopy("overview")}
      actionLabel="注册项目"
      onAction={onRegister}
    />
  );
}

function ProjectList({
  projects,
  selectedId,
}: {
  projects: Project[];
  selectedId?: string | null;
}) {
  return (
    <div className="overflow-hidden rounded-[10px] border border-[var(--border)]">
      <div className="text-label grid grid-cols-[1.2fr_1.8fr_100px_88px] gap-3 border-b border-[var(--border)] bg-[var(--shell-bg)] px-3 py-2 font-medium uppercase tracking-[0.06em] text-[var(--muted)]">
        <span>名称</span>
        <span>仓库</span>
        <span>分支</span>
        <span>状态</span>
      </div>
      {projects.map((project) => {
        const isSelected = project.id === selectedId;
        return (
          <div
            key={project.id}
            className={`anim-fast text-ui grid h-10 grid-cols-[1.2fr_1.8fr_100px_88px] items-center gap-3 border-b border-[var(--border)] px-3 last:border-b-0 hover:bg-[var(--shell-bg)] ${
              isSelected ? "bg-[var(--shell-bg)]" : ""
            }`}
          >
            <span className="flex items-center gap-2">
              <FolderKanban
                size={15}
                className={
                  isSelected
                    ? "text-[var(--accent)]"
                    : "text-[var(--muted)]"
                }
              />
              {project.name}
            </span>
            <span className="truncate text-[var(--muted)]">
              {project.repo_path}
            </span>
            <span className="text-[var(--muted)]">
              {project.default_branch}
            </span>
            <Chip
              size="sm"
              variant="soft"
              color={isSelected ? "accent" : "success"}
            >
              {isSelected ? "当前" : "已注册"}
            </Chip>
          </div>
        );
      })}
    </div>
  );
}

function RegisterProject({
  onCancel,
  onRegistered,
  onError,
}: {
  onCancel: () => void;
  onRegistered: (project: Project) => void;
  onError: (message: string) => void;
}) {
  const [repoPath, setRepoPath] = useState("");
  const [name, setName] = useState("");
  const [owner, setOwner] = useState("");
  const [repo, setRepo] = useState("");
  const [branch, setBranch] = useState("main");
  const [busy, setBusy] = useState(false);

  async function submit() {
    setBusy(true);
    try {
      const project = await invoke<Project>("register_project", {
        input: {
          repo_path: repoPath,
          name: name || null,
          github_owner: owner || null,
          github_repo: repo || null,
          default_branch: branch || "main",
        },
      });
      onRegistered(project);
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card className="mb-4 max-w-[680px] rounded-[10px] p-3.5">
      <Card.Header>
        <Card.Title className="text-[15px] font-medium leading-[1.4]">
          注册项目
        </Card.Title>
        <Card.Description className="text-ui-sm">
          本地仓库必须是 git 仓库，且 `.temli/` 已在 `.gitignore` 中忽略。
        </Card.Description>
      </Card.Header>
      <Card.Content className="grid gap-3">
        <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
          仓库路径
          <Input
            aria-label="仓库路径"
            placeholder="/Users/lhy/Project/Client/temli-demo"
            value={repoPath}
            onChange={(e) => setRepoPath(e.target.value)}
            className="h-8 text-ui"
          />
        </label>
        <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
          名称（可选）
          <Input
            aria-label="名称"
            placeholder="temli-demo"
            value={name}
            onChange={(e) => setName(e.target.value)}
            className="h-8 text-ui"
          />
        </label>
        <div className="grid grid-cols-2 gap-3">
          <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
            GitHub owner
            <Input
              aria-label="GitHub owner"
              placeholder="Lhy723"
              value={owner}
              onChange={(e) => setOwner(e.target.value)}
              className="h-8 text-ui"
            />
          </label>
          <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
            GitHub repo
            <Input
              aria-label="GitHub repo"
              placeholder="temli-demo"
              value={repo}
              onChange={(e) => setRepo(e.target.value)}
              className="h-8 text-ui"
            />
          </label>
        </div>
        <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
          默认分支
          <Input
            aria-label="默认分支"
            value={branch}
            onChange={(e) => setBranch(e.target.value)}
            className="h-8 text-ui"
          />
        </label>
      </Card.Content>
      <Card.Footer className="flex justify-end gap-1.5">
        <Button
          variant="ghost"
          size="sm"
          className="h-8 px-3 text-ui"
          onPress={onCancel}
          isDisabled={busy}
        >
          取消
        </Button>
        <Button
          variant="primary"
          size="sm"
          className="h-8 px-3 text-ui"
          onPress={submit}
          isDisabled={busy || !repoPath}
        >
          {busy ? "注册中…" : "注册"}
        </Button>
      </Card.Footer>
    </Card>
  );
}
