import { expect } from '@playwright/test';
import { HttpResponse } from 'msw';
import * as gen from '$lib/client/msw.gen';
import { adminUser } from '$mocks/e2e/data';
import { test } from '$test_helpers/e2e-fixture';
import { setupSession } from '$test_helpers/session';
import {
  expectNoHorizontalOverflow,
  gotoReady,
  openSidebar
} from '$test_helpers/layout';

test.beforeEach(async ({ context }) => {
  await setupSession(context);
});

test('overview page renders inside the app shell', async ({ page }) => {
  await gotoReady(page, '/');

  await expect(page.getByRole('main').getByText('Overview')).toBeVisible();
  await expectNoHorizontalOverflow(page);
});

test('overview shows caches, tokens and paths', async ({ page }) => {
  await gotoReady(page, '/');
  const main = page.getByRole('main');

  // Two mocked caches, only the `ci` token is not expired
  await expect(main.locator('a[href="/caches/cache-private"]')).toBeVisible();
  await expect(main.locator('a[href="/tokens/token-ci"]')).toBeVisible();
  await expect(main.locator('a[href="/tokens/token-old"]')).toHaveCount(0);
  await expect(main.getByText('ripgrep-14.1')).toBeVisible();
  await expect(main.getByText('hello-2.12')).toBeVisible();
  await expect(main.getByText('3×')).toBeVisible();
});

test('overview shows empty states', async ({ context, page }) => {
  await setupSession(context, 'empty');
  await gotoReady(page, '/');
  const main = page.getByRole('main');

  await expect(main.getByText('No caches yet.')).toBeVisible();
  await expect(main.getByText('No active tokens.')).toBeVisible();
  await expect(main.getByText('Nothing pushed yet.')).toBeVisible();
  await expect(main.getByText('Nothing pulled yet.')).toBeVisible();
});

test('sidebar wires up the navigation targets', async ({ page }) => {
  await gotoReady(page, '/');
  await openSidebar(page);

  for (const href of ['/caches', '/tokens', '/users', '/groups', '/settings']) {
    // oxlint-disable-next-line no-await-in-loop
    await expect(page.locator(`a[href="${href}"]`).first()).toBeAttached();
  }
});

test('navigating to caches from the sidebar lists mocked caches', async ({
  page
}) => {
  await gotoReady(page, '/');
  await openSidebar(page);
  await page.locator('a[href="/caches"]').first().click();

  await expect(page).toHaveURL(/\/caches/);
  await expect(page.getByRole('heading', { name: 'Caches' })).toBeVisible();
  await expect(page.getByText('private', { exact: true })).toBeVisible();
});

test('hides admin navigation for users without permissions', async ({
  context,
  page
}) => {
  await setupSession(context, 'readonly');
  await gotoReady(page, '/');
  await openSidebar(page);

  await expect(page.locator('a[href="/caches"]').first()).toBeAttached();
  await expect(page.locator('a[href="/tokens"]').first()).toBeAttached();
  for (const href of ['/users', '/groups', '/settings']) {
    // oxlint-disable-next-line no-await-in-loop
    await expect(page.locator(`a[href="${href}"]`)).toHaveCount(0);
  }
});

test('logs out through the user menu', async ({ network, page }) => {
  let called = false;
  network.use(
    gen.handleLogout(() => {
      called = true;
      // The backend clears the session cookie on logout.
      return HttpResponse.json(
        {},
        { headers: { 'Set-Cookie': 'centaurus_jwt=; Max-Age=0; Path=/' } }
      ) as never;
    })
  );
  await gotoReady(page, '/');
  await openSidebar(page);

  await page.getByText(adminUser.email).first().click();
  await page.getByRole('menuitem', { name: 'Log out' }).click();

  await expect(page).toHaveURL(/\/login$/);
  expect(called).toBe(true);
});
