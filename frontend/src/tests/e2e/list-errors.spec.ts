import { expect } from '@playwright/test';
import { test } from '$test_helpers/e2e-fixture';
import { setupSession } from '$test_helpers/session';
import { gotoReady } from '$test_helpers/layout';

test.beforeEach(async ({ context }) => setupSession(context));

// Detail pages redirect back to their list with `?error=...` when a record
// Fails to load; the list surfaces that as a toast and strips the param.
const cases = [
  { list: '/users', notFound: 'User not found', other: 'Failed to load user' },
  {
    list: '/groups',
    notFound: 'Group not found',
    other: 'Failed to load group'
  },
  {
    list: '/tokens',
    notFound: 'Token not found',
    other: 'Failed to load token'
  },
  {
    list: '/caches',
    notFound: 'Cache not found',
    other: 'Failed to load cache'
  }
];

test.describe('list error toasts', () => {
  for (const { list, notFound, other } of cases) {
    test(`${list}?error=not_found`, async ({ page }) => {
      await gotoReady(page, `${list}?error=not_found`);

      await expect(page.getByText(notFound)).toBeVisible();
      await expect(page).toHaveURL(new RegExp(`${list}$`));
    });

    test(`${list}?error=other`, async ({ page }) => {
      await gotoReady(page, `${list}?error=other`);

      await expect(page.getByText(other)).toBeVisible();
      await expect(page).toHaveURL(new RegExp(`${list}$`));
    });
  }
});
