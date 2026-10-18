import { type Page, expect } from '@playwright/test';
import * as gen from '$lib/client/msw.gen';
import { fail, json, test } from '$test_helpers/e2e-fixture';
import { seedMailInactive, setupSession } from '$test_helpers/session';
import { expectNoHorizontalOverflow, gotoReady } from '$test_helpers/layout';
import { adminUser } from '$mocks/e2e/data';

// 1x1 transparent PNG.
const PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==',
  'base64'
);

// The account buttons sit inside a tooltip trigger (itself a <button>), so
// Target the inner button that carries the click handler / disabled state.
const trigger = (page: Page, name: string) =>
  page.locator('[data-slot="button"]', { hasText: name });

test.beforeEach(async ({ context }) => setupSession(context));

test('/account forwards to the general tab', async ({ page }) => {
  await gotoReady(page, '/account');
  await expect(page).toHaveURL(/\/account\/general$/);
});

test('the sidebar user menu links to the account page', async ({ page }) => {
  await gotoReady(page, '/');
  const toggle = page.getByRole('button', { name: 'Toggle Sidebar' });
  if (await toggle.isVisible()) {
    await toggle.click();
  }

  await page.getByText(adminUser.email).first().click();
  await page.getByRole('menuitem', { name: 'Account' }).click();

  await expect(page).toHaveURL(/\/account\/general$/);
});

test.describe('account general', () => {
  test('prefills and saves the username', async ({ network, page }) => {
    let body = undefined as unknown;
    network.use(
      gen.handleUpdateAccount(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await gotoReady(page, '/account/general');
    await expectNoHorizontalOverflow(page);

    const name = page.getByPlaceholder('Enter your username');
    await expect(name).toHaveValue(adminUser.name);
    await name.fill('Ada Lovelace');
    await page.getByRole('button', { name: 'Save Changes' }).click();

    await expect(
      page.getByText('General settings saved successfully')
    ).toBeVisible();
    expect(body).toEqual({ username: 'Ada Lovelace' });
  });

  test('rejects an empty username', async ({ page }) => {
    await gotoReady(page, '/account/general');
    await expect(page.getByPlaceholder('Enter your username')).toHaveValue(
      adminUser.name
    );

    await page.getByPlaceholder('Enter your username').fill('');
    await page.getByRole('button', { name: 'Save Changes' }).click();

    await expect(page.getByText('Username is required')).toBeVisible();
  });

  test('surfaces a failed save', async ({ network, page }) => {
    network.use(gen.handleUpdateAccount(() => fail(500)));
    await gotoReady(page, '/account/general');
    await expect(page.getByPlaceholder('Enter your username')).toHaveValue(
      adminUser.name
    );

    await page.getByRole('button', { name: 'Save Changes' }).click();

    await expect(
      page.getByText('Failed to save general settings')
    ).toBeVisible();
  });

  test('rejects an unsupported avatar file', async ({ page }) => {
    await gotoReady(page, '/account/general');

    await page.locator('input[type="file"]').setInputFiles({
      buffer: Buffer.from('hello'),
      mimeType: 'text/plain',
      name: 'notes.txt'
    });

    await expect(
      page.getByText('Unsupported file type: text/plain')
    ).toBeVisible();
  });

  for (const [status, message] of [
    [200, 'Avatar updated successfully'],
    [413, 'File too large. Please upload a smaller file.'],
    [429, 'Rate limit exceeded. Please try again later.'],
    [500, 'Failed to update avatar']
  ] as const) {
    test(`uploads a cropped avatar (${status})`, async ({ network, page }) => {
      let body = undefined as { avatar?: string } | undefined;
      network.use(
        gen.handleUpdateAvatar(async ({ request }) => {
          body = await request.json();
          return status === 200 ? json({}) : fail(status);
        })
      );
      await gotoReady(page, '/account/general');

      await page.locator('input[type="file"]').setInputFiles({
        buffer: PNG,
        mimeType: 'image/png',
        name: 'avatar.png'
      });
      await page.getByRole('button', { name: 'Crop' }).click();

      await expect(page.getByText(message)).toBeVisible();
      expect(body?.avatar?.length).toBeGreaterThan(0);
    });
  }
});

test.describe('account password', () => {
  const openDialog = async (page: Page) => {
    await gotoReady(page, '/account/auth');
    await expectNoHorizontalOverflow(page);
    await trigger(page, 'Change Password').click();
    const dialog = page.getByRole('dialog');
    await dialog.getByPlaceholder('Old Password').fill('old-pass');
    await dialog
      .getByPlaceholder('New Password', { exact: true })
      .fill('new-pass');
    return dialog;
  };

  test('changes the password', async ({ network, page }) => {
    let body = undefined as Record<string, string> | undefined;
    network.use(
      gen.handleUpdatePassword(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    const dialog = await openDialog(page);
    await dialog.getByPlaceholder('Confirm New Password').fill('new-pass');
    await dialog.getByRole('button', { name: 'Change Password' }).click();

    await expect(
      page.getByText('Password was changed successfully')
    ).toBeVisible();
    await expect(dialog).toBeHidden();
    expect(Object.keys(body ?? {}).toSorted()).toEqual([
      'new_password',
      'old_password'
    ]);
    expect(body?.new_password).not.toBe('new-pass');
  });

  test('rejects a mismatching confirmation', async ({ network, page }) => {
    let called = false;
    network.use(
      gen.handleUpdatePassword(() => {
        called = true;
        return json({});
      })
    );
    const dialog = await openDialog(page);
    await dialog.getByPlaceholder('Confirm New Password').fill('other');
    await dialog.getByRole('button', { name: 'Change Password' }).click();

    await expect(dialog.getByText('Passwords are not equal')).toBeVisible();
    expect(called).toBe(false);
  });

  test('requires all fields', async ({ page }) => {
    await gotoReady(page, '/account/auth');
    await trigger(page, 'Change Password').click();
    const dialog = page.getByRole('dialog');
    await dialog.getByRole('button', { name: 'Change Password' }).click();

    await expect(dialog.getByText('Old Password is required')).toBeVisible();
    await expect(
      dialog.getByText('Password is required', { exact: true })
    ).toBeVisible();
    await expect(
      dialog.getByText('Password Confirm is required')
    ).toBeVisible();
  });

  test('flags a wrong old password (403)', async ({ network, page }) => {
    network.use(gen.handleUpdatePassword(() => fail(403)));
    const dialog = await openDialog(page);
    await dialog.getByPlaceholder('Confirm New Password').fill('new-pass');
    await dialog.getByRole('button', { name: 'Change Password' }).click();

    await expect(dialog.getByText('Old password is incorrect')).toBeVisible();
  });

  test('surfaces other failures', async ({ network, page }) => {
    network.use(gen.handleUpdatePassword(() => fail(500)));
    const dialog = await openDialog(page);
    await dialog.getByPlaceholder('Confirm New Password').fill('new-pass');
    await dialog.getByRole('button', { name: 'Change Password' }).click();

    await expect(page.getByText('Error while updating password')).toBeVisible();
  });

  test('is disabled for OIDC users', async ({ network, page }) => {
    network.use(gen.handleInfo(() => json({ ...adminUser, oidc_user: true })));
    await gotoReady(page, '/account/auth');

    await expect(trigger(page, 'Change Password')).toBeDisabled();
  });
});

test.describe('account email', () => {
  const startChange = async (page: Page) => {
    await gotoReady(page, '/account/auth');
    await trigger(page, 'Change Email').click();
    const dialog = page.getByRole('dialog');
    await dialog.getByPlaceholder('mail@example.com').fill('ada@new.example');
    await dialog.getByRole('button', { name: 'Change Email' }).click();
    return dialog;
  };

  test('changes the email through both confirmation codes', async ({
    network,
    page
  }) => {
    let start = undefined as unknown;
    let confirm = undefined as unknown;
    network.use(
      gen.handleStartEmailChange(async ({ request }) => {
        start = await request.json();
        return json({});
      }),
      gen.handleConfirmEmailChange(async ({ request }) => {
        confirm = await request.json();
        return json({});
      })
    );
    const dialog = await startChange(page);

    await expect(
      dialog.getByText(`Code from old Email (${adminUser.email})`)
    ).toBeVisible();
    expect(start).toEqual({ new_email: 'ada@new.example' });

    await dialog
      .getByRole('textbox', {
        name: `Code from old Email (${adminUser.email})`
      })
      .fill('111111');
    await dialog
      .getByRole('textbox', { name: 'Code from new Email (ada@new.example)' })
      .fill('222222');
    await dialog.getByRole('button', { name: 'Change Email' }).click();

    await expect(page.getByText('Email change successful')).toBeVisible();
    expect(confirm).toEqual({ new_code: '222222', old_code: '111111' });
  });

  test('flags an email that is already in use', async ({ network, page }) => {
    network.use(gen.handleStartEmailChange(() => fail(409)));
    const dialog = await startChange(page);

    await expect(
      dialog.getByText('This email is already in use.')
    ).toBeVisible();
  });

  test('surfaces a failure to send the codes', async ({ network, page }) => {
    network.use(gen.handleStartEmailChange(() => fail(500)));
    await startChange(page);

    await expect(
      page.getByText('There was an error while sending your emails.')
    ).toBeVisible();
  });

  for (const [status, field, message] of [
    [401, 'new', 'The code you entered is incorrect.'],
    [403, 'old', 'The code you entered is incorrect.'],
    [500, null, 'There was an error while confirming your email change.']
  ] as const) {
    test(`surfaces a ${status} when confirming`, async ({ network, page }) => {
      network.use(
        gen.handleStartEmailChange(() => json({})),
        gen.handleConfirmEmailChange(() => fail(status))
      );
      const dialog = await startChange(page);
      await expect(
        dialog.getByText(`Code from old Email (${adminUser.email})`)
      ).toBeVisible();
      await dialog
        .getByRole('textbox', {
          name: `Code from old Email (${adminUser.email})`
        })
        .fill('111111');
      await dialog
        .getByRole('textbox', { name: 'Code from new Email (ada@new.example)' })
        .fill('222222');
      await dialog.getByRole('button', { name: 'Change Email' }).click();

      // Field errors render inside the dialog; generic ones as a toast.
      await expect(
        (field ? dialog : page).getByText(message).first()
      ).toBeVisible();
    });
  }

  test('requires both 6-digit codes', async ({ network, page }) => {
    let called = false;
    network.use(
      gen.handleStartEmailChange(() => json({})),
      gen.handleConfirmEmailChange(() => {
        called = true;
        return json({});
      })
    );
    const dialog = await startChange(page);
    await expect(
      dialog.getByText(`Code from old Email (${adminUser.email})`)
    ).toBeVisible();
    await dialog
      .getByRole('textbox', {
        name: `Code from old Email (${adminUser.email})`
      })
      .fill('123');
    await dialog.getByRole('button', { name: 'Change Email' }).click();

    await expect(
      dialog.getByText('Code must be 6 characters long')
    ).toHaveCount(2);
    expect(called).toBe(false);
  });

  test('is disabled for OIDC users', async ({ network, page }) => {
    network.use(gen.handleInfo(() => json({ ...adminUser, oidc_user: true })));
    await gotoReady(page, '/account/auth');

    await expect(trigger(page, 'Change Email')).toBeDisabled();
  });

  test('is disabled when mail is not configured', async ({ context, page }) => {
    await seedMailInactive(context);
    await gotoReady(page, '/account/auth');

    await expect(trigger(page, 'Change Email')).toBeDisabled();
  });
});
