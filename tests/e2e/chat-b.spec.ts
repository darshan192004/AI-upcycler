import { expect, test, type Page } from '@playwright/test';

// Sub-project B acceptance checks (plan Task 13). These are written before the
// desktop UI exists: every assertion below is RED until Task 14 lands.
const APP_URL = 'http://127.0.0.1:8080';
const CHAT_URL = '**/v1/chat/completions';
const STREAM_BODY =
  'data: {"choices":[{"delta":{"content":"ok"}}]}\n\n' + 'data: [DONE]\n\n';

type ChatBody = Record<string, unknown>;

async function gotoChat(page: Page): Promise<void> {
  await page.goto(APP_URL);
  await page.getByRole('tab', { name: 'Chat' }).click();
}

async function stubChat(
  page: Page,
  onBody: (body: ChatBody) => void = () => undefined,
): Promise<void> {
  await page.route(CHAT_URL, async (route) => {
    onBody(route.request().postDataJSON() as ChatBody);
    await route.fulfill({
      status: 200,
      contentType: 'text/event-stream',
      body: STREAM_BODY,
    });
  });
}

test('provider switcher has the accessible name Provider', async ({ page }) => {
  await gotoChat(page);
  await expect(
    page.getByRole('combobox', { name: 'Provider', exact: true }),
  ).toBeVisible();
});

test('selecting openai puts provider openai on the next chat request', async ({
  page,
}) => {
  const bodies: ChatBody[] = [];
  await stubChat(page, (body) => bodies.push(body));
  await gotoChat(page);

  await page.getByRole('combobox', { name: 'Provider', exact: true }).selectOption('openai');
  await page.locator('textarea').fill('check provider routing');
  await page.getByRole('button', { name: 'Send message' }).click();

  await expect.poll(() => bodies[bodies.length - 1]?.provider).toBe('openai');
});

test('source badge reads Local engine for local telemetry', async ({ page }) => {
  let telemetrySocket: { send: (message: string) => void } | undefined;
  await page.routeWebSocket('**/api/telemetry', (socket) => {
    telemetrySocket = socket;
  });
  await gotoChat(page);

  const payload = JSON.stringify({
    event: 'generation_metrics',
    timestamp: Math.floor(Date.now() / 1000),
    data: {
      tokens_per_second: 12.5,
      current_token: 1,
      source: 'local',
      master_ram_utilization_pct: null,
      worker_nodes: [],
    },
  });
  // Resend until the app has attached its WebSocket listener: the route is
  // installed before navigation, but the listener is attached in an effect.
  await expect
    .poll(async () => {
      telemetrySocket?.send(payload);
      return await page.getByText('Local engine', { exact: true }).isVisible();
    })
    .toBe(true);
});

test('a session from a previous turn is in the session select after reload', async ({
  page,
}) => {
  let createdSessionId = '';
  await stubChat(page, (body) => {
    if (typeof body.session_id === 'string') createdSessionId = body.session_id;
  });
  await page.route('**/api/sessions', async (route) => {
    const sessions = createdSessionId
      ? [
          {
            session_id: createdSessionId,
            title: 'previous turn',
            model_id: 'local-model',
            created_at: new Date().toISOString(),
          },
        ]
      : [];
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ sessions }),
    });
  });

  await gotoChat(page);
  await page.locator('textarea').fill('create a shared session');
  await page.getByRole('button', { name: 'Send message' }).click();
  await expect.poll(() => createdSessionId).not.toBe('');

  await page.reload();
  await page.getByRole('tab', { name: 'Chat' }).click();
  const sessionSelect = page.getByRole('combobox', { name: 'Chat session' });
  await expect(sessionSelect).toBeVisible();
  await expect(
    sessionSelect.locator(`option[value="${createdSessionId}"]`),
  ).toHaveCount(1);
});

test('no browser-storage key contains chat message text', async ({ page }) => {
  const marker = 'privatelm-chat-secret-no-storage';
  await stubChat(page);
  await gotoChat(page);

  await page.locator('textarea').fill(marker);
  await page.getByRole('button', { name: 'Send message' }).click();
  await expect(page.getByText(marker, { exact: true }).first()).toBeVisible();

  const storageHits = await page.evaluate(() => {
    const hits: string[] = [];
    for (let i = 0; i < localStorage.length; i += 1) {
      const key = localStorage.key(i) ?? '';
      const value = localStorage.getItem(key) ?? '';
      if (key.includes('privatelm-chat-secret-') || value.includes('privatelm-chat-secret-')) {
        hits.push(key);
      }
    }
    return hits;
  });
  expect(storageHits).toEqual([]);
});
