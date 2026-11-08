import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Card, Chip, Input } from "@heroui/react";
import { DecisionInbox } from "./components/DecisionInbox";
import { TasksPanel } from "./components/TasksPanel";
import { Workspace } from "./components/Workspace";
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
  SlidersHorizontal,
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
  | "notes";

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
};

function emptyCopy(view: ViewId) {
  switch (view) {
    case "my":
      return "需要你审批或验收的任务会出现在这里。";
    case "reviews":
      return "任务完成后的证据与审查报告会出现在这里。";
    case "agent":
      return "Agent 会话、运行日志与审批请求会出现在这里。";
    case "views":
      return "保存的过滤视图会出现在这里。";
    default:
      return "先注册一个项目，之后任务、证据与决策都在这里汇总。";
  }
}

export default function App() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [tabs, setTabs] = useState<{ id: ViewId; label: string }[]>([
    { id: "overview", label: "概览" },
  ]);
  const [active, setActive] = useState<ViewId>("overview");
  const [showRegister, setShowRegister] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<Project[]>("list_projects")
      .then(setProjects)
      .catch((e) => setError(String(e)));
  }, []);

  const current = projects[0] ?? null;

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
    <div className="text-ui flex h-full overflow-hidden bg-[var(--background-secondary)] text-[var(--foreground)]">
      <aside className="flex w-[232px] shrink-0 flex-col px-2 pb-2 pt-9">
        <div className="flex items-center gap-0.5">
          <Button
            variant="ghost"
            size="sm"
            className="h-8 min-w-0 flex-1 justify-start gap-1.5 px-1.5 text-ui font-medium"
            onPress={() => open("projects")}
          >
            <FolderKanban size={15} className="shrink-0 opacity-70" />
            <span className="truncate">{current?.name ?? "temli"}</span>
            <ChevronDown size={13} className="shrink-0 opacity-50" />
          </Button>
          <Button
            isIconOnly
            variant="ghost"
            size="sm"
            className="h-8 w-8"
            aria-label="搜索"
          >
            <Search size={15} />
          </Button>
        </div>

        <nav className="mt-3 flex flex-col gap-0.5">
          {NAV.map((item) => (
            <Button
              key={item.id}
              variant={active === item.id ? "tertiary" : "ghost"}
              size="sm"
              className="h-7 justify-start gap-2 px-2 text-ui font-normal"
              onPress={() => open(item.id)}
            >
              <item.icon size={16} className="opacity-70" />
              {item.label}
            </Button>
          ))}
        </nav>

        <p className="text-label px-2 pb-1.5 pt-4 font-medium uppercase tracking-wide text-[var(--muted)]">
          空间
        </p>
        <nav className="flex flex-col gap-0.5">
          {SPACE.map((item) => (
            <Button
              key={item.id}
              variant={active === item.id ? "tertiary" : "ghost"}
              size="sm"
              className="h-7 justify-start gap-2 px-2 text-ui font-normal"
              onPress={() => open(item.id)}
            >
              <item.icon size={16} className="opacity-70" />
              {item.label}
            </Button>
          ))}
        </nav>

        <div className="mt-auto flex items-center gap-2 px-2 pt-3">
          <div className="flex size-5 items-center justify-center rounded-full bg-[var(--surface-secondary)] text-label font-medium text-[var(--muted)]">
            L
          </div>
          <span className="text-ui-sm text-[var(--muted)]">本地工作区</span>
        </div>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <div
          data-tauri-drag-region
          className="flex h-10 shrink-0 items-end gap-0.5 px-2 pb-1"
        >
          {tabs.map((tab) => {
            const isActive = active === tab.id;
            return (
              <div
                key={tab.id}
                className={`anim-fast group flex h-7 items-center rounded-md border ${
                  isActive
                    ? "border-[var(--border)] bg-[var(--background)] shadow-[0_1px_2px_rgba(0,0,0,0.06)]"
                    : "border-transparent hover:bg-[var(--background-tertiary)]"
                }`}
              >
                <button
                  type="button"
                  className={`tab-btn h-full pl-2.5 text-ui ${
                    isActive
                      ? "pr-2.5 text-[var(--foreground)]"
                      : "pr-2.5 text-[var(--muted)]"
                  }`}
                  onClick={() => setActive(tab.id)}
                >
                  {tab.label}
                </button>
                <button
                  type="button"
                  aria-label={`关闭 ${tab.label}`}
                  className={`icon-btn mr-1 flex size-5 items-center justify-center rounded opacity-0 transition-opacity duration-150 group-hover:opacity-60 hover:bg-[var(--surface-secondary)] ${
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
            className="h-7 w-7"
            aria-label="新建标签"
          >
            <Plus size={14} />
          </Button>
        </div>

        <main className="min-h-0 flex-1 overflow-hidden p-2 pt-0">
          <section className="flex h-full min-h-0 flex-col overflow-hidden rounded-lg border border-[var(--border)] bg-[var(--background)] shadow-[0_1px_2px_rgba(0,0,0,0.04)]">
            <header className="flex h-12 shrink-0 items-center justify-between border-b border-[var(--border)] px-4">
              <h1 className="text-ui font-medium">{TITLES[active]}</h1>
              <Button
                variant="primary"
                size="sm"
                className="h-7 gap-1 px-2.5 text-ui"
                onPress={startRegister}
              >
                <Plus size={14} />
                注册项目
              </Button>
            </header>

            <div className="flex h-9 shrink-0 items-center justify-between px-4">
              <div className="flex items-center gap-1">
                <Chip variant="soft" color="default" size="sm">
                  全部
                </Chip>
                <Chip variant="tertiary" color="default" size="sm">
                  进行中
                </Chip>
              </div>
              <Button
                isIconOnly
                variant="ghost"
                size="sm"
                className="h-6 w-6"
                aria-label="视图设置"
              >
                <SlidersHorizontal size={14} />
              </Button>
            </div>

            <div
              className={`min-h-0 flex-1 px-4 pb-4 ${
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
                      <ProjectList projects={projects} />
                      <DecisionInbox projectId={current.id} />
                    </div>
                  )}
                </>
              )}

              {active === "my" && (
                <>
                  {!current && <EmptyProject onRegister={startRegister} />}
                  {current && (
                    <TasksPanel key={current.id} projectId={current.id} />
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
                        setShowRegister(false);
                        setError(null);
                      }}
                      onError={setError}
                    />
                  )}
                  {projects.length > 0 && <ProjectList projects={projects} />}
                  {projects.length === 0 && !showRegister && (
                    <EmptyProject onRegister={startRegister} />
                  )}
                </>
              )}

              {active === "docs" && (
                <>
                  {!current && <EmptyProject onRegister={startRegister} />}
                  {current && (
                    <Workspace projectId={current.id} mode="docs" />
                  )}
                </>
              )}

              {active === "notes" && (
                <>
                  {!current && <EmptyProject onRegister={startRegister} />}
                  {current && (
                    <Workspace projectId={current.id} mode="notes" />
                  )}
                </>
              )}

              {active !== "overview" &&
                active !== "my" &&
                active !== "projects" &&
                active !== "docs" &&
                active !== "notes" && (
                  <div className="flex h-full flex-col items-center justify-center gap-1.5 text-center">
                    <p className="text-ui text-[var(--muted)]">
                      {emptyCopy(active)}
                    </p>
                    <p className="text-ui-sm text-[var(--muted)] opacity-70">
                      M0 骨架，功能在后续里程碑实现。
                    </p>
                  </div>
                )}
            </div>
          </section>
        </main>
      </div>
    </div>
  );
}

function EmptyProject({ onRegister }: { onRegister: () => void }) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2.5 text-center">
      <Inbox size={20} className="text-[var(--muted)] opacity-70" />
      <p className="text-ui text-[var(--muted)]">{emptyCopy("overview")}</p>
      <Button variant="primary" size="sm" className="h-7 px-3 text-ui" onPress={onRegister}>
        注册项目
      </Button>
    </div>
  );
}

function ProjectList({ projects }: { projects: Project[] }) {
  return (
    <div className="overflow-hidden rounded-md border border-[var(--border)]">
      <div className="text-ui-sm grid grid-cols-[1.2fr_1.8fr_100px_88px] gap-3 border-b border-[var(--border)] bg-[var(--background-secondary)] px-3 py-1.5 text-[var(--muted)]">
        <span>名称</span>
        <span>仓库</span>
        <span>分支</span>
        <span>状态</span>
      </div>
      {projects.map((project) => (
        <div
          key={project.id}
          className="anim-fast text-ui grid h-9 grid-cols-[1.2fr_1.8fr_100px_88px] items-center gap-3 border-b border-[var(--border)] px-3 last:border-b-0 hover:bg-[var(--background-secondary)]"
        >
          <span className="flex items-center gap-2">
            <FolderKanban size={15} className="text-[var(--muted)]" />
            {project.name}
          </span>
          <span className="truncate text-[var(--muted)]">{project.repo_path}</span>
          <span className="text-[var(--muted)]">{project.default_branch}</span>
          <Chip size="sm" variant="soft" color="success">
            已注册
          </Chip>
        </div>
      ))}
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
    <Card className="mb-4 max-w-[680px]">
      <Card.Header>
        <Card.Title className="text-ui font-medium">注册项目</Card.Title>
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
          />
        </label>
        <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
          名称（可选）
          <Input
            aria-label="名称"
            placeholder="temli-demo"
            value={name}
            onChange={(e) => setName(e.target.value)}
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
            />
          </label>
          <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
            GitHub repo
            <Input
              aria-label="GitHub repo"
              placeholder="temli-demo"
              value={repo}
              onChange={(e) => setRepo(e.target.value)}
            />
          </label>
        </div>
        <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
          默认分支
          <Input
            aria-label="默认分支"
            value={branch}
            onChange={(e) => setBranch(e.target.value)}
          />
        </label>
      </Card.Content>
      <Card.Footer className="flex justify-end gap-1.5">
        <Button variant="ghost" size="sm" onPress={onCancel} isDisabled={busy}>
          取消
        </Button>
        <Button
          variant="primary"
          size="sm"
          onPress={submit}
          isDisabled={busy || !repoPath}
        >
          {busy ? "注册中…" : "注册"}
        </Button>
      </Card.Footer>
    </Card>
  );
}
