import { expect, openApp, openView, test } from "./mock";

test("打开笔记并通过标签筛选与属性面板查看内容", async ({ page, mock }) => {
  await openApp(page);
  await openView(page, "知识");

  await page.getByRole("button", { name: "欢迎", exact: true }).click();
  const title = page.getByLabel("笔记标题");
  await expect(title).toHaveValue("欢迎");
  await expect(
    page.getByText("这是 E2E mock 笔记正文。", { exact: true }),
  ).toBeVisible();

  await page.getByRole("button", { name: "属性", exact: true }).click();
  await expect(page.getByLabel("属性面板")).toBeVisible();
  await expect(page.getByLabel("status的值")).toHaveValue("draft");
  await expect(page.getByLabel("owner的值")).toHaveValue("lhy");

  await page.getByRole("button", { name: /#工作/ }).first().click();

  await expect
    .poll(async () => (await mock.calls("notes_by_tag")).length)
    .toBe(1);
  const calls = await mock.calls("notes_by_tag");
  expect(calls[0].args).toEqual({ vaultId: "vault-1", tag: "工作" });

  await expect(
    page.getByRole("button", { name: /按标签命中/ }),
  ).toBeVisible();
});
