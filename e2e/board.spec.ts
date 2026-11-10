import { expect, openApp, openView, test } from "./mock";

test("看板渲染任务卡与 nextActor，并打开详情查看证据附件", async ({
  page,
  mock,
}) => {
  await openApp(page);
  await openView(page, "我的任务");

  const card = page
    .locator('div[role="button"][aria-pressed]')
    .filter({ hasText: "接入 mock IPC" });
  await expect(card).toBeVisible();
  await expect(card.getByText("待决策", { exact: true })).toBeVisible();
  await expect(card.getByText("待你", { exact: true })).toBeVisible();
  await expect(page.getByText("已有合并任务")).toBeVisible();

  await card.click();

  await expect(page.getByText("证据附件", { exact: true })).toBeVisible();
  await expect(page.getByText("vitest-report.xml")).toBeVisible();
  await expect(page.getByText("验收标准", { exact: true })).toBeVisible();
  await expect(page.getByText("pnpm test").first()).toBeVisible();

  const attachmentCalls = await mock.calls("list_task_attachments");
  expect(attachmentCalls.length).toBeGreaterThan(0);
  expect(attachmentCalls.at(-1)?.args).toEqual({ taskId: "task-1" });
});
