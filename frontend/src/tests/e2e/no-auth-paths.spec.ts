import { expect } from '@playwright/test';
import { test } from '$test_helpers/e2e-fixture';
import {
  seedAnonymous,
  seedSetupPending,
  setupSession
} from '$test_helpers/session';
import { gotoReady } from '$test_helpers/layout';

const pathname = (page: { url: () => string }) => new URL(page.url()).pathname;

// Public paths from `noAuthPaths` (`$lib/components/nav.svelte`). /password
// Always forwards to /password/forgot; /setup only stays put while setup is
// Pending (otherwise it bounces to /).
const publicCases: { path: string; expected: string; pending?: boolean }[] = [
  { expected: '/login', path: '/login' },
  { expected: '/setup', path: '/setup', pending: true },
  { expected: '/password/forgot', path: '/password' },
  { expected: '/password/forgot', path: '/password/forgot' },
  { expected: '/password/reset', path: '/password/reset' }
];

// Every protected surface must bounce anonymous visitors to /login and keep
// The intended destination in `redirect`.
const protectedPaths = [
  '/',
  '/account/general',
  '/account/auth',
  '/auth/cli',
  '/caches',
  '/caches/create',
  '/caches/cache-main',
  '/caches/cache-main/search',
  '/caches/cache-main/settings',
  '/groups',
  '/groups/create',
  '/groups/group-admins',
  '/settings/user',
  '/settings/mail',
  '/tokens',
  '/tokens/create',
  '/tokens/token-ci',
  '/users',
  '/users/create',
  '/users/user-1'
];

test.describe('public paths without auth', () => {
  test.beforeEach(async ({ context }) => seedAnonymous(context));

  for (const { path, expected, pending } of publicCases) {
    test(`serves ${path}`, async ({ context, page }) => {
      if (pending) {
        await seedSetupPending(context);
      }
      await gotoReady(page, path);
      expect(pathname(page)).toBe(expected);
    });
  }
});

test.describe('public paths with auth', () => {
  for (const { path, expected, pending } of publicCases) {
    if (path === '/login') {
      continue;
    }
    test(`still serves ${path}`, async ({ context, page }) => {
      await setupSession(context);
      if (pending) {
        await seedSetupPending(context);
      }
      await gotoReady(page, path);
      expect(pathname(page)).toBe(expected);
    });
  }
});

test.describe('protected paths without auth', () => {
  test.beforeEach(async ({ context }) => seedAnonymous(context));

  for (const path of protectedPaths) {
    test(`redirects ${path} to /login`, async ({ page }) => {
      await gotoReady(page, path);

      const url = new URL(page.url());
      expect(url.pathname).toBe('/login');
      expect(url.searchParams.get('redirect')).toBe(path);
    });
  }

  test('keeps the query string in the redirect target', async ({ page }) => {
    await gotoReady(page, '/users?error=not_found');

    const url = new URL(page.url());
    expect(url.pathname).toBe('/login');
    expect(url.searchParams.get('redirect')).toBe('/users?error=not_found');
  });
});

test('an expired session (cookie present, token invalid) returns to /login', async ({
  context,
  page
}) => {
  await setupSession(context);
  await seedAnonymous(context);
  await gotoReady(page, '/tokens');

  await expect(page).toHaveURL(/\/login\?redirect=%2Ftokens$/);
});
