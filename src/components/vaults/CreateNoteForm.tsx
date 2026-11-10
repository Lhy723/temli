import { Button, Input, ListBox, Select } from "@heroui/react";
import { X } from "lucide-react";
import type { VaultEntry } from "../../lib/api";
import { noteTitle } from "./shared";

export function CreateNoteForm({
  path,
  onPathChange,
  error,
  creating,
  onSubmit,
  onCancel,
  templates,
  templatePath,
  onTemplatePathChange,
}: {
  path: string;
  onPathChange: (path: string) => void;
  error: string | null;
  creating: boolean;
  onSubmit: () => void;
  onCancel: () => void;
  templates: VaultEntry[];
  templatePath: string;
  onTemplatePathChange: (path: string) => void;
}) {
  return (
    <div className="grid gap-1">
      <div className="flex items-center gap-1">
        <Input
          aria-label="新笔记路径"
          placeholder="文件夹/笔记.md"
          value={path}
          onChange={(e) => onPathChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") onSubmit();
          }}
          className="min-w-0 flex-1"
          disabled={creating}
          autoFocus
        />
        <Button
          variant="primary"
          size="sm"
          className="h-7 shrink-0 px-2 text-ui"
          onPress={onSubmit}
          isDisabled={creating}
        >
          {creating ? "创建中…" : "创建"}
        </Button>
        <Button
          isIconOnly
          variant="ghost"
          size="sm"
          className="h-7 w-7 shrink-0"
          aria-label="取消新建"
          onPress={onCancel}
          isDisabled={creating}
        >
          <X size={14} />
        </Button>
      </div>
      <div className="grid gap-1">
        <span className="text-label text-[var(--muted)]">
          模板（可选）
        </span>
        <Select
          aria-label="新笔记模板"
          className="w-full"
          placeholder="不使用模板"
          selectedKey={templatePath || null}
          onSelectionChange={(key) =>
            onTemplatePathChange(
              key == null || key === "__none__" ? "" : String(key),
            )
          }
          isDisabled={creating || templates.length === 0}
        >
          <Select.Trigger className="h-7 text-ui">
            <Select.Value />
            <Select.Indicator />
          </Select.Trigger>
          <Select.Popover>
            <ListBox>
              <ListBox.Item id="__none__" textValue="不使用模板">
                不使用模板
              </ListBox.Item>
              {templates.map((template) => (
                <ListBox.Item
                  key={template.path}
                  id={template.path}
                  textValue={template.title || noteTitle(template.path)}
                >
                  {template.title || noteTitle(template.path)}
                </ListBox.Item>
              ))}
            </ListBox>
          </Select.Popover>
        </Select>
      </div>
      {error && (
        <p role="alert" className="text-ui-sm text-[var(--danger)]">
          {error}
        </p>
      )}
    </div>
  );
}
