import type { Locator, Page } from '@playwright/test';
import { test } from '$test_helpers/e2e-fixture';
import {
  seedAnonymous,
  seedSetupPending,
  setupSession
} from '$test_helpers/session';
import { expectPageFits } from '$test_helpers/layout';

/**
 * Systematic horizontal-overflow sweep. Every Playwright project (desktop +
 * Mobile Chrome) runs each entry, so this catches small-screen overflow.
 */
const heading = (name: string | RegExp) => (p: Page) =>
  p.getByRole('heading', { name });

const pages: { path: string; ready: (page: Page) => Locator }[] = [
  { path: '/', ready: (p) => p.getByRole('main').getByText('Overview') },
  { path: '/caches', ready: (p) => p.locator('a[href="/caches/cache-main"]') },
  { path: '/caches/create', ready: (p) => p.getByPlaceholder('Cache name') },
  { path: '/caches/cache-main', ready: (p) => p.getByText('Number of paths:') },
  {
    path: '/caches/cache-main/search',
    ready: (p) => p.getByText('/nix/store/', { exact: false })
  },
  {
    path: '/caches/cache-main/settings',
    ready: (p) => p.getByText('Danger Zone')
  },
  { path: '/tokens', ready: (p) => p.getByText('token-ci') },
  { path: '/tokens/create', ready: (p) => p.getByPlaceholder('Enter name') },
  { path: '/tokens/token-ci', ready: heading(/Token:/) },
  { path: '/users', ready: (p) => p.getByText('bob@example.com') },
  {
    path: '/users/create',
    ready: (p) => p.getByPlaceholder('Enter user name')
  },
  { path: '/users/user-1', ready: heading(/User:/) },
  { path: '/groups', ready: (p) => p.getByText('group-staff') },
  {
    path: '/groups/create',
    ready: (p) => p.getByPlaceholder('Enter group name')
  },
  { path: '/groups/group-admins', ready: heading(/Group:/) },
  { path: '/groups/group-staff', ready: heading('Permissions') },
  { path: '/settings/user', ready: heading('User Settings') },
  { path: '/settings/mail', ready: heading('Mail Settings') },
  { path: '/account/general', ready: heading('General Settings') },
  { path: '/account/auth', ready: heading('Authentication') },
  { path: '/auth/cli', ready: (p) => p.getByText('CLI Auth', { exact: true }) }
];

const publicPages: { path: string; ready: (page: Page) => Locator }[] = [
  { path: '/login', ready: (p) => p.getByPlaceholder('mail@example.com') },
  {
    path: '/password/forgot',
    ready: (p) => p.getByPlaceholder('mail@example.com')
  },
  {
    path: '/password/reset',
    ready: (p) => p.getByPlaceholder('Enter your token')
  }
];

test.describe('no horizontal overflow', () => {
  test.beforeEach(async ({ context }) => setupSession(context));

  for (const { path, ready } of pages) {
    test(`${path} fits its viewport`, async ({ page }) => {
      await expectPageFits(page, path, ready(page));
    });
  }

  test('/caches (empty) fits its viewport', async ({ context, page }) => {
    await setupSession(context, 'empty');
    await expectPageFits(
      page,
      '/caches',
      page.getByText('No caches found. Create one to get started.')
    );
  });
});

test.describe('no horizontal overflow (public)', () => {
  test.beforeEach(async ({ context }) => seedAnonymous(context));

  for (const { path, ready } of publicPages) {
    test(`${path} fits its viewport`, async ({ page }) => {
      await expectPageFits(page, path, ready(page));
    });
  }

  test('/setup fits its viewport', async ({ context, page }) => {
    await seedSetupPending(context);
    await expectPageFits(page, '/setup', page.getByText('Database Setup'));
  });
});
