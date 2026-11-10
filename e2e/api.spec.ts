import { test, expect } from "@playwright/test";

/**
 * 真实后端集成用例：直接请求开发者模式下的本地 HTTP API。
 *
 * 需要先在设置里开启开发者模式，并通过环境变量提供：
 * - TEMLI_DEV_API_TOKEN（设置页可复制）
 * - TEMLI_DEV_API（可选，默认 http://127.0.0.1:3789）
 * - TEMLI_TEST_PROJECT_ID（可选，用于任务列表断言）
 *
 * 未提供 token 时整组跳过，避免 CI 在没有 GUI 的 runner 上假跑。
 */
const base = process.env.TEMLI_DEV_API ?? "http://127.0.0.1:3789";
const token = process.env.TEMLI_DEV_API_TOKEN;
const projectId = process.env.TEMLI_TEST_PROJECT_ID;

test.skip(!token, "需要运行中的应用与 TEMLI_DEV_API_TOKEN");

test("health 无需认证，业务接口需要 Bearer", async ({ request }) => {
  const health = await request.get(`${base}/health`);
  expect(health.ok()).toBeTruthy();
  const body = await health.json();
  expect(body.ok).toBe(true);
  expect(typeof body.version).toBe("string");

  const unauthorized = await request.get(`${base}/summary`);
  expect(unauthorized.status()).toBe(401);

  const summary = await request.get(`${base}/summary`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  expect(summary.ok()).toBeTruthy();
  const payload = await summary.json();
  expect(payload).toHaveProperty("project_count");
  expect(payload).toHaveProperty("task_status_counts");
  expect(typeof payload.default_agent).toBe("string");
});

test("任务列表与日志尾部返回真实数据", async ({ request }) => {
  const headers = { Authorization: `Bearer ${token}` };

  const logs = await request.get(`${base}/logs?lines=5`, { headers });
  expect(logs.ok()).toBeTruthy();
  const lines = await logs.json();
  expect(Array.isArray(lines)).toBeTruthy();

  test.skip(!projectId, "设置 TEMLI_TEST_PROJECT_ID 后验证任务列表");
  const tasks = await request.get(`${base}/tasks?project_id=${projectId}`, {
    headers,
  });
  expect(tasks.ok()).toBeTruthy();
  const rows = await tasks.json();
  expect(Array.isArray(rows)).toBeTruthy();
});
