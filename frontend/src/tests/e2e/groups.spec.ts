import { type Page, expect } from '@playwright/test';
import * as gen from '$lib/client/msw.gen';
import { fail, json, test } from '$test_helpers/e2e-fixture';
import { setupSession } from '$test_helpers/session';
import { expectNoHorizontalOverflow, gotoReady } from '$test_helpers/layout';
import { simpleCaches, simpleUsers } from '$mocks/e2e/data';

const row = (page: Page, text: string) =>
  page.getByRole('row').filter({ hasText: text });

const cacheAccess = (page: Page) =>
  page.locator('h5:has-text("Cache Access") + div');

const openDetail = async (page: Page, uuid: string, name: string) => {
  await gotoReady(page, `/groups/${uuid}`);
  await expect(page.getByPlaceholder('Enter group name')).toHaveValue(name);
};

/** Captures the body of the next `editGroup` call. */
const captureEdit = () => {
  const seen: { body?: Record<string, unknown> } = {};
  const handler = gen.handleEditGroup(async ({ request }) => {
    seen.body = await request.json();
    return json({});
  });
  return { handler, seen };
};

test.beforeEach(async ({ context }) => setupSession(context));

test.describe('groups list', () => {
  test('lists groups with permissions and members', async ({ page }) => {
    await gotoReady(page, '/groups');

    await expect(page.getByRole('heading', { name: 'Groups' })).toBeVisible();
    await expect(row(page, 'group-admins')).toContainText(
      'user:view, group:view'
    );
    await expect(row(page, 'group-admins')).toContainText('Bob User');
    await expect(row(page, 'group-staff')).toContainText('-');
    await expectNoHorizontalOverflow(page);
  });

  test('protects the admin group from deletion', async ({ page }) => {
    await gotoReady(page, '/groups');

    await expect(row(page, 'group-admins').getByRole('button')).toBeDisabled();
    await expect(row(page, 'group-staff').getByRole('button')).toBeEnabled();
  });

  test('shows an empty state with no groups', async ({ context, page }) => {
    await setupSession(context, 'empty');
    await gotoReady(page, '/groups');

    await expect(page.getByText('No results.')).toBeVisible();
  });

  test('shows an empty table when the list fails to load', async ({
    network,
    page
  }) => {
    network.use(gen.handleListGroups(() => fail(500)));
    await gotoReady(page, '/groups');

    await expect(page.getByText('No results.')).toBeVisible();
  });

  test('filters via the search input', async ({ page }) => {
    await gotoReady(page, '/groups');
    await expect(row(page, 'group-staff')).toBeVisible();

    await page.getByPlaceholder('Search...').fill('staff');
    await expect(row(page, 'group-admins')).toHaveCount(0);
    await expect(row(page, 'group-staff')).toBeVisible();
  });

  // Regression: member objects were searched as "[object Object]".
  test('finds groups by member name', async ({ page }) => {
    await gotoReady(page, '/groups');
    await expect(row(page, 'group-staff')).toBeVisible();

    await page.getByPlaceholder('Search...').fill('bob user');
    await expect(row(page, 'group-staff')).toHaveCount(0);
    await expect(row(page, 'group-admins')).toBeVisible();
  });

  test('deletes a group from the list', async ({ network, page }) => {
    let body = undefined as unknown;
    network.use(
      gen.handleDeleteGroup(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await gotoReady(page, '/groups');

    await row(page, 'group-staff').getByRole('button').click();
    const dialog = page.getByRole('dialog');
    await expect(
      dialog.getByText('Do you really want to delete the group Staff?')
    ).toBeVisible();
    await dialog.getByRole('button', { name: 'Delete' }).click();

    await expect(
      page.getByText('Group Staff deleted successfully')
    ).toBeVisible();
    expect(body).toEqual({ uuid: 'group-staff' });
  });

  test('surfaces a failed delete', async ({ network, page }) => {
    network.use(gen.handleDeleteGroup(() => fail(500)));
    await gotoReady(page, '/groups');

    await row(page, 'group-staff').getByRole('button').click();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Delete' })
      .click();

    await expect(page.getByText('Failed to delete group')).toBeVisible();
  });

  test('disables write actions without group:edit', async ({
    context,
    page
  }) => {
    await setupSession(context, 'readonly');
    await gotoReady(page, '/groups');

    await expect(row(page, 'group-staff')).toBeVisible();
    await expect(page.getByRole('link', { name: 'Create' })).toHaveAttribute(
      'aria-disabled',
      'true'
    );
    for (const id of ['group-admins', 'group-staff']) {
      // oxlint-disable-next-line no-await-in-loop
      await expect(row(page, id).getByRole('button')).toBeDisabled();
      // oxlint-disable-next-line no-await-in-loop
      await expect(row(page, id).getByRole('link')).toHaveAttribute(
        'aria-disabled',
        'true'
      );
    }
  });
});

test.describe('group create', () => {
  test('creates a group and opens its detail page', async ({
    network,
    page
  }) => {
    let body = undefined as unknown;
    network.use(
      gen.handleCreateGroup(async ({ request }) => {
        body = await request.json();
        return json({ uuid: 'group-new' });
      })
    );
    await gotoReady(page, '/groups/create');
    await expectNoHorizontalOverflow(page);

    await page.getByPlaceholder('Enter group name').fill('Engineering');
    await page.getByRole('button', { name: 'Create' }).click();

    await expect(page.getByText('Group created successfully.')).toBeVisible();
    await expect(page).toHaveURL(/\/groups\/group-new$/);
    expect(body).toEqual({ name: 'Engineering' });
  });

  test('rejects an empty name', async ({ network, page }) => {
    let called = false;
    network.use(
      gen.handleCreateGroup(() => {
        called = true;
        return json({ uuid: 'group-new' });
      })
    );
    await gotoReady(page, '/groups/create');

    await page.getByPlaceholder('Enter group name').fill('x');
    await page.getByPlaceholder('Enter group name').fill('');
    await page.getByRole('button', { name: 'Create' }).click();

    await expect(page.getByText('Name is required')).toBeVisible();
    expect(called).toBe(false);
  });

  for (const [status, message] of [
    [409, 'A group with this name already exists.'],
    [500, 'Error creating group.']
  ] as const) {
    test(`surfaces a ${status}`, async ({ network, page }) => {
      network.use(gen.handleCreateGroup(() => fail(status)));
      await gotoReady(page, '/groups/create');

      await page.getByPlaceholder('Enter group name').fill('Engineering');
      await page.getByRole('button', { name: 'Create' }).click();

      await expect(page.getByText(message)).toBeVisible();
      await expect(page).toHaveURL(/\/groups\/create$/);
    });
  }
});

test.describe('group detail (admin group)', () => {
  test('hides the permission matrix and blocks deletion', async ({ page }) => {
    await openDetail(page, 'group-admins', 'Admins');

    await expect(page.getByRole('heading', { name: /Group:/ })).toContainText(
      'Admins'
    );
    await expect(
      page.getByRole('heading', { name: 'Permissions' })
    ).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Delete' })).toBeDisabled();
    await expectNoHorizontalOverflow(page);
  });

  test('keeps the admin permissions when saving', async ({ network, page }) => {
    const { handler, seen } = captureEdit();
    network.use(handler);
    await openDetail(page, 'group-admins', 'Admins');

    await page.getByPlaceholder('Enter group name').fill('Administrators');
    await page.getByRole('button', { name: 'Save Changes' }).click();

    await expect(
      page.getByText('Group Admins updated successfully')
    ).toBeVisible();
    expect(seen.body).toEqual({
      caches: [{ access_type: 'Edit', name: 'main', uuid: 'cache-main' }],
      name: 'Administrators',
      permissions: ['user:view'],
      users: ['user-1'],
      uuid: 'group-admins'
    });
  });

  for (const [status, message] of [
    [409, 'This group name is already in use'],
    [406, 'Admin group must have at least 1 user'],
    [500, 'Failed to update group']
  ] as const) {
    test(`surfaces a ${status} on save`, async ({ network, page }) => {
      network.use(gen.handleEditGroup(() => fail(status)));
      await openDetail(page, 'group-admins', 'Admins');

      await page.getByRole('button', { name: 'Save Changes' }).click();

      await expect(page.getByText(message)).toBeVisible();
    });
  }
});

test.describe('group detail (regular group)', () => {
  test('grants permissions, members and cache access', async ({
    network,
    page
  }) => {
    const { handler, seen } = captureEdit();
    network.use(handler);
    await openDetail(page, 'group-staff', 'Staff');

    await expect(
      page.getByRole('heading', { name: 'Permissions' })
    ).toBeVisible();
    // Checkboxes sit inside a scroll area; dispatch the click directly so
    // Firefox doesn't stall on pointer actionability.
    await page
      .getByRole('checkbox', { name: 'View Settings' })
      .dispatchEvent('click');
    await page
      .getByRole('checkbox', { name: 'Create Caches' })
      .dispatchEvent('click');
    await expect(
      page.getByRole('checkbox', { name: 'View Settings' })
    ).toBeChecked();

    await page.getByRole('combobox').first().click();
    await page.getByRole('option', { name: 'Bob User' }).click();
    await page.keyboard.press('Escape');

    await cacheAccess(page).getByRole('button').last().click();
    await expect(cacheAccess(page).getByRole('combobox')).toContainText('main');

    await page
      .getByRole('button', { name: 'Save Changes' })
      .dispatchEvent('click');

    await expect(
      page.getByText('Group Staff updated successfully')
    ).toBeVisible();
    expect(seen.body).toMatchObject({
      caches: [{ access_type: 'View', name: 'main', uuid: 'cache-main' }],
      name: 'Staff',
      users: ['user-1'],
      uuid: 'group-staff'
    });
    expect(
      [...((seen.body?.permissions as string[]) ?? [])].toSorted()
    ).toEqual(['cache:create', 'settings:view']);
  });

  test('requires a name', async ({ page }) => {
    await openDetail(page, 'group-staff', 'Staff');

    await page.getByPlaceholder('Enter group name').fill('');
    await page
      .getByRole('button', { name: 'Save Changes' })
      .dispatchEvent('click');

    await expect(page.getByText('Group name is required')).toBeVisible();
  });

  test('deletes the group and returns to the list', async ({
    network,
    page
  }) => {
    let body = undefined as unknown;
    network.use(
      gen.handleDeleteGroup(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await openDetail(page, 'group-staff', 'Staff');

    await page.getByRole('button', { name: 'Delete' }).click();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Delete' })
      .click();

    await expect(page).toHaveURL(/\/groups$/);
    expect(body).toEqual({ uuid: 'group-staff' });
  });

  test('surfaces a failed delete', async ({ network, page }) => {
    network.use(gen.handleDeleteGroup(() => fail(500)));
    await openDetail(page, 'group-staff', 'Staff');

    await page.getByRole('button', { name: 'Delete' }).click();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Delete' })
      .click();

    await expect(page.getByText('Failed to delete group')).toBeVisible();
    await expect(page).toHaveURL(/\/groups\/group-staff$/);
  });

  test('is read-only without group:edit', async ({ context, page }) => {
    await setupSession(context, 'readonly');
    await openDetail(page, 'group-staff', 'Staff');

    await expect(page.getByPlaceholder('Enter group name')).toBeDisabled();
    await expect(page.getByRole('button', { name: 'Delete' })).toBeDisabled();
    // Only permissions the viewer holds are offered; it holds none.
    await expect(page.getByRole('checkbox')).toHaveCount(0);
    // Regression: Save and the member picker stayed enabled.
    await expect(
      page.getByRole('button', { name: 'Save Changes' })
    ).toBeDisabled();
    await expect(page.getByRole('combobox').first()).toBeDisabled();
    // Cache access requires cache:edit.
    await expect(cacheAccess(page).getByRole('button')).toBeDisabled();
  });

  for (const [status, message] of [
    [404, 'Group not found'],
    [500, 'Failed to load group']
  ] as const) {
    test(`redirects to the list on a ${status}`, async ({ network, page }) => {
      network.use(
        // `/api/group/:uuid` would otherwise also catch these two.
        gen.handleListUsersSimple(() => json(simpleUsers.default)),
        gen.handleListCachesSimpleGroup(() => json(simpleCaches.default)),
        gen.handleGroupInfo(() => fail(status))
      );
      await gotoReady(page, '/groups/group-staff');

      await expect(page).toHaveURL(/\/groups$/);
      await expect(page.getByText(message)).toBeVisible();
    });
  }
});
