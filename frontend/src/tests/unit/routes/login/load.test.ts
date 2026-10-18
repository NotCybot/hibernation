import { describe, expect, it } from 'vitest';
import { load } from '$routes/login/+page';
import { jsonFetch, runLoad } from '$test_helpers/load';

describe('login load', () => {
  // Regression: the auth config used to be skipped on `?error=`, which showed
  // The OIDC button even with SSO off. It must still load, but with `skip` set
  // So an instant-redirect setup doesn't loop back to the failing provider.
  it('still loads the auth config on an SSO error, with skip forced', async () => {
    const result = await runLoad(load, {
      fetch: jsonFetch({ instant_redirect: true, sso_type: 'Oidc' }),
      url: new URL('http://x/login?error=boom')
    });
    expect(result.error).toBe('boom');
    expect(result.redirectTo).toBe('/');
    expect(result.skip).toBe(true);
    await expect(result.config).resolves.toEqual({
      instant_redirect: true,
      sso_type: 'Oidc'
    });
  });

  it('loads the auth config when there is no error', async () => {
    const result = await runLoad(load, {
      fetch: jsonFetch({ sso_type: 'None' }),
      url: new URL('http://x/login')
    });
    expect(result.error).toBeUndefined();
    expect(result.redirectTo).toBe('/');
    expect(result.skip).toBe(false);
    await expect(result.config).resolves.toEqual({ sso_type: 'None' });
  });

  it('exposes a validated redirect target from the query string', async () => {
    const result = await runLoad(load, {
      fetch: jsonFetch({ sso_type: 'None' }),
      url: new URL('http://x/login?redirect=%2Fusers')
    });
    expect(result.redirectTo).toBe('/users');
  });

  it('ignores an unsafe redirect param', async () => {
    const result = await runLoad(load, {
      fetch: jsonFetch({ sso_type: 'None' }),
      url: new URL('http://x/login?redirect=//evil.com')
    });
    expect(result.redirectTo).toBe('/');
  });

  it('reads the skip flag from the query', async () => {
    const result = await runLoad(load, {
      fetch: jsonFetch({ sso_type: 'None' }),
      url: new URL('http://x/login?skip=true')
    });
    expect(result.skip).toBe(true);
  });
});
