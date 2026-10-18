import { expect } from '@playwright/test';
import * as gen from '$lib/client/msw.gen';
import { fail, json, test } from '$test_helpers/e2e-fixture';
import { seedAnonymous } from '$test_helpers/session';
import { expectNoHorizontalOverflow, gotoReady } from '$test_helpers/layout';

test.beforeEach(async ({ context }) => seedAnonymous(context));

test('/password forwards to the forgot form', async ({ page }) => {
  await gotoReady(page, '/password');
  await expect(page).toHaveURL(/\/password\/forgot$/);
});

test('the login page links to the forgot form', async ({ page }) => {
  await gotoReady(page, '/login');
  await page.getByRole('link', { name: 'Forgot your password?' }).click();
  await expect(page).toHaveURL(/\/password\/forgot$/);
});

test.describe('forgot password', () => {
  test('sends a reset link for the entered email', async ({
    network,
    page
  }) => {
    let body = undefined as { email?: string } | undefined;
    network.use(
      gen.handleSendResetLink(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await gotoReady(page, '/password/forgot');
    await expectNoHorizontalOverflow(page);

    await page.getByPlaceholder('mail@example.com').fill('bob@example.com');
    await page.getByRole('button', { name: 'Send Reset Link' }).click();

    await expect(
      page.getByText('Reset link sent to your email address.')
    ).toBeVisible();
    expect(body).toEqual({ email: 'bob@example.com' });
  });

  test('rejects an invalid email', async ({ network, page }) => {
    let called = false;
    network.use(
      gen.handleSendResetLink(() => {
        called = true;
        return json({});
      })
    );
    await gotoReady(page, '/password/forgot');

    await page.getByPlaceholder('mail@example.com').fill('nope');
    await page.getByRole('button', { name: 'Send Reset Link' }).click();

    await expect(page.getByText('Invalid email address')).toBeVisible();
    expect(called).toBe(false);
  });

  for (const [status, message] of [
    [429, 'Rate limit exceeded. Please try again later.'],
    [500, 'Failed to send reset link.']
  ] as const) {
    test(`surfaces a ${status}`, async ({ network, page }) => {
      network.use(gen.handleSendResetLink(() => fail(status)));
      await gotoReady(page, '/password/forgot');

      await page.getByPlaceholder('mail@example.com').fill('bob@example.com');
      await page.getByRole('button', { name: 'Send Reset Link' }).click();

      await expect(page.getByText(message)).toBeVisible();
    });
  }
});

test.describe('reset password', () => {
  test('prefills the token from the query string', async ({ page }) => {
    await gotoReady(page, '/password/reset?token=abc123');

    await expect(page.getByPlaceholder('Enter your token')).toHaveValue(
      'abc123'
    );
    await expectNoHorizontalOverflow(page);
  });

  test('resets the password and returns to login', async ({
    network,
    page
  }) => {
    let body = undefined as
      | { token?: string; new_password?: string }
      | undefined;
    network.use(
      gen.handleResetPassword(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await gotoReady(page, '/password/reset?token=abc123');

    await page.getByPlaceholder('Enter your new password').fill('secret123');
    await page.getByPlaceholder('Confirm your new password').fill('secret123');
    await page.getByRole('button', { name: 'Reset Password' }).click();

    await expect(
      page.getByText(
        'Password has been reset successfully. You can now log in.'
      )
    ).toBeVisible();
    await expect(page).toHaveURL(/\/login/);
    expect(body?.token).toBe('abc123');
    expect(body?.new_password).not.toBe('secret123');
  });

  test('validates token, length and confirmation', async ({ page }) => {
    await gotoReady(page, '/password/reset');

    await page.getByPlaceholder('Enter your new password').fill('123');
    await page.getByPlaceholder('Confirm your new password').fill('456');
    await page.getByRole('button', { name: 'Reset Password' }).click();

    await expect(page.getByText('Token is required')).toBeVisible();
    await expect(
      page.getByText('Password must be at least 6 characters long')
    ).toBeVisible();
    await expect(page.getByText('Passwords do not match')).toBeVisible();
  });

  for (const [status, message] of [
    [429, 'Rate limit exceeded. Please try again later.'],
    [400, 'Failed to reset password.']
  ] as const) {
    test(`surfaces a ${status}`, async ({ network, page }) => {
      network.use(gen.handleResetPassword(() => fail(status)));
      await gotoReady(page, '/password/reset?token=abc123');

      await page.getByPlaceholder('Enter your new password').fill('secret123');
      await page
        .getByPlaceholder('Confirm your new password')
        .fill('secret123');
      await page.getByRole('button', { name: 'Reset Password' }).click();

      await expect(page.getByText(message)).toBeVisible();
      await expect(page).toHaveURL(/\/password\/reset/);
    });
  }
});
