import { useState } from "react";
import { Button, Card, Input, TextArea } from "@heroui/react";
import { createTask, errorText, type TaskDetail } from "../../lib/api";

function CreateTaskForm({
  projectId,
  onCancel,
  onCreated,
}: {
  projectId: string;
  onCancel: () => void;
  onCreated: (detail: TaskDetail) => Promise<void> | void;
}) {
  const [title, setTitle] = useState("");
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit() {
    const trimmedTitle = title.trim();
    const trimmedPrompt = prompt.trim();
    if (!trimmedTitle || !trimmedPrompt) return;
    setBusy(true);
    setError(null);
    try {
      const created = await createTask({
        project_id: projectId,
        title: trimmedTitle,
        prompt: trimmedPrompt,
      });
      setTitle("");
      setPrompt("");
      await onCreated(created);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card className="mb-3 max-w-[720px] rounded-[10px] p-3.5">
      <Card.Header>
        <Card.Title className="text-[15px] font-medium leading-[1.4]">
          新建任务
        </Card.Title>
        <Card.Description className="text-ui-sm">
          描述目标与约束，验收标准由规划 Agent 生成。
        </Card.Description>
      </Card.Header>
      <Card.Content className="grid gap-2">
        <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
          标题
          <Input
            aria-label="任务标题"
            placeholder="例如：为登录页补充失败态测试"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            className="h-8 text-ui"
          />
        </label>
        <label className="text-ui-sm grid gap-1 text-[var(--muted)]">
          意图
          <TextArea
            aria-label="任务意图"
            rows={3}
            placeholder="说明要做什么、为什么，以及需要注意的约束。"
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            className="text-ui"
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
          isDisabled={busy || !title.trim() || !prompt.trim()}
        >
          {busy ? "创建中…" : "创建"}
        </Button>
      </Card.Footer>
    </Card>
  );
}


export { CreateTaskForm };
