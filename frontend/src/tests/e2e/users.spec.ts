import { type Page, expect } from '@playwright/test';
import * as gen from '$lib/client/msw.gen';
import { fail, json, test } from '$test_helpers/e2e-fixture';
import { seedMailInactive, setupSession } from '$test_helpers/session';
import { expectNoHorizontalOverflow, gotoReady } from '$test_helpers/layout';
import { simpleCaches, userDetails } from '$mocks/e2e/data';
import { Permission } from '$lib/permissions.svelte';

const row = (page: Page, text: string) =>
  page.getByRole('row').filter({ hasText: text });

const cacheAccess = (page: Page) =>
  page.locator('h5:has-text("Cache Access") + div');

/** Waits until the detail form is populated from the mocked user. */
const openDetail = async (page: Page) => {
  await gotoReady(page, '/users/user-1');
  await expect(page.getByPlaceholder('Enter user name')).toHaveValue(
    'Bob User'
  );
};

test.beforeEach(async ({ context }) => setupSession(context));

test.describe('users list', () => {
  test('lists users with their groups', async ({ page }) => {
    await gotoReady(page, '/users');

    await expect(page.getByRole('heading', { name: 'Users' })).toBeVisible();
    await expect(row(page, 'bob@example.com')).toContainText('Admins');
    await expect(row(page, 'cara@example.com')).toContainText('-');
    await expectNoHorizontalOverflow(page);
  });

  test('shows an empty state with no users', async ({ context, page }) => {
    await setupSession(context, 'empty');
    await gotoReady(page, '/users');

    await expect(page.getByText('No results.')).toBeVisible();
  });

  test('shows an empty table when the list fails to load', async ({
    network,
    page
  }) => {
    network.use(gen.handleListUsers(() => fail(500)));
    await gotoReady(page, '/users');

    await expect(page.getByText('No results.')).toBeVisible();
  });

  test('filters via the search input', async ({ page }) => {
    await gotoReady(page, '/users');
    await expect(page.getByText('cara@example.com')).toBeVisible();

    await page.getByPlaceholder('Search...').fill('bob');
    await expect(page.getByText('bob@example.com')).toBeVisible();
    await expect(page.getByText('cara@example.com')).toBeHidden();

    await page.getByPlaceholder('Search...').fill('zzz-no-match');
    await expect(page.getByText('No results.')).toBeVisible();
  });

  // Regression: group objects were searched as "[object Object]".
  test('finds users by group name', async ({ page }) => {
    await gotoReady(page, '/users');
    await expect(page.getByText('cara@example.com')).toBeVisible();

    await page.getByPlaceholder('Search...').fill('admins');
    await expect(page.getByText('bob@example.com')).toBeVisible();
    await expect(page.getByText('cara@example.com')).toBeHidden();
  });

  test('links rows to their detail page', async ({ page }) => {
    await gotoReady(page, '/users');

    await row(page, 'bob@example.com').getByRole('link').click();
    await expect(page).toHaveURL(/\/users\/user-1$/);
  });

  test('deletes a user from the list', async ({ network, page }) => {
    let body = undefined as unknown;
    network.use(
      gen.handleDeleteUser(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await gotoReady(page, '/users');

    await row(page, 'cara@example.com').getByRole('button').click();
    const dialog = page.getByRole('dialog');
    await expect(
      dialog.getByText('Do you really want to delete the user Cara User?')
    ).toBeVisible();
    await dialog.getByRole('button', { name: 'Delete' }).click();

    await expect(
      page.getByText('User Cara User deleted successfully')
    ).toBeVisible();
    expect(body).toEqual({ uuid: 'user-2' });
  });

  test('surfaces a failed delete', async ({ network, page }) => {
    network.use(gen.handleDeleteUser(() => fail(500)));
    await gotoReady(page, '/users');

    await row(page, 'cara@example.com').getByRole('button').click();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Delete' })
      .click();

    await expect(page.getByText('Failed to delete user')).toBeVisible();
  });

  test('disables write actions without user:edit', async ({
    context,
    page
  }) => {
    await setupSession(context, 'readonly');
    await gotoReady(page, '/users');

    await expect(page.getByText('bob@example.com')).toBeVisible();
    await expect(page.getByRole('link', { name: 'Create' })).toHaveAttribute(
      'aria-disabled',
      'true'
    );
    const bob = row(page, 'bob@example.com');
    await expect(bob.getByRole('button')).toBeDisabled();
    await expect(bob.getByRole('link')).toHaveAttribute(
      'aria-disabled',
      'true'
    );
  });
});

test.describe('user create', () => {
  test('creates a user and opens its detail page', async ({
    network,
    page
  }) => {
    let body = undefined as Record<string, unknown> | undefined;
    network.use(
      gen.handleCreateUser(async ({ request }) => {
        body = await request.json();
        return json({ uuid: 'user-new' });
      })
    );
    await gotoReady(page, '/users/create');
    await expectNoHorizontalOverflow(page);
    // With mail configured the user sets their own password via email.
    await expect(page.getByPlaceholder('Enter password')).toHaveCount(0);

    await page.getByPlaceholder('Enter user name').fill('New Person');
    await page.getByPlaceholder('Enter email').fill('new@example.com');
    await page.getByRole('button', { name: 'Create' }).click();

    await expect(page).toHaveURL(/\/users\/user-new$/);
    expect(body).toMatchObject({
      email: 'new@example.com',
      name: 'New Person'
    });
  });

  test('validates name and email', async ({ page }) => {
    await gotoReady(page, '/users/create');

    await page.getByPlaceholder('Enter email').fill('not-an-email');
    await page.getByRole('button', { name: 'Create' }).click();

    await expect(page.getByText('Invalid email address')).toBeVisible();
    await expect(page).toHaveURL(/\/users\/create$/);
  });

  for (const [status, message] of [
    [409, 'A user with this email already exists.'],
    [500, 'Error creating user.']
  ] as const) {
    test(`surfaces a ${status}`, async ({ network, page }) => {
      network.use(gen.handleCreateUser(() => fail(status)));
      await gotoReady(page, '/users/create');

      await page.getByPlaceholder('Enter user name').fill('New Person');
      await page.getByPlaceholder('Enter email').fill('new@example.com');
      await page.getByRole('button', { name: 'Create' }).click();

      await expect(page.getByText(message)).toBeVisible();
      await expect(page).toHaveURL(/\/users\/create$/);
    });
  }

  test('cancel returns to the list', async ({ page }) => {
    await gotoReady(page, '/users/create');
    await page.getByRole('link', { name: 'Cancel' }).click();
    await expect(page).toHaveURL(/\/users$/);
  });

  test.describe('without mail', () => {
    test.beforeEach(async ({ context }) => seedMailInactive(context));

    test('requires an initial password of 6+ characters', async ({
      network,
      page
    }) => {
      let called = false;
      network.use(
        gen.handleCreateUser(() => {
          called = true;
          return json({ uuid: 'user-new' });
        })
      );
      await gotoReady(page, '/users/create');

      await page.getByPlaceholder('Enter user name').fill('New Person');
      await page.getByPlaceholder('Enter email').fill('new@example.com');
      await page.getByPlaceholder('Enter password').fill('123');
      await page.getByRole('button', { name: 'Create' }).click();

      await expect(
        page.getByText('Password must be at least 6 characters long.')
      ).toBeVisible();
      expect(called).toBe(false);
    });

    test('sends an encrypted password', async ({ network, page }) => {
      let body = undefined as Record<string, unknown> | undefined;
      network.use(
        gen.handleCreateUser(async ({ request }) => {
          body = await request.json();
          return json({ uuid: 'user-new' });
        })
      );
      await gotoReady(page, '/users/create');

      await page.getByPlaceholder('Enter user name').fill('New Person');
      await page.getByPlaceholder('Enter email').fill('new@example.com');
      await page.getByPlaceholder('Enter password').fill('secret123');
      await page.getByRole('button', { name: 'Create' }).click();

      await expect(page).toHaveURL(/\/users\/user-new$/);
      expect(body?.password).not.toBe('secret123');
    });
  });
});

test.describe('user detail', () => {
  test('renders the user settings', async ({ page }) => {
    await openDetail(page);

    await expect(page.getByRole('heading', { name: /User:/ })).toContainText(
      'Bob User'
    );
    await expect(page.getByPlaceholder('mail@example.com')).toHaveValue(
      'bob@example.com'
    );
    await expect(page.getByRole('combobox').first()).toContainText('Admins');
    await expectNoHorizontalOverflow(page);
  });

  test('saves name, groups and cache access', async ({ network, page }) => {
    let body = undefined as Record<string, unknown> | undefined;
    network.use(
      gen.handleEditUser(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await openDetail(page);

    await page.getByPlaceholder('Enter user name').fill('Bobby');
    await page.getByRole('button', { name: 'Save Changes' }).click();

    await expect(
      page.getByText('User Bob User updated successfully')
    ).toBeVisible();
    expect(body).toEqual({
      caches: userDetails.caches,
      groups: ['group-admins'],
      name: 'Bobby',
      uuid: 'user-1'
    });
  });

  test('removes a group membership', async ({ network, page }) => {
    let body = undefined as { groups?: string[] } | undefined;
    network.use(
      gen.handleEditUser(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await openDetail(page);

    await page.getByRole('combobox').first().click();
    await page.getByRole('option', { name: 'Admins' }).click();
    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'Save Changes' }).click();

    await expect(
      page.getByText('User Bob User updated successfully')
    ).toBeVisible();
    expect(body?.groups).toEqual([]);
  });

  test('requires a name', async ({ page }) => {
    await openDetail(page);

    await page.getByPlaceholder('Enter user name').fill('');
    await page.getByRole('button', { name: 'Save Changes' }).click();

    await expect(page.getByText('User name is required')).toBeVisible();
  });

  for (const [status, message] of [
    [409, 'Cannot remove the last user from the admin group'],
    [403, 'Cannot assign permissions that you do not have'],
    [500, 'Failed to update user']
  ] as const) {
    test(`surfaces a ${status} on save`, async ({ network, page }) => {
      network.use(gen.handleEditUser(() => fail(status)));
      await openDetail(page);

      await page.getByRole('button', { name: 'Save Changes' }).click();

      await expect(page.getByText(message)).toBeVisible();
    });
  }

  test('edits cache access when the user may edit caches', async ({
    network,
    page
  }) => {
    let body = undefined as { caches?: unknown[] } | undefined;
    network.use(
      // Must precede the detail override: `/api/user/management/:uuid` would
      // Otherwise also match `/api/user/management/caches`.
      gen.handleListCachesSimple(() =>
        json([
          { name: 'main', uuid: 'cache-main' },
          { name: 'private', uuid: 'cache-private' }
        ])
      ),
      gen.handleUserInfoDetail(() =>
        json({ ...userDetails, permissions: [Permission.CACHE_EDIT] })
      ),
      gen.handleEditUser(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await openDetail(page);
    const access = cacheAccess(page);

    // Upgrade the existing mapping to Edit.
    await access.getByRole('button', { name: 'View' }).click();
    await page.getByRole('option', { name: 'Edit' }).click();
    // Add a mapping for the remaining cache (defaults to View).
    await access.getByRole('button').last().click();
    await expect(access.getByRole('combobox').nth(1)).toContainText('private');

    await page.getByRole('button', { name: 'Save Changes' }).click();
    await expect(
      page.getByText('User Bob User updated successfully')
    ).toBeVisible();
    expect(body?.caches).toEqual([
      { access_type: 'Edit', name: 'main', uuid: 'cache-main' },
      { access_type: 'View', name: 'private', uuid: 'cache-private' }
    ]);
  });

  test('removes a cache mapping', async ({ network, page }) => {
    let body = undefined as { caches?: unknown[] } | undefined;
    network.use(
      gen.handleListCachesSimple(() => json(simpleCaches.default)),
      gen.handleUserInfoDetail(() =>
        json({ ...userDetails, permissions: [Permission.CACHE_EDIT] })
      ),
      gen.handleEditUser(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await openDetail(page);

    await cacheAccess(page).getByRole('button').last().click();
    await expect(cacheAccess(page).getByRole('combobox')).toHaveCount(0);
    await page.getByRole('button', { name: 'Save Changes' }).click();

    await expect(
      page.getByText('User Bob User updated successfully')
    ).toBeVisible();
    expect(body?.caches).toEqual([]);
  });

  test('resets the avatar', async ({ network, page }) => {
    let body = undefined as unknown;
    network.use(
      gen.handleResetUserAvatar(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await openDetail(page);

    await page.getByRole('button', { name: 'Reset Avatar' }).click();

    await expect(page.getByText('Avatar reset successfully')).toBeVisible();
    expect(body).toEqual({ uuid: 'user-1' });
  });

  test('surfaces a failed avatar reset', async ({ network, page }) => {
    network.use(gen.handleResetUserAvatar(() => fail(500)));
    await openDetail(page);

    await page.getByRole('button', { name: 'Reset Avatar' }).click();

    await expect(page.getByText('Failed to reset avatar')).toBeVisible();
  });

  test('resets the password', async ({ network, page }) => {
    let body = undefined as Record<string, unknown> | undefined;
    network.use(
      gen.handleResetUserPassword(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await openDetail(page);

    await page.getByRole('button', { name: 'Reset Password' }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByPlaceholder('Enter new password').fill('secret123');
    await dialog.getByRole('button', { name: 'Reset' }).click();

    await expect(
      page.getByText('Password for user Bob User reset successfully')
    ).toBeVisible();
    expect(body?.uuid).toBe('user-1');
    expect(body?.new_password).not.toBe('secret123');
  });

  test('requires a 6+ character reset password', async ({ page }) => {
    await openDetail(page);

    await page.getByRole('button', { name: 'Reset Password' }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByPlaceholder('Enter new password').fill('123');
    await dialog.getByRole('button', { name: 'Reset' }).click();

    await expect(
      dialog.getByText('Password must be at least 6 characters long')
    ).toBeVisible();
  });

  for (const [status, message] of [
    [403, 'You do not have permission to reset this password'],
    [500, 'Failed to reset password']
  ] as const) {
    test(`surfaces a ${status} on password reset`, async ({
      network,
      page
    }) => {
      network.use(gen.handleResetUserPassword(() => fail(status)));
      await openDetail(page);

      await page.getByRole('button', { name: 'Reset Password' }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByPlaceholder('Enter new password').fill('secret123');
      await dialog.getByRole('button', { name: 'Reset' }).click();

      await expect(page.getByText(message)).toBeVisible();
    });
  }

  test('changes the email', async ({ network, page }) => {
    let body = undefined as unknown;
    network.use(
      gen.handleChangeUserEmail(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await openDetail(page);

    await page.getByRole('button', { name: 'Change Email' }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByPlaceholder('mail@example.com').fill('nope');
    await dialog.getByRole('button', { exact: true, name: 'Change' }).click();
    await expect(dialog.getByText('Invalid email address')).toBeVisible();

    await dialog.getByPlaceholder('mail@example.com').fill('bob@new.example');
    await dialog.getByRole('button', { exact: true, name: 'Change' }).click();

    await expect(
      page.getByText('Email for user Bob User changed successfully')
    ).toBeVisible();
    expect(body).toEqual({ new_email: 'bob@new.example', uuid: 'user-1' });
  });

  for (const [status, message] of [
    [409, 'Email already in use'],
    [403, 'You do not have permission to change this email'],
    [500, 'Failed to change email']
  ] as const) {
    test(`surfaces a ${status} on email change`, async ({ network, page }) => {
      network.use(gen.handleChangeUserEmail(() => fail(status)));
      await openDetail(page);

      await page.getByRole('button', { name: 'Change Email' }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByPlaceholder('mail@example.com').fill('bob@new.example');
      await dialog.getByRole('button', { exact: true, name: 'Change' }).click();

      await expect(page.getByText(message)).toBeVisible();
    });
  }

  test('converts an OIDC user to a local user', async ({ network, page }) => {
    let body = undefined as Record<string, unknown> | undefined;
    network.use(
      gen.handleListCachesSimple(() => json(simpleCaches.default)),
      gen.handleUserInfoDetail(() => json({ ...userDetails, oidc_user: true })),
      gen.handleConvertOidcUser(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await openDetail(page);

    // OIDC users have no local password or email to manage.
    await expect(
      page.getByRole('button', { name: 'Reset Password' })
    ).toHaveCount(0);
    await expect(
      page.getByRole('button', { name: 'Change Email' })
    ).toHaveCount(0);

    await page.getByRole('button', { name: 'Convert to Local User' }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByPlaceholder('Enter new password').fill('secret123');
    await dialog.getByRole('button', { name: 'Convert' }).click();

    await expect(
      page.getByText('User Bob User converted to local user successfully')
    ).toBeVisible();
    expect(body?.uuid).toBe('user-1');
  });

  for (const [status, message] of [
    [403, 'You do not have permission to convert this user'],
    [500, 'Failed to convert user']
  ] as const) {
    test(`surfaces a ${status} on convert`, async ({ network, page }) => {
      network.use(
        gen.handleListCachesSimple(() => json(simpleCaches.default)),
        gen.handleUserInfoDetail(() =>
          json({ ...userDetails, oidc_user: true })
        ),
        gen.handleConvertOidcUser(() => fail(status))
      );
      await openDetail(page);

      await page.getByRole('button', { name: 'Convert to Local User' }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByPlaceholder('Enter new password').fill('secret123');
      await dialog.getByRole('button', { name: 'Convert' }).click();

      await expect(page.getByText(message)).toBeVisible();
    });
  }

  test('deletes the user and returns to the list', async ({
    network,
    page
  }) => {
    let body = undefined as unknown;
    network.use(
      gen.handleDeleteUser(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await openDetail(page);

    await page.getByRole('button', { name: 'Delete' }).click();
    const dialog = page.getByRole('dialog');
    await expect(
      dialog.getByText('Do you really want to delete the user Bob User?')
    ).toBeVisible();
    await dialog.getByRole('button', { name: 'Delete' }).click();

    await expect(page).toHaveURL(/\/users$/);
    expect(body).toEqual({ uuid: 'user-1' });
  });

  for (const [status, message] of [
    [409, 'Cannot delete the last user from the admin group'],
    [403, 'You do not have permission to delete this user'],
    [500, 'Failed to delete user']
  ] as const) {
    test(`surfaces a ${status} on delete`, async ({ network, page }) => {
      network.use(gen.handleDeleteUser(() => fail(status)));
      await openDetail(page);

      await page.getByRole('button', { name: 'Delete' }).click();
      await page
        .getByRole('dialog')
        .getByRole('button', { name: 'Delete' })
        .click();

      await expect(page.getByText(message)).toBeVisible();
      await expect(page).toHaveURL(/\/users\/user-1$/);
    });
  }

  for (const [status, message] of [
    [404, 'User not found'],
    [500, 'Failed to load user']
  ] as const) {
    test(`redirects to the list on a ${status}`, async ({ network, page }) => {
      network.use(
        gen.handleListCachesSimple(() => json(simpleCaches.default)),
        gen.handleUserInfoDetail(() => fail(status))
      );
      await gotoReady(page, '/users/user-1');

      await expect(page).toHaveURL(/\/users$/);
      await expect(page.getByText(message)).toBeVisible();
    });
  }

  test('is read-only without user:edit', async ({ context, page }) => {
    await setupSession(context, 'readonly');
    await openDetail(page);

    await expect(page.getByPlaceholder('Enter user name')).toBeDisabled();
    await expect(page.getByRole('button', { name: 'Delete' })).toBeDisabled();
    await expect(
      page.getByRole('button', { name: 'Reset Avatar' })
    ).toBeDisabled();
    await expect(
      page.getByRole('button', { name: 'Save Changes' })
    ).toBeDisabled();
    // The target holds permissions the viewer lacks: no special edits.
    await expect(
      page.getByRole('button', { name: 'Reset Password' })
    ).toHaveCount(0);
    await expect(
      page.getByRole('button', { name: 'Change Email' })
    ).toHaveCount(0);
  });

  test('back button returns to the list', async ({ page }) => {
    await openDetail(page);
    await page.locator('a[href="/users"]').last().click();
    await expect(page).toHaveURL(/\/users$/);
  });
});
