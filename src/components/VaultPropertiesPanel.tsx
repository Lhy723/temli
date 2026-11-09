import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Button } from "@heroui/react";
import { Plus, Save, SlidersHorizontal, X } from "lucide-react";
import { errorText } from "../lib/api";

type ParsedProperty =
  | { kind: "pair"; key: string; value: string }
  | { kind: "raw"; key: string; value: string };

type PropertyRow = ParsedProperty & { id: string };

/**
 * frontmatter 属性面板：解析 `---` 包裹的键值行，支持编辑/删除/新增。
 * 嵌套对象、数组、块标量等复杂值与非法键只读展示，保存时不提交以由后端原样保留。
 */
export function VaultPropertiesPanel({
  body,
  saving = false,
  onSave,
}: {
  body: string;
  saving?: boolean;
  onSave: (props: Record<string, string>) => Promise<void>;
}) {
  const parsed = useMemo(() => parseFrontmatter(body), [body]);
  const originalPairKeys = useMemo(
    () =>
      parsed.entries
        .filter((entry) => entry.kind === "pair")
        .map((entry) => entry.key),
    [parsed],
  );
  const idRef = useRef(0);
  const nextId = useCallback(() => {
    idRef.current += 1;
    return `property-${idRef.current}`;
  }, []);
  const [rows, setRows] = useState<PropertyRow[]>(() =>
    toRows(parsed.entries, nextId),
  );
  const signatureRef = useRef(parsed.signature);
  const [focusId, setFocusId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // 仅当 frontmatter 原文变化时重置编辑状态；正文其他位置输入不会打断编辑。
  useEffect(() => {
    if (signatureRef.current === parsed.signature) return;
    signatureRef.current = parsed.signature;
    setRows(toRows(parsed.entries, nextId));
    setFocusId(null);
    setError(null);
  }, [parsed, nextId]);

  function updatePair(
    id: string,
    field: "key" | "value",
    value: string,
  ): void {
    setRows((prev) =>
      prev.map((row) => {
        if (row.id !== id || row.kind !== "pair") return row;
        return field === "key" ? { ...row, key: value } : { ...row, value };
      }),
    );
  }

  function removeRow(id: string): void {
    setRows((prev) => prev.filter((row) => row.id !== id));
  }

  function addRow(): void {
    const id = nextId();
    setRows((prev) => [...prev, { id, kind: "pair", key: "", value: "" }]);
    setFocusId(id);
  }

  async function save(): Promise<void> {
    if (saving) return;
    const props: Record<string, string> = {};
    const present = new Set<string>();

    for (const row of rows) {
      if (row.kind !== "pair") continue;
      const key = row.key.trim();
      if (!key) continue;
      if (!/^[A-Za-z0-9_-]+$/.test(key)) {
        setError(`属性名不合法：${key}（仅支持字母、数字、_ 和 -）`);
        return;
      }
      present.add(key);
      props[key] = row.value.trim();
    }

    // 后端按「空值删除、未出现的键保留」合并 frontmatter；
    // 被删除的原键显式传空串，复杂值不传入以保持原样。
    for (const key of originalPairKeys) {
      if (!present.has(key)) props[key] = "";
    }

    setError(null);
    try {
      await onSave(props);
    } catch (reason) {
      setError(errorText(reason));
    }
  }

  return (
    <section className="flex min-h-0 flex-col" aria-label="属性面板">
      <header className="flex h-9 shrink-0 items-center gap-1.5 border-b border-[var(--border)] px-2.5 text-ui-sm text-[var(--muted)]">
        <SlidersHorizontal size={13} className="shrink-0" />
        <span>属性</span>
        <span className="ml-auto text-label">{rows.length}</span>
      </header>

      <div className="flex min-h-0 flex-1 flex-col gap-1.5 overflow-auto p-2">
        {rows.length === 0 && (
          <div className="flex flex-col items-center gap-1.5 px-2 py-5 text-center">
            <SlidersHorizontal
              size={16}
              className="text-[var(--muted)] opacity-70"
            />
            <p className="text-ui-sm text-[var(--muted)]">还没有属性</p>
            <p className="text-label text-[var(--muted)] opacity-70">
              添加键值对后写入笔记 frontmatter。
            </p>
          </div>
        )}

        {rows.map((row) =>
          row.kind === "pair" ? (
            <div
              key={row.id}
              className="rounded-[10px] border border-[var(--border)] bg-[var(--background-secondary)] p-1.5"
            >
              <div className="flex items-center gap-1">
                <input
                  aria-label="属性键"
                  placeholder="键"
                  value={row.key}
                  onChange={(event) =>
                    updatePair(row.id, "key", event.target.value)
                  }
                  disabled={saving}
                  autoFocus={focusId === row.id}
                  spellCheck={false}
                  className="h-6 min-w-0 flex-1 rounded bg-transparent px-1 text-ui-sm font-medium text-[var(--foreground)] outline-none placeholder:text-[var(--muted)] focus-visible:bg-[var(--background)] disabled:cursor-not-allowed disabled:opacity-50"
                />
                <button
                  type="button"
                  aria-label={`删除属性 ${row.key || "未命名"}`}
                  title="删除属性"
                  onClick={() => removeRow(row.id)}
                  disabled={saving}
                  className="anim-fast flex size-5 shrink-0 items-center justify-center rounded text-[var(--muted)] hover:bg-[var(--background-tertiary)] hover:text-[var(--danger)] disabled:cursor-not-allowed disabled:opacity-50"
                >
                  <X size={12} />
                </button>
              </div>
              <input
                aria-label={`${row.key || "属性"}的值`}
                placeholder="值"
                value={row.value}
                onChange={(event) =>
                  updatePair(row.id, "value", event.target.value)
                }
                disabled={saving}
                spellCheck={false}
                className="mt-1 h-7 w-full rounded-lg border border-[var(--border)] bg-[var(--background)] px-1.5 font-mono text-ui-sm text-[var(--foreground)] outline-none placeholder:font-sans placeholder:text-[var(--muted)] focus-visible:border-[var(--accent)] disabled:cursor-not-allowed disabled:opacity-50"
              />
            </div>
          ) : (
            <div
              key={row.id}
              className="rounded-[10px] border border-dashed border-[var(--border)] bg-[var(--background-secondary)] p-1.5"
            >
              <div className="flex items-center gap-1 px-1">
                <span className="min-w-0 flex-1 truncate text-ui-sm font-medium text-[var(--foreground)]">
                  {row.key}
                </span>
                <span className="shrink-0 text-label text-[var(--muted)]">
                  只读
                </span>
              </div>
              <pre className="mt-1 max-h-32 overflow-auto whitespace-pre-wrap break-words rounded-lg bg-[var(--background)] p-1.5 font-mono text-label text-[var(--muted)]">
                {row.value}
              </pre>
            </div>
          ),
        )}
      </div>

      <div className="flex shrink-0 flex-col gap-1.5 border-t border-[var(--border)] p-2">
        {error && (
          <p role="alert" className="text-ui-sm text-[var(--danger)]">
            {error}
          </p>
        )}
        <button
          type="button"
          onClick={addRow}
          disabled={saving}
          className="anim-fast flex h-7 w-full items-center justify-center gap-1 rounded-lg border border-dashed border-[var(--border)] text-ui-sm text-[var(--muted)] hover:bg-[var(--background-secondary)] hover:text-[var(--foreground)] disabled:cursor-not-allowed disabled:opacity-50"
        >
          <Plus size={13} />
          新增属性
        </button>
        <Button
          variant="primary"
          size="sm"
          className="h-8 w-full gap-1.5 text-ui-sm"
          onPress={() => void save()}
          isDisabled={saving}
        >
          <Save size={13} />
          {saving ? "保存中…" : "保存属性"}
        </Button>
      </div>
    </section>
  );
}

function parseFrontmatter(body: string): {
  entries: ParsedProperty[];
  signature: string;
} {
  const source = body.startsWith("\uFEFF") ? body.slice(1) : body;
  const lines = source.split(/\r?\n/);
  if (lines.length < 2 || !/^---[ \t]*$/.test(lines[0])) {
    return { entries: [], signature: "" };
  }

  let end = -1;
  for (let index = 1; index < lines.length; index += 1) {
    if (/^---[ \t]*$/.test(lines[index])) {
      end = index;
      break;
    }
  }
  if (end === -1) return { entries: [], signature: "" };

  const signature = lines.slice(0, end + 1).join("\n");
  const entries: ParsedProperty[] = [];
  let index = 1;

  while (index < end) {
    const line = lines[index];
    if (!line.trim()) {
      index += 1;
      continue;
    }
    // 不属于任何键的续行（非法 YAML），跳过。
    if (isContinuationLine(line)) {
      index += 1;
      continue;
    }

    const match = /^([^\s:#][^:]*?):(?:[ \t]+(.*))?$/.exec(line);
    if (!match) {
      index += 1;
      continue;
    }

    const key = match[1].trim();
    const firstValue = (match[2] ?? "").replace(/[ \t]+$/, "");
    const continuation: string[] = [];
    let cursor = index + 1;

    while (cursor < end) {
      const next = lines[cursor];
      if (!next.trim()) {
        let lookahead = cursor + 1;
        while (lookahead < end && !lines[lookahead].trim()) {
          lookahead += 1;
        }
        if (lookahead < end && isContinuationLine(lines[lookahead])) {
          continuation.push(next);
          cursor += 1;
          continue;
        }
        break;
      }
      if (!isContinuationLine(next)) break;
      continuation.push(next);
      cursor += 1;
    }

    const trimmedValue = firstValue.trim();
    const isComplex =
      continuation.length > 0 || /^[\[{|>]/.test(trimmedValue);
    const value = unquoteScalar(trimmedValue);
    // 保存时会 trim 且空值代表删除，仅空白内容的引号串保持只读以免误删。
    const isQuotedEmpty = trimmedValue !== "" && value.trim() === "";
    // 非法键会被后端拒绝整次保存，只读展示且不参与保存/删除。
    const isRaw =
      isComplex || isQuotedEmpty || !/^[A-Za-z0-9_-]+$/.test(key);

    if (isRaw) {
      entries.push({
        kind: "raw",
        key,
        value: [firstValue, ...continuation].join("\n"),
      });
    } else {
      entries.push({ kind: "pair", key, value });
    }

    index = cursor;
  }

  return { entries, signature };
}

/** 去掉 YAML 标量外层引号；无法安全解析时返回原串。 */
function unquoteScalar(value: string): string {
  if (value.length < 2) return value;
  const first = value[0];
  const last = value[value.length - 1];

  if (first === '"' && last === '"') {
    try {
      const parsed: unknown = JSON.parse(value);
      if (typeof parsed === "string") return parsed;
    } catch {
      // YAML 双引号转义并非 JSON 子集时保持原样，避免误改内容。
    }
    return value;
  }

  if (first === "'" && last === "'") {
    // YAML 单引号串仅支持用 '' 表示一个单引号。
    return value.slice(1, -1).replace(/''/g, "'");
  }

  return value;
}

function isContinuationLine(line: string): boolean {
  return /^\s/.test(line) || /^-(\s|$)/.test(line);
}

function toRows(
  entries: ParsedProperty[],
  nextId: () => string,
): PropertyRow[] {
  return entries.map((entry) => {
    const id = nextId();
    return entry.kind === "pair"
      ? { kind: "pair", key: entry.key, value: entry.value, id }
      : {
          kind: "raw",
          key: entry.key,
          value: entry.value,
          id,
        };
  });
}
