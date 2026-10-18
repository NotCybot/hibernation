import { expect } from '@playwright/test';
import { http } from 'msw';
import * as gen from '$lib/client/msw.gen';
import { fail, json, test } from '$test_helpers/e2e-fixture';
import { seedAnonymous, setupSession } from '$test_helpers/session';
import { expectNoHorizontalOverflow, gotoReady } from '$test_helpers/layout';
import { OIDC_ERRORS } from '$lib/permissions.svelte';
import { authConfig } from '$mocks/e2e/data';

const URL = 'http://localhost:4173';
const oidcConfig = { ...authConfig, sso_type: 'Oidc' };
const oidcUrl = (url?: string) =>
  http.get('*/api/auth/oidc/url', () => (url ? json({ url }) : fail(500)));

test.beforeEach(async ({ context }) => seedAnonymous(context));

test.describe('login form', () => {
  test('renders the form without OIDC when SSO is off', async ({ page }) => {
    await gotoReady(page, '/login');

    await expect(
      page.getByText('Login', { exact: true }).first()
    ).toBeVisible();
    await expect(page.getByPlaceholder('mail@example.com')).toBeVisible();
    await expect(
      page.getByRole('link', { name: 'Forgot your password?' })
    ).toBeAttached();
    await expect(
      page.getByRole('button', { name: 'OIDC Provider' })
    ).toHaveCount(0);
    await expectNoHorizontalOverflow(page);
  });

  test('hides the forgot-password link when mail is disabled', async ({
    network,
    page
  }) => {
    network.use(
      gen.handleAuthConfig(() => json({ ...authConfig, mail_enabled: false }))
    );
    await gotoReady(page, '/login');

    await expect(page.getByPlaceholder('mail@example.com')).toBeVisible();
    await expect(
      page.getByRole('link', { name: 'Forgot your password?' })
    ).toHaveCount(0);
  });

  test('blocks submission with invalid input', async ({ network, page }) => {
    let called = false;
    network.use(
      gen.handleAuthenticate(() => {
        called = true;
        return json({ user: 'user-admin' });
      })
    );
    await gotoReady(page, '/login');

    await page.getByPlaceholder('mail@example.com').fill('not-an-email');
    await page.getByRole('button', { name: 'Login' }).click();

    await expect(page.getByText('Invalid email address')).toBeVisible();
    expect(called).toBe(false);
  });

  test('logs in and follows a safe redirect param', async ({
    context,
    network,
    page
  }) => {
    let body = undefined as { email?: string; password?: string } | undefined;
    network.use(
      gen.handleAuthenticate(async ({ request }) => {
        body = await request.json();
        return json({ user: 'user-admin' });
      })
    );
    await gotoReady(page, '/login?redirect=%2Ftokens');

    await page.getByPlaceholder('mail@example.com').fill('admin@example.com');
    await page.getByPlaceholder('Your password').fill('hunter22');
    // The real backend sets the session cookie on success; mirror that so the
    // Server-side auth guard lets the follow-up navigation through.
    await context.addCookies([
      { name: 'centaurus_jwt', url: URL, value: 'e2e-token' }
    ]);
    await page.getByRole('button', { name: 'Login' }).click();

    await expect(page).toHaveURL(/\/tokens$/);
    expect(body?.email).toBe('admin@example.com');
    // The password is never sent in clear text.
    expect(body?.password).not.toBe('hunter22');
  });

  for (const redirect of ['https://evil.example', '//evil.example', '/login']) {
    test(`ignores unsafe redirect ${redirect}`, async ({
      context,
      network,
      page
    }) => {
      network.use(gen.handleAuthenticate(() => json({ user: 'user-admin' })));
      await gotoReady(page, `/login?redirect=${encodeURIComponent(redirect)}`);

      await page.getByPlaceholder('mail@example.com').fill('admin@example.com');
      await page.getByPlaceholder('Your password').fill('hunter22');
      await context.addCookies([
        { name: 'centaurus_jwt', url: URL, value: 'e2e-token' }
      ]);
      await page.getByRole('button', { name: 'Login' }).click();

      await expect(page).toHaveURL(`${URL}/`);
    });
  }

  for (const [status, message] of [
    [401, 'Invalid email or password.'],
    [429, 'Rate limit exceeded. Please try again later.'],
    [500, 'Login failed. Please try again.']
  ] as const) {
    test(`surfaces a ${status} login failure`, async ({ network, page }) => {
      network.use(gen.handleAuthenticate(() => fail(status)));
      await gotoReady(page, '/login');

      await page.getByPlaceholder('mail@example.com').fill('admin@example.com');
      await page.getByPlaceholder('Your password').fill('wrong');
      await page.getByRole('button', { name: 'Login' }).click();

      await expect(page.getByText(message)).toBeVisible();
      await expect(page).toHaveURL(/\/login/);
      await expect(page.getByRole('button', { name: 'Retry' })).toBeVisible();
    });
  }

  test('redirects an authenticated user away from /login', async ({
    context,
    network,
    page
  }) => {
    await setupSession(context);
    // `beforeEach` seeded an anonymous session; make the token valid again.
    network.use(gen.handleTestToken(() => json({ valid: true })));
    await gotoReady(page, '/login?redirect=%2Fcaches');

    await expect(page).toHaveURL(/\/caches$/);
  });
});

test.describe('OIDC login', () => {
  test('shows the provider button and follows the OIDC url', async ({
    network,
    page
  }) => {
    let redirectTo: string | null = null;
    network.use(
      gen.handleAuthConfig(() => json(oidcConfig)),
      http.get('*/api/auth/oidc/url', ({ request }) => {
        redirectTo = new globalThis.URL(request.url).searchParams.get(
          'redirect_to'
        );
        return json({ url: 'https://idp.example/authorize' });
      })
    );
    await page.route('https://idp.example/**', async (route) =>
      route.fulfill({ body: 'idp', contentType: 'text/plain' })
    );
    await gotoReady(page, '/login?redirect=%2Fusers');

    await page.getByRole('button', { name: 'OIDC Provider' }).click();

    await expect(page).toHaveURL('https://idp.example/authorize');
    expect(redirectTo).toBe('/users');
  });

  test('offers a retry when the OIDC url cannot be fetched', async ({
    network,
    page
  }) => {
    network.use(
      gen.handleAuthConfig(() => json(oidcConfig)),
      oidcUrl()
    );
    await gotoReady(page, '/login');

    await page.getByRole('button', { name: 'OIDC Provider' }).click();
    await expect(page.getByText('Failed to get OIDC URL.')).toBeVisible();

    const retry = page.getByRole('button', { name: 'Retry OIDC' });
    await expect(retry).toBeVisible();

    network.use(oidcUrl('https://idp.example/authorize'));
    await page.route('https://idp.example/**', async (route) =>
      route.fulfill({ body: 'idp', contentType: 'text/plain' })
    );
    await retry.click();
    await expect(page).toHaveURL('https://idp.example/authorize');
  });

  test('redirects instantly when instant_redirect is enabled', async ({
    network,
    page
  }) => {
    network.use(
      gen.handleAuthConfig(() =>
        json({ ...oidcConfig, instant_redirect: true })
      ),
      oidcUrl('https://idp.example/authorize')
    );
    await page.route('https://idp.example/**', async (route) =>
      route.fulfill({ body: 'idp', contentType: 'text/plain' })
    );
    await page.goto('/login');

    await expect(page).toHaveURL('https://idp.example/authorize');
  });

  test('skip=true suppresses the instant redirect', async ({
    network,
    page
  }) => {
    network.use(
      gen.handleAuthConfig(() =>
        json({ ...oidcConfig, instant_redirect: true })
      ),
      oidcUrl('https://idp.example/authorize')
    );
    await gotoReady(page, '/login?skip=true');

    await expect(
      page.getByRole('button', { name: 'OIDC Provider' })
    ).toBeVisible();
    // The `skip` flag is stripped from the URL once handled.
    await expect(page).toHaveURL(`${URL}/login`);
  });
});

test.describe('OIDC error query param', () => {
  for (const [code, message] of Object.entries(OIDC_ERRORS)) {
    test(`maps ${code} to its toast`, async ({ page }) => {
      await gotoReady(page, `/login?error=${code}`);

      await expect(page.getByText(message)).toBeVisible();
      await expect(page).toHaveURL(`${URL}/login`);
    });
  }

  // Regression: on `?error=` the auth config was skipped, so the OIDC button
  // Showed (and failed) even with SSO off.
  test('hides OIDC and skips the url request when SSO is off', async ({
    network,
    page
  }) => {
    let urlRequested = false;
    network.use(
      http.get('*/api/auth/oidc/url', () => {
        urlRequested = true;
        return json({ url: 'https://idp.example/authorize' });
      })
    );
    await gotoReady(page, '/login?error=invalid_grant');

    await expect(page.getByText(OIDC_ERRORS.invalid_grant)).toBeVisible();
    await expect(page.getByPlaceholder('mail@example.com')).toBeVisible();
    await expect(
      page.getByRole('button', { name: 'OIDC Provider' })
    ).toHaveCount(0);
    expect(urlRequested).toBe(false);
  });

  test('does not loop back to the provider with instant redirect', async ({
    network,
    page
  }) => {
    network.use(
      gen.handleAuthConfig(() =>
        json({ ...oidcConfig, instant_redirect: true })
      ),
      oidcUrl('https://idp.example/authorize')
    );
    await page.route('https://idp.example/**', async (route) =>
      route.fulfill({ body: 'idp', contentType: 'text/plain' })
    );
    await gotoReady(page, '/login?error=invalid_grant');

    await expect(page.getByText(OIDC_ERRORS.invalid_grant)).toBeVisible();
    await expect(
      page.getByRole('button', { name: 'OIDC Provider' })
    ).toBeVisible();
    await expect(page).toHaveURL(`${URL}/login`);
  });

  test('falls back to a generic message for unknown codes', async ({
    page
  }) => {
    await gotoReady(page, '/login?error=weird_failure');

    await expect(
      page.getByText('SSO login failed: weird_failure')
    ).toBeVisible();
    await expect(page).toHaveURL(`${URL}/login`);
  });
});
