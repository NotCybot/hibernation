import { type Page, expect } from '@playwright/test';
import { http } from 'msw';
import * as gen from '$lib/client/msw.gen';
import { fail, json, test } from '$test_helpers/e2e-fixture';
import { seedSetupPending, setupSession } from '$test_helpers/session';
import { expectNoHorizontalOverflow, gotoReady } from '$test_helpers/layout';
import { isSetup, oidcSettings } from '$mocks/e2e/data';

const URL = 'http://localhost:4173';

const acceptDisclaimer = async (page: Page) => {
  await page
    .getByRole('checkbox', { name: 'I have read the disclaimer' })
    .click();
  await page.getByRole('button', { name: 'Next' }).click();
  await expect(page.getByText('User Setup')).toBeVisible();
};

const fillAdmin = async (page: Page) => {
  await page.getByPlaceholder('Enter username').fill('admin');
  await page.getByPlaceholder('Enter email').fill('admin@example.com');
  await page.getByPlaceholder('Enter password').fill('secret123');
};

test.beforeEach(async ({ context }) => seedSetupPending(context));

test.describe('setup wizard', () => {
  test('renders the disclaimer with the detected backends', async ({
    page
  }) => {
    await gotoReady(page, '/setup');

    await expect(page.getByText('Database Setup')).toBeVisible();
    await expect(page.getByText('sqlite', { exact: true })).toBeVisible();
    await expect(page.getByText('local', { exact: true })).toBeVisible();
    await expectNoHorizontalOverflow(page);
  });

  test('requires the disclaimer to be accepted', async ({ page }) => {
    await gotoReady(page, '/setup');

    await page.getByRole('button', { name: 'Next' }).click();

    await expect(
      page.getByText('You must accept the disclaimer to proceed.')
    ).toBeVisible();
    await expect(page.getByText('Database Setup')).toBeVisible();
  });

  test('can step back to the disclaimer', async ({ page }) => {
    await gotoReady(page, '/setup');
    await acceptDisclaimer(page);

    await page.getByRole('button', { name: 'Previous' }).click();

    await expect(page.getByText('Database Setup')).toBeVisible();
    await expect(
      page.getByRole('checkbox', { name: 'I have read the disclaimer' })
    ).toBeChecked();
  });

  test('validates the admin user fields', async ({ page }) => {
    await gotoReady(page, '/setup');
    await acceptDisclaimer(page);

    await page.getByPlaceholder('Enter username').fill('ab');
    await page.getByPlaceholder('Enter email').fill('nope');
    await page.getByPlaceholder('Enter password').fill('123');
    await page.getByRole('button', { name: 'Finish' }).click();

    await expect(
      page.getByText('Username must be at least 3 characters long.')
    ).toBeVisible();
    await expect(page.getByText('Invalid email address.')).toBeVisible();
    await expect(
      page.getByText('Password must be at least 6 characters long.')
    ).toBeVisible();
  });

  test('creates the admin user and lands on the overview', async ({
    context,
    network,
    page
  }) => {
    let body = undefined as Record<string, string> | undefined;
    network.use(
      gen.handleCompleteSetup(async ({ request }) => {
        body = await request.json();
        return json({ user: 'user-admin' });
      })
    );
    await gotoReady(page, '/setup');
    await acceptDisclaimer(page);
    await fillAdmin(page);
    // The backend logs the new admin in; mirror its session cookie.
    await context.addCookies([
      { name: 'centaurus_jwt', url: URL, value: 'e2e-token' }
    ]);
    await page.getByRole('button', { name: 'Finish' }).click();

    await expect(page).toHaveURL(`${URL}/`);
    expect(body?.admin_username).toBe('admin');
    expect(body?.admin_email).toBe('admin@example.com');
    expect(body?.admin_password).not.toBe('secret123');
  });

  for (const [status, message] of [
    [409, 'The setup was already completed.'],
    [500, 'The server failed to find the admin group.'],
    [400, 'An unknown error occurred.']
  ] as const) {
    test(`surfaces a ${status} from completing setup`, async ({
      network,
      page
    }) => {
      network.use(gen.handleCompleteSetup(() => fail(status)));
      await gotoReady(page, '/setup');
      await acceptDisclaimer(page);
      await fillAdmin(page);
      await page.getByRole('button', { name: 'Finish' }).click();

      await expect(page.getByText(message)).toBeVisible();
      await expect(page).toHaveURL(/\/setup/);
    });
  }
});

test.describe('setup wizard (OIDC)', () => {
  const openOidc = async (page: Page) => {
    await gotoReady(page, '/setup');
    await acceptDisclaimer(page);
    await page.getByRole('tab', { name: 'OpenID Connect' }).click();
  };

  const fillOidc = async (page: Page) => {
    await page
      .getByPlaceholder('https://accounts.example.com')
      .fill('https://idp.example');
    await page.getByPlaceholder('your-client-id').fill('hibernation');
  };

  test('shows the callback url derived from the site url', async ({ page }) => {
    await openOidc(page);

    await expect(
      page.getByText('https://hibernation.example/api/auth/oidc/callback')
    ).toBeVisible();
  });

  test('initialises OIDC and follows the provider url', async ({
    network,
    page
  }) => {
    let body = undefined as Record<string, unknown> | undefined;
    network.use(
      gen.handleInitOidc(async ({ request }) => {
        body = await request.json();
        return json({});
      }),
      http.get('*/api/auth/oidc/url', () =>
        json({ url: 'https://idp.example/authorize' })
      )
    );
    await page.route('https://idp.example/**', async (route) =>
      route.fulfill({ body: 'idp', contentType: 'text/plain' })
    );
    await openOidc(page);

    await page
      .getByPlaceholder('https://accounts.example.com')
      .fill('https://idp.example');
    await page.getByPlaceholder('your-client-id').fill('hibernation');
    await page.getByPlaceholder('your-client-secret').fill('s3cret');
    await page.getByRole('button', { name: 'Finish' }).click();

    await expect(page).toHaveURL('https://idp.example/authorize');
    expect(body).toMatchObject({
      oidc_client_id: 'hibernation',
      oidc_client_secret: 's3cret',
      oidc_issuer: 'https://idp.example'
    });
  });

  test('flags an invalid issuer (406)', async ({ network, page }) => {
    network.use(gen.handleInitOidc(() => fail(406)));
    await openOidc(page);

    await fillOidc(page);
    await page.getByRole('button', { name: 'Finish' }).click();

    await expect(
      page.getByText(
        'Invalid OIDC configuration URL. Check the server logs for more information.'
      )
    ).toBeVisible();
  });

  test('requires issuer and client id', async ({ network, page }) => {
    let called = false;
    network.use(
      gen.handleInitOidc(() => {
        called = true;
        return json({});
      })
    );
    await openOidc(page);
    await page.getByRole('button', { name: 'Finish' }).click();

    await expect(
      page.getByPlaceholder('https://accounts.example.com')
    ).toHaveAttribute('aria-invalid', 'true');
    expect(called).toBe(false);
  });

  test('surfaces other init failures', async ({ network, page }) => {
    network.use(gen.handleInitOidc(() => fail(500)));
    await openOidc(page);
    await fillOidc(page);
    await page.getByRole('button', { name: 'Finish' }).click();

    await expect(
      page.getByText(
        'An unknown error occurred. Check the server logs for more information.'
      )
    ).toBeVisible();
  });

  test('surfaces a missing provider url', async ({ network, page }) => {
    network.use(
      gen.handleInitOidc(() => json({})),
      http.get('*/api/auth/oidc/url', () => fail(500))
    );
    await openOidc(page);
    await fillOidc(page);
    await page.getByRole('button', { name: 'Finish' }).click();

    await expect(
      page.getByText(
        'Failed to retrieve OIDC URL. Check the server logs for more information.'
      )
    ).toBeVisible();
  });

  test('preselects OIDC when settings were configured before', async ({
    network,
    page
  }) => {
    network.use(
      gen.handleGetOidcSettings(() =>
        json({
          ...oidcSettings,
          from_env: ['oidc_client_id'],
          settings: {
            oidc_client_id: 'env-id',
            oidc_issuer: 'https://idp.example'
          }
        })
      )
    );
    await gotoReady(page, '/setup');

    // The disclaimer is pre-accepted for an already-configured instance.
    await expect(
      page.getByRole('checkbox', { name: 'I have read the disclaimer' })
    ).toBeChecked();
    await page.getByRole('button', { name: 'Next' }).click();

    await expect(
      page.getByRole('tab', { name: 'OpenID Connect' })
    ).toHaveAttribute('data-state', 'active');
    await expect(
      page.getByPlaceholder('https://accounts.example.com')
    ).toHaveValue('https://idp.example');
    // Values coming from the environment are locked.
    await expect(page.getByPlaceholder('your-client-id')).toBeDisabled();
  });
});

test.describe('setup redirects', () => {
  test('/setup forwards to / once the instance is set up', async ({
    context,
    network,
    page
  }) => {
    network.use(gen.handleIsSetup(() => json(isSetup)));
    await setupSession(context);
    await gotoReady(page, '/setup');

    await expect(page).toHaveURL(`${URL}/`);
  });

  test('pending setup forces protected pages to /setup', async ({
    context,
    page
  }) => {
    await setupSession(context);
    await gotoReady(page, '/caches');

    await expect(page).toHaveURL(/\/setup$/);
  });
});
