import { expect, openApp, openView, test } from "./mock";

test("开发者模式保存调用 set_dev_mode，开启后显示 token 与日志", async ({
  page,
  mock,
}) => {
  await openApp(page);
  await openView(page, "设置");

  const dev = page
    .getByRole("heading", { name: "开发者", exact: true })
    .locator("xpath=ancestor::section[1]");
  const devSwitch = dev.getByRole("switch", { name: "开发者模式" });
  await expect(devSwitch).toBeVisible();
  // Heroui Switch 的可见控件是覆盖在 input 上的 span，直接点击会被拦截；
  // 对 input 触发原生 click 才能走 React 的 onChange。
  await devSwitch.evaluate((element) => (element as HTMLElement).click());
  await expect(devSwitch).toBeChecked();

  await dev.getByRole("button", { name: "保存", exact: true }).click();

  await expect
    .poll(async () => (await mock.calls("set_dev_mode")).length)
    .toBe(1);
  const calls = await mock.calls("set_dev_mode");
  expect(calls[0].args).toEqual({ enabled: true, port: 1421 });

  await expect(dev.getByText("已保存")).toBeVisible();
  await expect(dev.getByText("本地 API", { exact: true })).toBeVisible();
  await expect(dev.getByLabel("应用日志")).toContainText(
    "temli dev api listening",
  );

  await dev.getByRole("button", { name: "显示" }).click();
  await expect(dev.getByText("temli-dev-token-e2e")).toBeVisible();

  const logCalls = await mock.calls("read_app_log");
  expect(logCalls.at(-1)?.args).toEqual({ lines: 200 });
});
