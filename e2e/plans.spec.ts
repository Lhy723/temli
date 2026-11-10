import { expect, openApp, openView, test } from "./mock";

test("计划时间线可展开，验收意见随 accept_stage 提交", async ({
  page,
  mock,
}) => {
  await openApp(page);
  await openView(page, "计划");
  await expect(page.getByText("计划：打通 E2E")).toBeVisible();
  await expect(page.getByText("阶段 1/2")).toBeVisible();

  const plan = page.locator("article").filter({ hasText: "计划：打通 E2E" });
  await plan.getByRole("button").first().click();

  await expect(plan.getByText("基础开发")).toBeVisible();
  await expect(plan.getByText("计划验收", { exact: true })).toBeVisible();
  await expect(plan.getByText("待验收", { exact: true })).toBeVisible();

  const note = "阶段二验收通过：mock IPC 用例全部为绿。";
  await page.getByLabel("验收意见").fill(note);
  await plan.getByRole("button", { name: "验收阶段" }).last().click();

  await expect
    .poll(async () => (await mock.calls("accept_stage")).length)
    .toBe(1);
  const calls = await mock.calls("accept_stage");
  expect(calls[0].args).toEqual({ stageId: "stage-2", note });

  await expect(page.getByText("全部阶段已通过")).toBeVisible();
  await expect(page.getByText(note)).toBeVisible();
});
