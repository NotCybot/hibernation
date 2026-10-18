import type { BrowserContext } from '@playwright/test';
import type { Scenario } from '../mocks/e2e/data';

const URL = 'http://localhost:4173';

/**
 * Mirrors a cookie into `document.cookie` for every page in the context. The
 * e2e MSW handlers read `mock_*` cookies from the *client-side* request
 * cookies; some browsers do not reliably expose context cookies to those
 * intercepted fetches, so we also seed them through an init script.
 */
const seedCookie = async (
  context: BrowserContext,
  name: string,
  value: string
) => {
  await context.addCookies([{ name, url: URL, value }]);
  await context.addInitScript((cookie) => {
    // oxlint-disable-next-line no-document-cookie
    document.cookie = `${cookie}; path=/`;
  }, `${name}=${value}`);
};

/**
 * Seeds the auth cookie (so protected routes don't redirect to /login) and the
 * `mock_scenario` cookie that the e2e MSW handlers read to vary their data.
 */
export const setupSession = async (
  context: BrowserContext,
  scenario: Scenario = 'default'
) => {
  await context.addCookies([
    { name: 'centaurus_jwt', url: URL, value: 'e2e-token' }
  ]);
  await seedCookie(context, 'mock_scenario', scenario);
};

/**
 * Seeds only `mock_setup=pending` (no auth cookie) so `isSetup` reports an
 * un-provisioned instance and the /setup wizard renders.
 */
export const seedSetupPending = async (context: BrowserContext) =>
  seedCookie(context, 'mock_setup', 'pending');

/** Seeds `mock_mail=off` so `mailActive` reports mail as unconfigured. */
export const seedMailInactive = async (context: BrowserContext) =>
  seedCookie(context, 'mock_mail', 'off');

/** Seeds `mock_anon=1` so `info` / `testToken` report no session. */
export const seedAnonymous = async (context: BrowserContext) =>
  seedCookie(context, 'mock_anon', '1');
