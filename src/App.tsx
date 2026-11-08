import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Card, Chip, Input } from "@heroui/react";
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
    case "docs":
      return "仓库 docs/ 下的文档会出现在这里。";
    case "notes":
      return "项目内 .temli/notes 的笔记会出现在这里。";
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
    <div className="flex h-full overflow-hidden bg-[var(--background-secondary)] text-[var(--foreground)]">
      <aside className="flex w-60 shrink-0 flex-col px-2 pb-2 pt-10">
        <div className="flex items-center gap-1 pb-3">
          <Button
            variant="ghost"
            size="sm"
            className="min-w-0 flex-1 justify-start gap-2"
            onPress={() => open("projects")}
          >
            <FolderKanban size={16} />
            <span className="truncate">{current?.name ?? "temli"}</span>
            <ChevronDown size={14} className="shrink-0 opacity-60" />
          </Button>
          <Button isIconOnly variant="ghost" size="sm" aria-label="搜索">
            <Search size={16} />
          </Button>
        </div>

        <nav className="flex flex-col gap-0.5">
          {NAV.map((item) => (
            <Button
              key={item.id}
              variant="ghost"
              size="sm"
              className="justify-start gap-2"
              onPress={() => open(item.id)}
            >
              <item.icon size={16} />
              {item.label}
            </Button>
          ))}
        </nav>

        <p className="px-2 pb-1 pt-4 text-xs text-[var(--muted)]">空间</p>
        <nav className="flex flex-col gap-0.5">
          {SPACE.map((item) => (
            <Button
              key={item.id}
              variant="ghost"
              size="sm"
              className="justify-start gap-2"
              onPress={() => open(item.id)}
            >
              <item.icon size={16} />
              {item.label}
            </Button>
          ))}
        </nav>

        <div className="mt-auto flex items-center gap-2 px-2 pt-3">
          <div className="flex size-6 items-center justify-center rounded-full bg-[var(--surface-secondary)] text-xs">
            L
          </div>
          <span className="text-sm text-[var(--muted)]">本地工作区</span>
        </div>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <div
          data-tauri-drag-region
          className="flex h-11 shrink-0 items-end gap-1 px-3"
        >
          {tabs.map((tab) => (
            <div
              key={tab.id}
              className={`group flex h-8 items-center gap-1 rounded-md px-2 text-sm ${
                active === tab.id
                  ? "bg-[var(--background)] shadow-sm"
                  : "text-[var(--muted)]"
              }`}
            >
              <button
                type="button"
                className="max-w-40 truncate"
                onClick={() => setActive(tab.id)}
              >
                {tab.label}
              </button>
              <button
                type="button"
                aria-label={`关闭 ${tab.label}`}
                className="opacity-0 transition-opacity group-hover:opacity-60"
                onClick={() => close(tab.id)}
              >
                <X size={12} />
              </button>
            </div>
          ))}
          <Button isIconOnly variant="ghost" size="sm" aria-label="新建视图">
            <Plus size={15} />
          </Button>
        </div>

        <main className="min-h-0 flex-1 overflow-hidden px-2 pb-2">
          <section className="flex h-full min-h-0 flex-col overflow-hidden rounded-xl border border-[var(--border)] bg-[var(--background)]">
            <header className="flex items-center justify-between px-4 py-3">
              <h1 className="text-sm font-medium">{TITLES[active]}</h1>
              <Button
                variant="primary"
                size="sm"
                className="gap-1"
                onPress={startRegister}
              >
                <Plus size={15} />
                注册项目
              </Button>
            </header>

            <div className="flex items-center justify-between gap-2 px-4 pb-2">
              <div className="flex items-center gap-1">
                <Chip variant="soft" color="default" size="sm">
                  全部
                </Chip>
                <Chip variant="tertiary" color="default" size="sm">
                  进行中
                </Chip>
              </div>
              <Button isIconOnly variant="ghost" size="sm" aria-label="视图设置">
                <SlidersHorizontal size={15} />
              </Button>
            </div>

            <div className="min-h-0 flex-1 overflow-auto px-4 pb-4">
              {error && (
                <p role="alert" className="mb-3 text-sm text-[var(--danger)]">
                  {error}
                </p>
              )}

              {active === "overview" && (
                <>
                  {projects.length === 0 && <EmptyProject onRegister={startRegister} />}
                  {projects.length > 0 && <ProjectList projects={projects} />}
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

              {active !== "overview" && active !== "projects" && (
                <div className="flex h-full flex-col items-center justify-center gap-2 text-center">
                  <p className="text-sm text-[var(--muted)]">{emptyCopy(active)}</p>
                  <p className="text-xs text-[var(--muted)]">M0 骨架，功能在后续里程碑实现。</p>
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
    <div className="flex h-full flex-col items-center justify-center gap-3 text-center">
      <Inbox size={26} className="text-[var(--muted)]" />
      <p className="text-sm text-[var(--muted)]">{emptyCopy("overview")}</p>
      <Button variant="primary" size="sm" onPress={onRegister}>
        注册项目
      </Button>
    </div>
  );
}

function ProjectList({ projects }: { projects: Project[] }) {
  return (
    <div className="overflow-hidden rounded-lg border border-[var(--border)]">
      <div className="grid grid-cols-[1.2fr_1.6fr_100px_90px] gap-3 border-b border-[var(--border)] px-3 py-2 text-xs text-[var(--muted)]">
        <span>名称</span>
        <span>仓库</span>
        <span>分支</span>
        <span>状态</span>
      </div>
      {projects.map((project) => (
        <div
          key={project.id}
          className="grid grid-cols-[1.2fr_1.6fr_100px_90px] items-center gap-3 border-b border-[var(--border)] px-3 py-2 text-sm last:border-b-0"
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
    <Card className="mb-4 max-w-[720px]">
      <Card.Header>
        <Card.Title>注册项目</Card.Title>
        <Card.Description>
          本地仓库必须是 git 仓库，且 `.temli/` 已在 `.gitignore` 中忽略。
        </Card.Description>
      </Card.Header>
      <Card.Content className="grid gap-3">
        <label className="grid gap-1 text-xs text-[var(--muted)]">
          仓库路径
          <Input
            aria-label="仓库路径"
            placeholder="/Users/lhy/Project/Client/temli-demo"
            value={repoPath}
            onChange={(e) => setRepoPath(e.target.value)}
          />
        </label>
        <label className="grid gap-1 text-xs text-[var(--muted)]">
          名称（可选）
          <Input
            aria-label="名称"
            placeholder="temli-demo"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </label>
        <div className="grid grid-cols-2 gap-3">
          <label className="grid gap-1 text-xs text-[var(--muted)]">
            GitHub owner
            <Input
              aria-label="GitHub owner"
              placeholder="Lhy723"
              value={owner}
              onChange={(e) => setOwner(e.target.value)}
            />
          </label>
          <label className="grid gap-1 text-xs text-[var(--muted)]">
            GitHub repo
            <Input
              aria-label="GitHub repo"
              placeholder="temli-demo"
              value={repo}
              onChange={(e) => setRepo(e.target.value)}
            />
          </label>
        </div>
        <label className="grid gap-1 text-xs text-[var(--muted)]">
          默认分支
          <Input
            aria-label="默认分支"
            value={branch}
            onChange={(e) => setBranch(e.target.value)}
          />
        </label>
      </Card.Content>
      <Card.Footer className="flex justify-end gap-2">
        <Button variant="ghost" size="sm" onPress={onCancel} isDisabled={busy}>
          取消
        </Button>
        <Button variant="primary" size="sm" onPress={submit} isDisabled={busy || !repoPath}>
          {busy ? "注册中…" : "注册"}
        </Button>
      </Card.Footer>
    </Card>
  );
}
