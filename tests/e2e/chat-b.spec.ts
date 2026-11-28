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

async function stubLocalModels(page: Page): Promise<void> {
  await page.route('**/v1/models', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        object: 'list',
        data: [
          {
            id: 'sk-test-local-model',
            object: 'model',
            created: 0,
            owned_by: 'test',
            meta: {
              quantization: 'test',
              file_size_bytes: 1,
              layers: 1,
              recommended_min_ram_gb: 1,
            },
          },
        ],
      }),
    });
  });
  await page.route('**/api/providers', async (route) => {
    await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' });
  });
}

function sessionRow(id: string, title: string): ChatBody {
  return {
    session_id: id,
    title,
    model_id: 'sk-test-local-model',
    created_at: new Date().toISOString(),
  };
}

function messageRow(id: string, content: string): ChatBody {
  return {
    message_id: id,
    role: 'user',
    content,
    token_count: 1,
    generation_time_sec: 0,
    created_at: new Date().toISOString(),
  };
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
  const telemetrySockets: { send: (message: string) => void }[] = [];
  await page.routeWebSocket('**/api/telemetry', (socket) => {
    telemetrySockets.push(socket);
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
      for (const socket of telemetrySockets) socket.send(payload);
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

test('new session clears the thread but keeps saved sessions', async ({ page }) => {
  const savedId = 'sk-test-session-a';
  await stubLocalModels(page);
  await page.route('**/api/sessions', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ sessions: [sessionRow(savedId, 'saved turn')] }),
    });
  });
  await page.route('**/api/sessions/*/messages', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        messages: [messageRow('sk-test-message-a', 'sk-test-saved-message')],
      }),
    });
  });
  await gotoChat(page);

  const sessionSelect = page.getByRole('combobox', { name: 'Chat session' });
  await sessionSelect.selectOption(savedId);
  await expect(page.getByText('sk-test-saved-message', { exact: true })).toBeVisible();

  await page.getByRole('button', { name: 'New session' }).click();

  await expect(page.getByText('sk-test-saved-message', { exact: true })).toHaveCount(0);
  await expect(sessionSelect.locator(`option[value="${savedId}"]`)).toHaveCount(1);
  await expect.poll(() => sessionSelect.inputValue()).not.toBe(savedId);
});

test('image parts from a turn remain in the next request history', async ({ page }) => {
  const bodies: ChatBody[] = [];
  await stubLocalModels(page);
  await stubChat(page, (body) => bodies.push(body));
  await page.addInitScript(() => {
    Object.assign(window, { __AI_UPCYCLER_MULTIMODAL__: true });
  });
  await page.route('**/api/media', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        media_hash: 'sk-test-media-hash',
        mime_type: 'image/png',
        file_name: 'sk-test-pixel.png',
        byte_count: 4,
      }),
    });
  });
  await gotoChat(page);
  await expect(page.getByRole('combobox', { name: 'Model', exact: true })).toHaveValue(
    'sk-test-local-model',
  );

  const attachInput = page.locator('input[aria-label="Attach image"]');
  await expect(attachInput).toBeVisible();
  await attachInput.setInputFiles({
    name: 'sk-test-pixel.png',
    mimeType: 'image/png',
    buffer: Buffer.from([0x89, 0x50, 0x4e, 0x47]),
  });
  await expect(page.getByLabel('Image attachments').locator('li')).toHaveCount(1);

  await page.locator('textarea').fill('sk-test-first-turn');
  await page.getByRole('button', { name: 'Send message' }).click();
  await expect.poll(() => bodies.length).toBe(1);

  await page.locator('textarea').fill('sk-test-second-turn');
  await page.getByRole('button', { name: 'Send message' }).click();
  await expect.poll(() => bodies.length).toBe(2);

  const messages = bodies[1].messages as Array<{ role: string; content: unknown }>;
  const retained = messages.some(
    (message) =>
      message.role === 'user' &&
      Array.isArray(message.content) &&
      message.content.some(
        (part: unknown) =>
          typeof part === 'object' &&
          part !== null &&
          (part as ChatBody).type === 'image' &&
          (part as ChatBody).media_hash === 'sk-test-media-hash',
      ),
  );
  expect(retained).toBe(true);
});

test('stale session messages cannot replace the current thread', async ({ page }) => {
  const staleId = 'sk-test-session-a';
  const freshId = 'sk-test-session-b';
  let releaseStale = () => undefined;
  const staleGate = new Promise<void>((resolve) => {
    releaseStale = resolve;
  });
  let releaseFresh = () => undefined;
  const freshGate = new Promise<void>((resolve) => {
    releaseFresh = resolve;
  });
  let staleServed = false;

  await stubLocalModels(page);
  await page.route('**/api/sessions', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        sessions: [sessionRow(staleId, 'stale'), sessionRow(freshId, 'fresh')],
      }),
    });
  });
  await page.route('**/api/sessions/*/messages', async (route) => {
    const url = route.request().url();
    if (url.includes(`/${staleId}/messages`)) {
      await staleGate;
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          messages: [messageRow('sk-test-message-stale', 'sk-test-stale-message')],
        }),
      });
      staleServed = true;
      return;
    }
    await freshGate;
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        messages: [messageRow('sk-test-message-fresh', 'sk-test-fresh-message')],
      }),
    });
  });

  await gotoChat(page);
  const sessionSelect = page.getByRole('combobox', { name: 'Chat session' });
  await page.locator('textarea').fill('sk-test-draft');
  await sessionSelect.selectOption(staleId);
  await sessionSelect.selectOption(freshId);

  await expect(page.getByRole('button', { name: 'Send message' })).toBeDisabled();
  releaseFresh();
  await expect(page.getByText('sk-test-fresh-message', { exact: true })).toBeVisible();
  releaseStale();
  await expect.poll(() => staleServed).toBe(true);
  await page.waitForTimeout(100);
  await expect(page.getByText('sk-test-stale-message', { exact: true })).toHaveCount(0);
});
