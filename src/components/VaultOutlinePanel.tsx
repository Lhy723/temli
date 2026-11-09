import { useMemo } from "react";
import { ListTree } from "lucide-react";

type OutlineHeading = {
  line: number;
  level: number;
  text: string;
};

/**
 * 大纲面板：解析 ATX 标题（忽略 frontmatter 与代码块），点击回调 1-based 行号。
 */
export function VaultOutlinePanel({
  body,
  onJump,
}: {
  body: string;
  onJump: (line: number) => void;
}) {
  const headings = useMemo(() => parseHeadings(body), [body]);

  return (
    <section className="flex min-h-0 flex-col" aria-label="大纲面板">
      <header className="flex h-9 shrink-0 items-center gap-1.5 border-b border-[var(--border)] px-2.5 text-ui-sm text-[var(--muted)]">
        <ListTree size={13} className="shrink-0" />
        <span>大纲</span>
        <span className="ml-auto text-label">{headings.length}</span>
      </header>

      <div className="min-h-0 flex-1 overflow-auto py-1">
        {headings.length === 0 ? (
          <div className="flex flex-col items-center gap-1.5 px-2 py-6 text-center">
            <ListTree size={16} className="text-[var(--muted)] opacity-70" />
            <p className="text-ui-sm text-[var(--muted)]">还没有标题</p>
            <p className="text-label text-[var(--muted)] opacity-70">
              用 # 标题 组织笔记，点击即可跳转。
            </p>
          </div>
        ) : (
          headings.map((heading) => (
            <button
              key={heading.line}
              type="button"
              onClick={() => onJump(heading.line)}
              title={`第 ${heading.line} 行 · ${heading.text}`}
              aria-label={`跳转到第 ${heading.line} 行：${heading.text}`}
              className={`anim-fast flex min-h-7 w-full items-center rounded px-2 py-1 text-left hover:bg-[var(--background-secondary)] ${
                heading.level === 1
                  ? "text-ui font-semibold text-[var(--foreground)]"
                  : heading.level === 2
                    ? "text-ui font-medium text-[var(--foreground)]"
                    : heading.level <= 4
                      ? "text-ui-sm text-[var(--foreground)]"
                      : "text-ui-sm text-[var(--muted)]"
              }`}
              style={{ paddingLeft: 8 + (heading.level - 1) * 12 }}
            >
              <span className="truncate">{heading.text}</span>
            </button>
          ))
        )}
      </div>
    </section>
  );
}

function parseHeadings(body: string): OutlineHeading[] {
  const headings: OutlineHeading[] = [];
  const source = body.startsWith("\uFEFF") ? body.slice(1) : body;
  const lines = source.split(/\r?\n/);
  let fence: { char: string; length: number } | null = null;

  // 跳过 frontmatter（首个 `---` 到下一个 `---`），避免其中的 `#` 被当作标题。
  let start = 0;
  if (lines[0]?.trim() === "---") {
    for (let index = 1; index < lines.length; index += 1) {
      if (lines[index].trim() === "---") {
        start = index + 1;
        break;
      }
    }
  }

  lines.forEach((line, index) => {
    if (index < start) return;

    const fenceMatch = /^ {0,3}(`{3,}|~{3,})/.exec(line);

    if (fence) {
      if (
        fenceMatch &&
        fenceMatch[1][0] === fence.char &&
        fenceMatch[1].length >= fence.length &&
        line.slice(fenceMatch[0].length).trim() === ""
      ) {
        fence = null;
      }
      return;
    }

    if (fenceMatch) {
      fence = { char: fenceMatch[1][0], length: fenceMatch[1].length };
      return;
    }

    const match = /^ {0,3}(#{1,6})(?:[ \t]+(.*?))?[ \t]*$/.exec(line);
    if (!match) return;

    const text = (match[2] ?? "")
      .replace(/[ \t]+#+[ \t]*$/, "")
      .trim();

    headings.push({
      line: index + 1,
      level: match[1].length,
      text: text || "（空标题）",
    });
  });

  return headings;
}
