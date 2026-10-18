import { type Page, expect } from '@playwright/test';
import * as gen from '$lib/client/msw.gen';
import { fail, json, test } from '$test_helpers/e2e-fixture';
import { seedAnonymous, setupSession } from '$test_helpers/session';
import { gotoReady } from '$test_helpers/layout';

// The page hands the code to the CLI's local callback server.
const CLI_SERVER = 'http://localhost:16401/**';

/**
 * On success the page shows a confirmation and calls `window.close()`.
 * Firefox honours that for Playwright-opened pages, Chromium does not.
 */
const finished = async (page: Page) => {
  await expect
    .poll(
      async () =>
        page.isClosed() ||
        (await page
          .getByText(
            'CLI authenticated successfully! You can now close this window.'
          )
          .isVisible()
          .catch(() => page.isClosed()))
    )
    .toBe(true);
};

test.describe('cli auth', () => {
  test.beforeEach(async ({ context }) => setupSession(context));

  test('sends the code to the local CLI server', async ({ page }) => {
    let sent: string | null = null;
    await page.route(CLI_SERVER, async (route) => {
      sent = new URL(route.request().url()).searchParams.get('code');
      return route.fulfill({
        body: 'ok',
        headers: { 'Access-Control-Allow-Origin': '*' }
      });
    });
    // No `gotoReady`: the page may close itself before the network idles.
    await page.goto('/auth/cli');

    await finished(page);
    expect(sent).toBe('cli-code');
  });

  test('shows the code for manual entry when the CLI is unreachable', async ({
    page
  }) => {
    await page.route(CLI_SERVER, async (route) => route.abort());
    await gotoReady(page, '/auth/cli');

    await expect(
      page.getByText('Failed to send code. Please enter it manually:')
    ).toBeVisible();
    await expect(page.getByRole('textbox')).toHaveValue('cli-code');
    await expect(page.getByRole('button', { name: 'Try Again' })).toHaveCount(
      0
    );
  });

  test('offers a retry when no code can be requested', async ({
    network,
    page
  }) => {
    await page.route(CLI_SERVER, async (route) =>
      route.fulfill({
        body: 'ok',
        headers: { 'Access-Control-Allow-Origin': '*' }
      })
    );
    network.use(gen.handleNewCode(() => fail(500)));
    await gotoReady(page, '/auth/cli');

    await expect(page.getByText('Failed to get CLI auth code')).toBeVisible();

    network.use(gen.handleNewCode(() => json({ code: 'retry-code' })));
    await page.getByRole('button', { name: 'Try Again' }).click();

    await finished(page);
  });
});

test('requires a session', async ({ context, page }) => {
  await seedAnonymous(context);
  await gotoReady(page, '/auth/cli');

  await expect(page).toHaveURL(/\/login\?redirect=%2Fauth%2Fcli$/);
});
