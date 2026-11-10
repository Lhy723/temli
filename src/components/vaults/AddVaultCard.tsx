import { Button, Card, Input } from "@heroui/react";

export function AddVaultCard({
  name,
  onNameChange,
  root,
  onRootChange,
  busy,
  error,
  onSubmit,
  onCancel,
}: {
  name: string;
  onNameChange: (name: string) => void;
  root: string;
  onRootChange: (root: string) => void;
  busy: boolean;
  error: string | null;
  onSubmit: () => void;
  onCancel: () => void;
}) {
  return (
    <Card className="max-w-[680px] shrink-0">
      <Card.Header>
        <Card.Title className="text-ui font-medium">添加笔记库</Card.Title>
        <Card.Description className="text-ui-sm">
          笔记以目录下的 Markdown 文件为准，与应用内的项目互相独立；目录不存在时会自动创建。
        </Card.Description>
      </Card.Header>
      <Card.Content className="grid gap-2">
        <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
          名称
          <Input
            aria-label="笔记库名称"
            placeholder="个人知识库"
            value={name}
            onChange={(e) => onNameChange(e.target.value)}
            disabled={busy}
          />
        </label>
        <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
          目录
          <Input
            aria-label="笔记库目录"
            placeholder="/Users/lhy/Notes"
            value={root}
            onChange={(e) => onRootChange(e.target.value)}
            disabled={busy}
            onKeyDown={(e) => {
              if (e.key === "Enter") onSubmit();
            }}
          />
        </label>
        {error && (
          <p role="alert" className="text-ui-sm text-[var(--danger)]">
            {error}
          </p>
        )}
      </Card.Content>
      <Card.Footer className="flex justify-end gap-1.5">
        <Button
          variant="ghost"
          size="sm"
          className="h-7 px-2.5 text-ui"
          onPress={onCancel}
          isDisabled={busy}
        >
          取消
        </Button>
        <Button
          variant="primary"
          size="sm"
          className="h-7 px-2.5 text-ui"
          onPress={onSubmit}
          isDisabled={busy || !root.trim()}
        >
          {busy ? "添加中…" : "添加"}
        </Button>
      </Card.Footer>
    </Card>
  );
}
