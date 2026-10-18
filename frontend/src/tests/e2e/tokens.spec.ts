import { type Page, expect } from '@playwright/test';
import * as gen from '$lib/client/msw.gen';
import { fail, json, test } from '$test_helpers/e2e-fixture';
import { setupSession } from '$test_helpers/session';
import { expectNoHorizontalOverflow, gotoReady } from '$test_helpers/layout';

const row = (page: Page, text: string) =>
  page.getByRole('row').filter({ hasText: text });

const openDetail = async (page: Page) => {
  await gotoReady(page, '/tokens/token-ci');
  await expect(page.getByPlaceholder('Enter name')).toHaveValue('ci');
};

test.beforeEach(async ({ context }) => setupSession(context));

test.describe('tokens list', () => {
  test('lists tokens with usage info', async ({ page }) => {
    await gotoReady(page, '/tokens');

    await expect(page.getByRole('heading', { name: 'Token' })).toBeVisible();
    await expect(row(page, 'token-ci')).toBeVisible();
    await expect(row(page, 'token-old')).toContainText('Never');
    await expectNoHorizontalOverflow(page);
  });

  test('shows an empty state and nothing to purge', async ({
    context,
    page
  }) => {
    await setupSession(context, 'empty');
    await gotoReady(page, '/tokens');

    await expect(page.getByText('No results.')).toBeVisible();
    await expect(
      page.getByRole('button', { name: 'Delete Expired' })
    ).toBeDisabled();
  });

  test('shows an empty table when the list fails to load', async ({
    network,
    page
  }) => {
    network.use(gen.handleListTokens(() => fail(500)));
    await gotoReady(page, '/tokens');

    await expect(page.getByText('No results.')).toBeVisible();
  });

  test('disables the purge when no token is expired', async ({
    network,
    page
  }) => {
    network.use(
      gen.handleListTokens(() =>
        json([{ exp: '2099-01-01T00:00:00Z', name: 'ci', uuid: 'token-ci' }])
      )
    );
    await gotoReady(page, '/tokens');

    await expect(row(page, 'token-ci')).toBeVisible();
    await expect(
      page.getByRole('button', { name: 'Delete Expired' })
    ).toBeDisabled();
  });

  for (const [deleted, message] of [
    [1, 'Deleted 1 expired token(s)'],
    [0, 'No expired tokens to delete']
  ] as const) {
    test(`purges expired tokens (${deleted} deleted)`, async ({
      network,
      page
    }) => {
      let called = false;
      network.use(
        gen.handleDeleteExpiredTokens(() => {
          called = true;
          return json({ deleted });
        })
      );
      await gotoReady(page, '/tokens');

      await page.getByRole('button', { name: 'Delete Expired' }).click();
      const dialog = page.getByRole('dialog');
      // Exactly one fixture token ("old") is past its expiry.
      await expect(
        dialog.getByText('Permanently delete 1 expired token(s)?')
      ).toBeVisible();
      await dialog.getByRole('button', { name: 'Delete' }).click();

      await expect(page.getByText(message)).toBeVisible();
      expect(called).toBe(true);
    });
  }

  test('surfaces a failed purge', async ({ network, page }) => {
    network.use(gen.handleDeleteExpiredTokens(() => fail(500)));
    await gotoReady(page, '/tokens');

    await page.getByRole('button', { name: 'Delete Expired' }).click();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Delete' })
      .click();

    await expect(
      page.getByText('Failed to delete expired tokens')
    ).toBeVisible();
  });

  test('deletes a single token', async ({ network, page }) => {
    let body = undefined as unknown;
    network.use(
      gen.handleDeleteToken(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await gotoReady(page, '/tokens');

    await row(page, 'token-old').getByRole('button').click();
    const dialog = page.getByRole('dialog');
    await expect(
      dialog.getByText('Do you really want to delete the token old?')
    ).toBeVisible();
    await dialog.getByRole('button', { name: 'Delete' }).click();

    await expect(
      page.getByText('Token old deleted successfully')
    ).toBeVisible();
    expect(body).toEqual({ uuid: 'token-old' });
  });

  test('surfaces a failed delete', async ({ network, page }) => {
    network.use(gen.handleDeleteToken(() => fail(500)));
    await gotoReady(page, '/tokens');

    await row(page, 'token-old').getByRole('button').click();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Delete' })
      .click();

    await expect(page.getByText('Failed to delete token')).toBeVisible();
  });

  test('filters via the search input', async ({ page }) => {
    await gotoReady(page, '/tokens');
    await expect(row(page, 'token-old')).toBeVisible();

    await page.getByPlaceholder('Search...').fill('token-ci');
    await expect(row(page, 'token-old')).toHaveCount(0);
    await expect(row(page, 'token-ci')).toBeVisible();
  });

  test('links rows to their detail page', async ({ page }) => {
    await gotoReady(page, '/tokens');

    await row(page, 'token-ci').getByRole('link').click();
    await expect(page).toHaveURL(/\/tokens\/token-ci$/);
  });
});

test.describe('token create', () => {
  test('creates a token and reveals the secret once', async ({
    network,
    page
  }) => {
    let body = undefined as Record<string, unknown> | undefined;
    network.use(
      gen.handleCreateToken(async ({ request }) => {
        body = await request.json();
        return json({ token: 'secret-token', uuid: 'token-new' });
      })
    );
    await gotoReady(page, '/tokens/create');
    await expectNoHorizontalOverflow(page);

    await page.getByPlaceholder('Enter name').fill('deploy');
    await page.getByRole('button', { name: 'Create' }).click();

    await expect(page).toHaveURL(/\/tokens\/token-new$/);
    await expect(page.getByText('secret-token')).toBeVisible();
    await expect(page.getByText('(Can not be viewed again!)')).toBeVisible();
    expect(body?.name).toBe('deploy');
    // Defaults to ~30 days from now, sent as an ISO timestamp.
    const days =
      (new Date(String(body?.exp)).getTime() - Date.now()) / (24 * 3600 * 1000);
    expect(days).toBeGreaterThan(29);
    expect(days).toBeLessThan(31);

    // The secret lives only in sessionStorage and is gone after a reload.
    await page.reload();
    await page.waitForLoadState('networkidle');
    await expect(page.getByText('secret-token')).toHaveCount(0);
    await expect(page.getByRole('textbox').last()).toHaveValue(
      'Can not be viewed.'
    );
  });

  test('rejects an empty name', async ({ page }) => {
    await gotoReady(page, '/tokens/create');

    await page.getByPlaceholder('Enter name').fill('x');
    await page.getByPlaceholder('Enter name').fill('');
    await page.getByRole('button', { name: 'Create' }).click();

    await expect(page.getByText('Name is required')).toBeVisible();
    await expect(page).toHaveURL(/\/tokens\/create$/);
  });

  for (const [status, message] of [
    [409, 'A token with this name already exists.'],
    [500, 'Error creating token.']
  ] as const) {
    test(`surfaces a ${status}`, async ({ network, page }) => {
      network.use(gen.handleCreateToken(() => fail(status)));
      await gotoReady(page, '/tokens/create');

      await page.getByPlaceholder('Enter name').fill('deploy');
      await page.getByRole('button', { name: 'Create' }).click();

      await expect(page.getByText(message)).toBeVisible();
    });
  }
});

test.describe('token detail', () => {
  test('renders the token without its secret', async ({ page }) => {
    await openDetail(page);

    await expect(page.getByRole('heading', { name: /Token:/ })).toContainText(
      'ci'
    );
    await expect(page.getByRole('textbox').last()).toHaveValue(
      'Can not be viewed.'
    );
    await expectNoHorizontalOverflow(page);
  });

  test('saves the token name', async ({ network, page }) => {
    let body = undefined as Record<string, unknown> | undefined;
    network.use(
      gen.handleEditToken(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await openDetail(page);

    await page.getByPlaceholder('Enter name').fill('ci-renamed');
    await page.getByRole('button', { name: 'Save Changes' }).click();

    await expect(page.getByText('Token ci updated successfully')).toBeVisible();
    expect(body).toMatchObject({ name: 'ci-renamed', uuid: 'token-ci' });
    expect(new Date(body?.exp as string).toISOString()).toBe(
      '2099-01-01T00:00:00.000Z'
    );
  });

  test('requires a name', async ({ page }) => {
    await openDetail(page);

    await page.getByPlaceholder('Enter name').fill('');
    await page.getByRole('button', { name: 'Save Changes' }).click();

    await expect(page.getByText('Token name is required')).toBeVisible();
  });

  for (const [status, message] of [
    [409, 'This token name is already in use'],
    [500, 'Failed to update token']
  ] as const) {
    test(`surfaces a ${status} on save`, async ({ network, page }) => {
      network.use(gen.handleEditToken(() => fail(status)));
      await openDetail(page);

      await page.getByRole('button', { name: 'Save Changes' }).click();

      await expect(page.getByText(message)).toBeVisible();
    });
  }

  test('regenerates and copies the secret', async ({
    browserName,
    context,
    page
  }) => {
    await openDetail(page);

    await page.getByRole('button', { name: 'Regenerate' }).click();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Regenerate' })
      .click();

    await expect(
      page.getByText('Token ci regenerated successfully')
    ).toBeVisible();
    const copy = page.getByRole('button', { name: /secret-token-2/ });
    await expect(copy).toBeVisible();

    // Clipboard permissions can only be granted in Chromium.
    if (browserName === 'chromium') {
      await context.grantPermissions(['clipboard-read', 'clipboard-write']);
      await copy.click();
      await expect(copy.getByText('Copied')).toBeAttached();
      expect(
        await page.evaluate(async () => navigator.clipboard.readText())
      ).toBe('secret-token-2');
    }
  });

  test('surfaces a failed regenerate', async ({ network, page }) => {
    network.use(gen.handleTokenRegenerate(() => fail(500)));
    await openDetail(page);

    await page.getByRole('button', { name: 'Regenerate' }).click();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Regenerate' })
      .click();

    await expect(page.getByText('Failed to regenerate token')).toBeVisible();
  });

  test('deletes the token and returns to the list', async ({
    network,
    page
  }) => {
    let body = undefined as unknown;
    network.use(
      gen.handleDeleteToken(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await openDetail(page);

    await page.getByRole('button', { name: 'Delete' }).click();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Delete' })
      .click();

    await expect(page).toHaveURL(/\/tokens$/);
    expect(body).toEqual({ uuid: 'token-ci' });
  });

  test('surfaces a failed delete', async ({ network, page }) => {
    network.use(gen.handleDeleteToken(() => fail(500)));
    await openDetail(page);

    await page.getByRole('button', { name: 'Delete' }).click();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Delete' })
      .click();

    await expect(page.getByText('Failed to delete token')).toBeVisible();
  });

  for (const [status, message] of [
    [404, 'Token not found'],
    [500, 'Failed to load token']
  ] as const) {
    test(`redirects to the list on a ${status}`, async ({ network, page }) => {
      network.use(gen.handleTokenInfo(() => fail(status)));
      await gotoReady(page, '/tokens/token-ci');

      await expect(page).toHaveURL(/\/tokens$/);
      await expect(page.getByText(message)).toBeVisible();
    });
  }
});
