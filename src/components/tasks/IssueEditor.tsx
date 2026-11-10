import { useState } from "react";
import { Button, Input, TextArea } from "@heroui/react";
import type { ProjectIssue, UpdateIssueInput } from "../../lib/api";

// 已发布 issue 的远端编辑入口：标题/正文、指派人与关闭/重开。
// 保存成功后由父级刷新列表，不在这里改本地 issues。
function IssueEditControls({
  issue,
  compact = false,
  onSave,
}: {
  issue: ProjectIssue;
  compact?: boolean;
  onSave: (input: UpdateIssueInput) => Promise<string | null>;
}) {
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState(issue.title);
  const [body, setBody] = useState("");
  const [assignee, setAssignee] = useState("");
  const [saving, setSaving] = useState<"fields" | "assignee" | "state" | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);

  const trimmedTitle = title.trim();
  const titleChanged = trimmedTitle !== issue.title;
  // 后端不返回 issue 正文，输入框留空表示只改其它字段，不覆盖远端正文。
  const bodyDirty = body.trim().length > 0;
  const closed = issue.github_state === "closed";
  const buttonClass = compact ? "h-7 px-2 text-label" : "h-8 px-3 text-ui";
  const fieldClass = compact ? "h-7 text-ui-sm" : "h-8 text-ui";
  const errorClass = compact ? "text-label" : "text-ui-sm";

  function openEditor() {
    setTitle(issue.title);
    setBody("");
    setAssignee("");
    setError(null);
    setEditing(true);
  }

  async function saveFields() {
    if (saving !== null) return;
    if (!trimmedTitle) {
      setError("标题不能为空。");
      return;
    }
    if (!titleChanged && !bodyDirty) return;
    setSaving("fields");
    setError(null);
    const input: UpdateIssueInput = { item_id: issue.id };
    if (titleChanged) input.title = trimmedTitle;
    if (bodyDirty) input.body = body;
    const message = await onSave(input);
    setSaving(null);
    if (message) {
      setError(message);
      return;
    }
    setEditing(false);
  }

  async function saveAssignee() {
    const login = assignee.trim();
    // 空输入不提交，避免把远端指派清空。
    if (!login || saving !== null) return;
    setSaving("assignee");
    setError(null);
    const message = await onSave({ item_id: issue.id, assignee: login });
    setSaving(null);
    if (message) {
      setError(message);
      return;
    }
    setAssignee("");
  }

  async function toggleState() {
    if (saving !== null) return;
    setSaving("state");
    setError(null);
    const message = await onSave({
      item_id: issue.id,
      state: closed ? "open" : "closed",
    });
    setSaving(null);
    if (message) setError(message);
  }

  return (
    <div className="grid gap-1">
      <div className="flex flex-wrap items-center gap-1">
        {!editing && (
          <Button
            variant="secondary"
            size="sm"
            className={buttonClass}
            onPress={openEditor}
            isDisabled={saving !== null}
          >
            编辑
          </Button>
        )}
        <Button
          variant="secondary"
          size="sm"
          className={buttonClass}
          onPress={() => void toggleState()}
          isDisabled={saving !== null}
        >
          {saving === "state"
            ? "处理中…"
            : closed
              ? "重开 issue"
              : "关闭 issue"}
        </Button>
      </div>
      {editing && (
        <div className="grid gap-1">
          <label className="grid gap-0.5 text-label text-[var(--muted)]">
            标题
            <Input
              aria-label="issue 标题"
              value={title}
              onChange={(event) => {
                setTitle(event.target.value);
                setError(null);
              }}
              disabled={saving !== null}
              fullWidth
              className={fieldClass}
            />
          </label>
          <label className="grid gap-0.5 text-label text-[var(--muted)]">
            正文
            <TextArea
              aria-label="issue 正文"
              rows={3}
              placeholder="留空则保持远端正文不变"
              value={body}
              onChange={(event) => {
                setBody(event.target.value);
                setError(null);
              }}
              disabled={saving !== null}
              fullWidth
              className={compact ? "text-ui-sm" : "text-ui"}
            />
          </label>
          <div className="flex justify-end gap-1">
            <Button
              variant="ghost"
              size="sm"
              className={buttonClass}
              onPress={() => {
                setEditing(false);
                setError(null);
              }}
              isDisabled={saving !== null}
            >
              取消
            </Button>
            <Button
              variant="secondary"
              size="sm"
              className={buttonClass}
              aria-label="保存标题与正文"
              onPress={() => void saveFields()}
              isDisabled={
                saving !== null || (!titleChanged && !bodyDirty)
              }
            >
              {saving === "fields" ? "保存中…" : "保存"}
            </Button>
          </div>
          <div className="flex items-end gap-1">
            <label className="grid min-w-0 flex-1 gap-0.5 text-label text-[var(--muted)]">
              指派人
              <Input
                aria-label="issue 指派人"
                placeholder="GitHub 登录名"
                value={assignee}
                onChange={(event) => {
                  setAssignee(event.target.value);
                  setError(null);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    void saveAssignee();
                  }
                }}
                disabled={saving !== null}
                fullWidth
                className={fieldClass}
              />
            </label>
            <Button
              variant="secondary"
              size="sm"
              className={`${buttonClass} shrink-0`}
              aria-label="保存指派人"
              onPress={() => void saveAssignee()}
              isDisabled={saving !== null || !assignee.trim()}
            >
              {saving === "assignee" ? "保存中…" : "保存"}
            </Button>
          </div>
        </div>
      )}
      {error && (
        <p role="alert" className={`text-[var(--danger)] ${errorClass}`}>
          {error}
        </p>
      )}
    </div>
  );
}


export { IssueEditControls };
