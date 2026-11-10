import { Button, Chip, ListBox, Select } from "@heroui/react";
import { FileText, X } from "lucide-react";
import type { ContextItem } from "../../../lib/api";
import { Muted, Section, type ContextOption } from "../shared";

export function ContextSection({
  contextItems,
  contextError,
  contextOptions,
  contextOptionsLoading,
  contextOptionsError,
  linkingContext,
  removingContextId,
  contextKey,
  setContextKey,
  onLinkContext,
  onUnlinkContext,
}: {
  contextItems: ContextItem[] | null;
  contextError: string | null;
  contextOptions: ContextOption[];
  contextOptionsLoading: boolean;
  contextOptionsError: string | null;
  linkingContext: boolean;
  removingContextId: string | null;
  contextKey: string | null;
  setContextKey: (key: string | null) => void;
  onLinkContext: (itemId: string) => Promise<boolean>;
  onUnlinkContext: (itemId: string) => void;
}) {
  const linkedContextKeys = new Set(
    (contextItems ?? []).map((item) => `${item.kind}:${item.id}`),
  );
  const availableContextOptions = contextOptions.filter(
    (option) => !linkedContextKeys.has(`${option.kind}:${option.id}`),
  );

  async function submitContext() {
    if (!contextKey || linkingContext) return;
    const linked = await onLinkContext(contextKey);
    if (linked) setContextKey(null);
  }

  return (
            <Section
              title="上下文"
              icon={<FileText size={14} />}
              count={contextItems?.length ?? 0}
            >
              {contextError && (
                <p role="alert" className="text-ui-sm text-[var(--danger)]">
                  {contextError}
                </p>
              )}
              {contextItems === null ? (
                <Muted>
                  {contextError ? "无法加载上下文。" : "加载中…"}
                </Muted>
              ) : (
                <div className="grid gap-2">
                  {contextItems.length === 0 ? (
                    <Muted>没有关联的笔记或文档。</Muted>
                  ) : (
                    <div className="overflow-hidden rounded-[10px] border border-[var(--border)]">
                      {contextItems.map((item) => (
                        <div
                          key={`${item.kind}:${item.id}`}
                          className="flex items-center gap-2 border-b border-[var(--border)] px-2 py-1.5 last:border-b-0"
                        >
                          <Chip
                            size="sm"
                            variant="tertiary"
                            color="default"
                            className="shrink-0"
                          >
                            {item.kind === "doc"
                              ? "文档"
                              : item.kind === "note"
                                ? "笔记"
                                : item.kind}
                          </Chip>
                          <span
                            className="min-w-0 flex-1 truncate"
                            title={item.title}
                          >
                            {item.title}
                          </span>
                          <Button
                            isIconOnly
                            variant="ghost"
                            size="sm"
                            className="h-6 w-6 shrink-0 text-[var(--muted)]"
                            aria-label={`取消关联：${item.title}`}
                            onPress={() => onUnlinkContext(item.id)}
                            isDisabled={removingContextId !== null}
                          >
                            <X size={13} />
                          </Button>
                        </div>
                      ))}
                    </div>
                  )}
                  <div className="flex items-center gap-1.5">
                    <Select
                      aria-label="选择要关联的笔记或文档"
                      className="min-w-0 flex-1"
                      placeholder={
                        contextOptionsLoading
                          ? "加载可选内容…"
                          : availableContextOptions.length === 0
                            ? "没有可关联的内容"
                            : "选择笔记或文档…"
                      }
                      selectedKey={contextKey}
                      onSelectionChange={(key) =>
                        setContextKey(key == null ? null : String(key))
                      }
                      isDisabled={
                        linkingContext ||
                        contextOptionsLoading ||
                        availableContextOptions.length === 0
                      }
                    >
                      <Select.Trigger className="h-7 text-ui">
                        <Select.Value />
                        <Select.Indicator />
                      </Select.Trigger>
                      <Select.Popover>
                        <ListBox>
                          {availableContextOptions.map((option) => (
                            <ListBox.Item
                              key={`${option.kind}:${option.id}`}
                              id={option.id}
                              textValue={`${
                                option.kind === "doc" ? "文档" : "笔记"
                              } · ${option.title}`}
                            >
                              <span className="flex min-w-0 flex-1 items-center gap-2">
                                <Chip
                                  size="sm"
                                  variant="tertiary"
                                  color="default"
                                  className="shrink-0"
                                >
                                  {option.kind === "doc" ? "文档" : "笔记"}
                                </Chip>
                                <span className="min-w-0 flex-1 truncate">
                                  {option.title}
                                </span>
                                <ListBox.ItemIndicator />
                              </span>
                            </ListBox.Item>
                          ))}
                        </ListBox>
                      </Select.Popover>
                    </Select>
                    <Button
                      variant="secondary"
                      size="sm"
                      className="h-7 shrink-0 px-2.5 text-ui"
                      onPress={() => void submitContext()}
                      isDisabled={!contextKey || linkingContext}
                    >
                      {linkingContext ? "关联中…" : "关联"}
                    </Button>
                  </div>
                  {contextOptionsError && (
                    <p role="alert" className="text-ui-sm text-[var(--danger)]">
                      {contextOptionsError}
                    </p>
                  )}
                </div>
              )}
            </Section>
  );
}
