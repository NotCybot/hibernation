import { expect } from '@playwright/test';
import * as gen from '$lib/client/msw.gen';
import { fail, json, test } from '$test_helpers/e2e-fixture';
import { setupSession } from '$test_helpers/session';
import { expectNoHorizontalOverflow, gotoReady } from '$test_helpers/layout';
import { mailSettings, userSettings } from '$mocks/e2e/data';

const smtpOn = {
  from_env: [] as string[],
  settings: {
    smtp_enabled: true,
    smtp_from_address: 'no-reply@example.com',
    smtp_from_name: 'Hibernation',
    smtp_port: 587,
    smtp_server: 'mail.example.com',
    smtp_use_tls: true,
    smtp_username: 'mailer'
  }
};

test.beforeEach(async ({ context }) => setupSession(context));

test('/settings forwards to the user tab', async ({ page }) => {
  await gotoReady(page, '/settings');
  await expect(page).toHaveURL(/\/settings\/user$/);
});

test.describe('user settings', () => {
  test('shows the OIDC callback url', async ({ page }) => {
    await gotoReady(page, '/settings/user');

    await expect(
      page.getByRole('heading', { name: 'User Settings' })
    ).toBeVisible();
    await expect(
      page.getByText('https://hibernation.example/api/auth/oidc/callback')
    ).toBeVisible();
    await expectNoHorizontalOverflow(page);
  });

  test('saves the settings with the entered values', async ({
    network,
    page
  }) => {
    let body = undefined as Record<string, unknown> | undefined;
    network.use(
      gen.handleSaveUserSettings(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await gotoReady(page, '/settings/user');

    await page
      .getByRole('switch', { name: 'Enable SSO via OpenID Connect' })
      .click();
    await page
      .getByPlaceholder('https://accounts.example.com')
      .fill('https://idp.example');
    await page.getByPlaceholder('your-client-id').fill('hibernation');
    await page.getByPlaceholder('groups').fill('');
    await page.getByRole('button', { name: 'Save Changes' }).click();

    await expect(
      page.getByText('User settings saved successfully')
    ).toBeVisible();
    expect(body).toMatchObject({
      oidc_client_id: 'hibernation',
      oidc_enabled: true,
      oidc_issuer: 'https://idp.example'
    });
    // An empty group claim is sent as "unset", not as an empty string.
    expect(body).not.toHaveProperty('oidc_group_claim');
  });

  test('requires issuer and client id when OIDC is enabled', async ({
    page
  }) => {
    await gotoReady(page, '/settings/user');

    await page
      .getByRole('switch', { name: 'Enable SSO via OpenID Connect' })
      .click();
    await page.getByRole('button', { name: 'Save Changes' }).click();

    await expect(
      page.getByText('This field is required when OIDC is enabled.')
    ).toHaveCount(2);
  });

  test('flags an invalid issuer (406)', async ({ network, page }) => {
    network.use(gen.handleSaveUserSettings(() => fail(406)));
    await gotoReady(page, '/settings/user');
    await expect(
      page.getByRole('heading', { name: 'User Settings' })
    ).toBeVisible();

    await page.getByRole('button', { name: 'Save Changes' }).click();

    await expect(
      page.getByText(
        'Invalid OIDC configuration URL. Check the server logs for more information.'
      )
    ).toBeVisible();
  });

  test('surfaces a failed save', async ({ network, page }) => {
    network.use(gen.handleSaveUserSettings(() => fail(500)));
    await gotoReady(page, '/settings/user');
    await expect(
      page.getByRole('heading', { name: 'User Settings' })
    ).toBeVisible();

    await page.getByRole('button', { name: 'Save Changes' }).click();

    await expect(page.getByText('Failed to save user settings')).toBeVisible();
  });

  test('locks values coming from the environment', async ({
    network,
    page
  }) => {
    let body = undefined as Record<string, unknown> | undefined;
    network.use(
      gen.handleGetUserSettings(() =>
        json({
          ...userSettings,
          from_env: ['oidc_client_id'],
          settings: { oidc_client_id: 'env-client' }
        })
      ),
      gen.handleSaveUserSettings(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await gotoReady(page, '/settings/user');

    const clientId = page.getByPlaceholder('your-client-id');
    await expect(clientId).toHaveValue('env-client');
    await expect(clientId).toBeDisabled();
    await expect(
      page.getByText(
        'Values loaded from environment variables cannot be edited here.'
      )
    ).toBeVisible();

    await page.getByRole('button', { name: 'Save Changes' }).click();
    await expect(
      page.getByText('User settings saved successfully')
    ).toBeVisible();
    // Env-provided keys are stripped from the payload.
    expect(body).not.toHaveProperty('oidc_client_id');
  });

  test('is read-only without settings:edit', async ({ context, page }) => {
    await setupSession(context, 'readonly');
    await gotoReady(page, '/settings/user');

    await expect(page.getByPlaceholder('your-client-id')).toBeDisabled();
    await expect(
      page.getByRole('switch', { name: 'Enable SSO via OpenID Connect' })
    ).toBeDisabled();
    await expect(
      page.getByRole('button', { name: 'Save Changes' })
    ).toBeDisabled();
  });
});

test.describe('mail settings', () => {
  test('renders without a test button while SMTP is off', async ({ page }) => {
    await gotoReady(page, '/settings/mail');

    await expect(
      page.getByRole('heading', { name: 'Mail Settings' })
    ).toBeVisible();
    await expect(page.getByPlaceholder('Example App')).toHaveValue(
      mailSettings.settings.smtp_from_name
    );
    await expect(
      page.getByRole('button', { name: 'Send Test Email' })
    ).toHaveCount(0);
    await expectNoHorizontalOverflow(page);
  });

  test('requires the SMTP fields once SMTP is enabled', async ({ page }) => {
    await gotoReady(page, '/settings/mail');
    await expect(page.getByPlaceholder('Example App')).toHaveValue(
      mailSettings.settings.smtp_from_name
    );

    await page.getByRole('switch', { name: 'Enable SMTP' }).click();
    await page.getByRole('button', { name: 'Save Changes' }).click();

    // Server, port, username and from address (from name is prefilled).
    await expect(
      page.getByText('This field is required when SMTP is enabled.')
    ).toHaveCount(4);
  });

  test('saves the SMTP configuration', async ({ network, page }) => {
    let body = undefined as Record<string, unknown> | undefined;
    network.use(
      gen.handleSaveMailSettings(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await gotoReady(page, '/settings/mail');
    await expect(page.getByPlaceholder('Example App')).toHaveValue(
      mailSettings.settings.smtp_from_name
    );

    await page.getByRole('switch', { name: 'Enable SMTP' }).click();
    await page.getByPlaceholder('mail.example.com').fill('smtp.example.com');
    await page.getByPlaceholder('587').fill('465');
    await page.getByPlaceholder('user@example.com').fill('mailer');
    await page.getByPlaceholder('Password').fill('pw');
    await page.getByPlaceholder('no-reply@example.com').fill('ci@example.com');
    await page.getByRole('button', { name: 'Save Changes' }).click();

    await expect(
      page.getByText('Mail settings saved successfully')
    ).toBeVisible();
    expect(body).toMatchObject({
      smtp_enabled: true,
      smtp_from_address: 'ci@example.com',
      smtp_password: 'pw',
      smtp_port: 465,
      smtp_server: 'smtp.example.com',
      smtp_username: 'mailer'
    });
  });

  for (const [status, message] of [
    [406, 'Invalid From Address provided'],
    [400, 'Failed to create SMTP transport with provided settings'],
    [500, 'Failed to save mail settings']
  ] as const) {
    test(`surfaces a ${status} on save`, async ({ network, page }) => {
      network.use(
        gen.handleGetMailSettings(() => json(smtpOn)),
        gen.handleSaveMailSettings(() => fail(status))
      );
      await gotoReady(page, '/settings/mail');
      await expect(page.getByPlaceholder('mail.example.com')).toHaveValue(
        'mail.example.com'
      );

      await page.getByRole('button', { name: 'Save Changes' }).click();

      await expect(page.getByText(message)).toBeVisible();
    });
  }

  for (const [status, message] of [
    [200, 'Test email sent successfully.'],
    [429, 'Rate limit exceeded. Please try again later.'],
    [500, 'Failed to send test email. Check SMTP settings.']
  ] as const) {
    test(`sends a test email (${status})`, async ({ network, page }) => {
      network.use(
        gen.handleGetMailSettings(() => json(smtpOn)),
        gen.handleTestMail(() => (status === 200 ? json({}) : fail(status)))
      );
      await gotoReady(page, '/settings/mail');

      await page.getByRole('button', { name: 'Send Test Email' }).click();

      await expect(page.getByText(message)).toBeVisible();
    });
  }

  test('locks env values and hides the test button when read-only', async ({
    context,
    network,
    page
  }) => {
    await setupSession(context, 'readonly');
    network.use(
      gen.handleGetMailSettings(() =>
        json({ ...smtpOn, from_env: ['smtp_server'] })
      )
    );
    await gotoReady(page, '/settings/mail');

    await expect(page.getByPlaceholder('mail.example.com')).toBeDisabled();
    await expect(page.getByPlaceholder('Example App')).toBeDisabled();
    await expect(
      page.getByRole('button', { name: 'Save Changes' })
    ).toBeDisabled();
    await expect(
      page.getByText(
        'Values loaded from environment variables cannot be edited here.'
      )
    ).toBeVisible();
    await expect(
      page.getByRole('button', { name: 'Send Test Email' })
    ).toHaveCount(0);
  });
});

test.describe('settings navigation', () => {
  test('switches between the user and mail tabs', async ({ page }) => {
    await gotoReady(page, '/settings/user');

    await page.locator('a[href="/settings/mail"]').click();
    await expect(page).toHaveURL(/\/settings\/mail$/);
    await expect(
      page.getByRole('heading', { name: 'Mail Settings' })
    ).toBeVisible();

    await page.locator('a[href="/settings/user"]').click();
    await expect(page).toHaveURL(/\/settings\/user$/);
  });
});
