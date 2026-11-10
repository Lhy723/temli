import { useEffect, useState } from "react";
import { Button, Chip, Input, ListBox, Select } from "@heroui/react";
import { CircleStop, Play, X } from "lucide-react";
import {
  formatDate,
  statusLabel,
  statusTone,
  type AgentManifest,
  type TaskDetail,
} from "../../../lib/api";
import {
  ADAPTER_DEFAULT_KEY,
  ADAPTER_FOLLOW_KEY,
  AgentRoleTags,
  adapterLabel,
  adapterRoles,
  taskAgentInfo,
} from "../shared";

export function DetailHeader({
  detail,
  adapters,
  adaptersLoading,
  adaptersError,
  onSaveAgents,
  onSaveModel,
  busy,
  cancelling,
  onCancel,
  onStart,
  onClose,
}: {
  detail: TaskDetail | null;
  adapters: AgentManifest[];
  adaptersLoading: boolean;
  adaptersError: string | null;
  onSaveAgents: (
    developer: string | null,
    reviewer: string | null,
  ) => Promise<string | null>;
  onSaveModel: (model: string | null) => Promise<string | null>;
  busy: boolean;
  cancelling: boolean;
  onCancel: () => void;
  onStart: () => void;
  onClose: () => void;
}) {
  const detailTaskId = detail?.task.id ?? null;
  const savedDeveloper = detail ? taskAgentInfo(detail.task).developer : null;
  const savedReviewer = detail ? taskAgentInfo(detail.task).reviewer : null;
  const savedModel = detail ? (detail.task.model ?? null) : null;
  const [developerKey, setDeveloperKey] = useState<string | null>(
    savedDeveloper,
  );
  const [reviewerKey, setReviewerKey] = useState<string | null>(
    savedReviewer ?? savedDeveloper,
  );
  const [reviewerFollows, setReviewerFollows] = useState(
    savedReviewer === null,
  );
  const [agentsSaving, setAgentsSaving] = useState(false);
  const [agentsSaveError, setAgentsSaveError] = useState<string | null>(null);
  const [modelInput, setModelInput] = useState(savedModel ?? "");
  const [modelSaving, setModelSaving] = useState(false);
  const [modelSaveError, setModelSaveError] = useState<string | null>(null);
  useEffect(() => {
    setDeveloperKey(savedDeveloper);
    setReviewerKey(savedReviewer ?? savedDeveloper);
    setReviewerFollows(savedReviewer === null);
    setAgentsSaveError(null);
    setModelInput(savedModel ?? "");
    setModelSaveError(null);
  }, [detailTaskId, savedDeveloper, savedReviewer, savedModel]);

  const status = detail?.task.status;
  const canCancel = status === "queued" || status === "running";
  const startDisabled =
    busy ||
    cancelling ||
    !detail ||
    detail.task.status === "running" ||
    detail.task.status === "verifying" ||
    detail.task.status === "merged";
  const agentsDirty =
    detail !== null &&
    (developerKey !== savedDeveloper ||
      (reviewerFollows
        ? savedReviewer !== null
        : reviewerKey !== savedReviewer));
  const knownAdapterIds = new Set(adapters.map((adapter) => adapter.id));
  const adapterFallbackLabel = adaptersLoading
    ? "加载中"
    : adaptersError
      ? "未校验"
      : "未注册";
  const unregisteredAdapterIds = Array.from(
    new Set(
      [developerKey, reviewerKey].filter(
        (id): id is string => id !== null && !knownAdapterIds.has(id),
      ),
    ),
  );
  // 审查下拉优先只列声明了 reviewer 角色的适配器；一个都没有时退回完整列表。
  const reviewerCapable = adapters.filter((adapter) =>
    adapterRoles(adapter).includes("reviewer"),
  );
  const reviewerCandidates =
    reviewerCapable.length > 0 ? reviewerCapable : adapters;
  const reviewerCandidateIds = new Set(
    reviewerCandidates.map((adapter) => adapter.id),
  );
  // 已保存/当前选择的适配器即使不支持审查也要可选，便于看清并改回。
  const retainedReviewerAdapters = adapters.filter(
    (adapter) =>
      !reviewerCandidateIds.has(adapter.id) &&
      ((!reviewerFollows && adapter.id === reviewerKey) ||
        adapter.id === savedReviewer),
  );
  const reviewerAdapterOptions = [
    ...reviewerCandidates,
    ...retainedReviewerAdapters,
  ];
  const normalizedModel = modelInput.trim() === "" ? null : modelInput.trim();
  const normalizedSavedModel =
    savedModel && savedModel.trim() ? savedModel.trim() : null;
  const modelDirty =
    detail !== null && normalizedModel !== normalizedSavedModel;

  async function submitAgents() {
    if (!detail || agentsSaving || !agentsDirty) return;
    setAgentsSaving(true);
    setAgentsSaveError(null);
    const failure = await onSaveAgents(
      developerKey,
      reviewerFollows ? null : reviewerKey,
    );
    if (failure) setAgentsSaveError(failure);
    setAgentsSaving(false);
  }

  async function submitModel() {
    if (!detail || modelSaving || !modelDirty) return;
    setModelSaving(true);
    setModelSaveError(null);
    const failure = await onSaveModel(normalizedModel);
    if (failure) setModelSaveError(failure);
    setModelSaving(false);
  }

  return (
      <header className="shrink-0 border-b border-[var(--border)] px-3.5 py-3">
        <div className="flex items-start gap-2">
          <div className="min-w-0 flex-1">
            <p className="truncate text-ui font-medium">
              {detail ? detail.task.title : "任务详情"}
            </p>
            {detail && (
              <div className="mt-1 flex items-center gap-1.5 text-ui-sm text-[var(--muted)]">
                <Chip
                  size="sm"
                  variant="soft"
                  color={statusTone(detail.task.status)}
                >
                  {statusLabel(detail.task.status)}
                </Chip>
                <span>尝试 {detail.task.attempts}</span>
                <span>{formatDate(detail.task.created_at)}</span>
              </div>
            )}
          </div>
          {detail && canCancel && (
            <Button
              variant="danger-soft"
              size="sm"
              className="h-7 shrink-0 gap-1 px-2 text-ui"
              onPress={onCancel}
              isDisabled={cancelling}
            >
              <CircleStop size={14} />
              {cancelling ? "取消中…" : "取消"}
            </Button>
          )}
          {detail && (
            <Button
              variant="primary"
              size="sm"
              className="h-8 shrink-0 gap-1 px-3 text-ui"
              onPress={onStart}
              isDisabled={startDisabled}
            >
              <Play size={14} />
              {busy ? "启动中…" : "开始执行"}
            </Button>
          )}
          <Button
            isIconOnly
            variant="ghost"
            size="sm"
            className="h-7 w-7 shrink-0 rounded-lg"
            aria-label="关闭详情"
            onPress={onClose}
          >
            <X size={14} />
          </Button>
        </div>
        {detail && (
          <div className="mt-1.5 grid gap-1">
            <div className="flex items-end gap-1.5">
              <div className="grid min-w-0 flex-1 gap-0.5">
                <span className="text-label text-[var(--muted)]">开发</span>
                <Select
                  aria-label="开发适配器"
                  className="w-full min-w-0"
                  placeholder={adaptersLoading ? "加载中…" : "默认适配器"}
                  selectedKey={developerKey ?? ADAPTER_DEFAULT_KEY}
                  onSelectionChange={(key) => {
                    const next =
                      key == null || String(key) === ADAPTER_DEFAULT_KEY
                        ? null
                        : String(key);
                    if (reviewerFollows) setReviewerKey(next);
                    setDeveloperKey(next);
                    setAgentsSaveError(null);
                  }}
                  isDisabled={agentsSaving}
                >
                  <Select.Trigger className="h-7 text-ui">
                    <Select.Value />
                    <Select.Indicator />
                  </Select.Trigger>
                  <Select.Popover>
                    <ListBox>
                      <ListBox.Item
                        id={ADAPTER_DEFAULT_KEY}
                        textValue="默认适配器"
                      >
                        <span className="flex min-w-0 flex-1 items-center gap-2">
                          <span className="min-w-0 flex-1 truncate">
                            默认适配器
                          </span>
                          <ListBox.ItemIndicator />
                        </span>
                      </ListBox.Item>
                      {adapters.map((adapter) => (
                        <ListBox.Item
                          key={adapter.id}
                          id={adapter.id}
                          textValue={`${adapter.name} (${adapter.id})`}
                        >
                          <span className="flex min-w-0 flex-1 items-center gap-2">
                            <span className="min-w-0 flex-1 truncate">
                              {adapter.name}
                            </span>
                            <AgentRoleTags adapter={adapter} />
                            <span className="shrink-0 font-mono text-label text-[var(--muted)]">
                              {adapter.id}
                            </span>
                            <ListBox.ItemIndicator />
                          </span>
                        </ListBox.Item>
                      ))}
                      {unregisteredAdapterIds.map((id) => (
                        <ListBox.Item
                          key={`missing-${id}`}
                          id={id}
                          textValue={`${id}（${adapterFallbackLabel}）`}
                        >
                          <span className="flex min-w-0 flex-1 items-center gap-2">
                            <span className="min-w-0 flex-1 truncate font-mono">
                              {id}
                            </span>
                            <Chip
                              size="sm"
                              variant="soft"
                              color={
                                adaptersLoading || adaptersError
                                  ? "default"
                                  : "warning"
                              }
                              className="shrink-0"
                            >
                              {adapterFallbackLabel}
                            </Chip>
                            <ListBox.ItemIndicator />
                          </span>
                        </ListBox.Item>
                      ))}
                    </ListBox>
                  </Select.Popover>
                </Select>
              </div>
              <div className="grid min-w-0 flex-1 gap-0.5">
                <span className="text-label text-[var(--muted)]">审查</span>
                <Select
                  aria-label="审查适配器"
                  className="w-full min-w-0"
                  placeholder={adaptersLoading ? "加载中…" : "跟随开发者"}
                  selectedKey={
                    reviewerFollows ? ADAPTER_FOLLOW_KEY : reviewerKey
                  }
                  onSelectionChange={(key) => {
                    if (key == null) return;
                    const raw = String(key);
                    const following = raw === ADAPTER_FOLLOW_KEY;
                    setReviewerFollows(following);
                    setReviewerKey(following ? developerKey : raw);
                    setAgentsSaveError(null);
                  }}
                  isDisabled={agentsSaving}
                >
                  <Select.Trigger className="h-7 text-ui">
                    <Select.Value />
                    <Select.Indicator />
                  </Select.Trigger>
                  <Select.Popover>
                    <ListBox>
                      <ListBox.Item
                        id={ADAPTER_FOLLOW_KEY}
                        textValue={`跟随开发者（${adapterLabel(
                          adapters,
                          developerKey,
                        )}）`}
                      >
                        <span className="flex min-w-0 flex-1 items-center gap-2">
                          <span className="min-w-0 flex-1 truncate">
                            跟随开发者
                          </span>
                          <span className="shrink-0 truncate text-label text-[var(--muted)]">
                            {adapterLabel(adapters, developerKey)}
                          </span>
                          <ListBox.ItemIndicator />
                        </span>
                      </ListBox.Item>
                      {reviewerAdapterOptions.map((adapter) => (
                        <ListBox.Item
                          key={adapter.id}
                          id={adapter.id}
                          textValue={`${adapter.name} (${adapter.id})`}
                        >
                          <span className="flex min-w-0 flex-1 items-center gap-2">
                            <span className="min-w-0 flex-1 truncate">
                              {adapter.name}
                            </span>
                            <AgentRoleTags adapter={adapter} />
                            <span className="shrink-0 font-mono text-label text-[var(--muted)]">
                              {adapter.id}
                            </span>
                            {!reviewerCandidateIds.has(adapter.id) && (
                              <Chip
                                size="sm"
                                variant="soft"
                                color="warning"
                                className="shrink-0"
                              >
                                不支持审查
                              </Chip>
                            )}
                            <ListBox.ItemIndicator />
                          </span>
                        </ListBox.Item>
                      ))}
                      {unregisteredAdapterIds.map((id) => (
                        <ListBox.Item
                          key={`missing-${id}`}
                          id={id}
                          textValue={`${id}（${adapterFallbackLabel}）`}
                        >
                          <span className="flex min-w-0 flex-1 items-center gap-2">
                            <span className="min-w-0 flex-1 truncate font-mono">
                              {id}
                            </span>
                            <Chip
                              size="sm"
                              variant="soft"
                              color={
                                adaptersLoading || adaptersError
                                  ? "default"
                                  : "warning"
                              }
                              className="shrink-0"
                            >
                              {adapterFallbackLabel}
                            </Chip>
                            <ListBox.ItemIndicator />
                          </span>
                        </ListBox.Item>
                      ))}
                    </ListBox>
                  </Select.Popover>
                </Select>
              </div>
              <Button
                variant="secondary"
                size="sm"
                className="h-7 shrink-0 px-2.5 text-ui"
                onPress={() => void submitAgents()}
                isDisabled={!agentsDirty || agentsSaving}
              >
                {agentsSaving ? "保存中…" : "保存"}
              </Button>
            </div>
            <div className="flex items-end gap-1.5">
              <div className="grid min-w-0 flex-1 gap-0.5">
                <span className="text-label text-[var(--muted)]">模型</span>
                <Input
                  aria-label="任务模型"
                  placeholder="默认模型"
                  value={modelInput}
                  onChange={(event) => {
                    setModelInput(event.target.value);
                    setModelSaveError(null);
                  }}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      event.preventDefault();
                      void submitModel();
                    }
                  }}
                  disabled={modelSaving}
                  fullWidth
                  className="h-7 text-ui"
                />
              </div>
              <Button
                variant="secondary"
                size="sm"
                className="h-7 shrink-0 px-2.5 text-ui"
                aria-label="保存模型"
                onPress={() => void submitModel()}
                isDisabled={!modelDirty || modelSaving}
              >
                {modelSaving ? "保存中…" : "保存"}
              </Button>
            </div>
            {adaptersError && (
              <p role="alert" className="text-ui-sm text-[var(--danger)]">
                适配器列表加载失败：{adaptersError}
              </p>
            )}
            {agentsSaveError && (
              <p role="alert" className="text-ui-sm text-[var(--danger)]">
                {agentsSaveError}
              </p>
            )}
            {modelSaveError && (
              <p role="alert" className="text-ui-sm text-[var(--danger)]">
                {modelSaveError}
              </p>
            )}
          </div>
        )}
      </header>
  );
}
