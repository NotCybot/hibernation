import { describe, expect, it } from 'vitest';
import { load } from '$routes/setup/+page';
import { catchRedirect, jsonFetch, runLoad } from '$test_helpers/load';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const event = (fetch: typeof globalThis.fetch) => ({ fetch }) as any;

describe('setup load', () => {
  it('redirects to / when setup is already complete', async () => {
    const redirect = await catchRedirect(async () =>
      load(
        event(
          jsonFetch({
            db_backend: 'SQLite',
            is_setup: true
          })
        )
      )
    );
    expect(redirect.status).toBe(302);
    expect(redirect.location).toBe('/');
  });

  it('returns the db backend when setup is not complete', async () => {
    const result = await runLoad(
      load,
      event(
        jsonFetch({
          db_backend: 'PostgreSQL',
          from_env: [],
          is_setup: false,
          settings: {}
        })
      )
    );
    expect(result.db_backend).toBe('PostgreSQL');
    expect(result.configured).toBe(false);
  });

  it('falls back to "unknown" when data is missing', async () => {
    const result = await runLoad(load, event(jsonFetch(null)));
    expect(result.db_backend).toBe('unknown');
    expect(result.storage_backend).toBe('unknown');
    expect(result.configured).toBe(false);
    expect(result.settings).toBeNull();
  });

  const routed = (status: object, oidc: object): typeof globalThis.fetch =>
    (async (req: Request) =>
      Response.json(
        new URL(req.url).pathname.includes('oidc') ? oidc : status
      )) as never;

  it('exposes the storage backend and normalises the oidc settings', async () => {
    const result = await runLoad(
      load,
      event(
        routed(
          { db_backend: 'SQLite', is_setup: false, storage_backend: 'S3' },
          { from_env: [], settings: { oidc_enabled: false }, site_url: 'x' }
        )
      )
    );
    expect(result.storage_backend).toBe('S3');
    expect(result.configured).toBe(false);
    expect(result.settings.settings).toMatchObject({
      oidc_client_secret: '',
      oidc_pkce: false,
      sso_create_user: false
    });
  });

  it('marks setup configured when a user-set value exists', async () => {
    const result = await runLoad(
      load,
      event(
        routed(
          { is_setup: false },
          { from_env: [], settings: { oidc_issuer: 'https://idp' } }
        )
      )
    );
    expect(result.configured).toBe(true);
  });

  it('ignores values that only come from the environment', async () => {
    const result = await runLoad(
      load,
      event(
        routed(
          { is_setup: false },
          {
            from_env: ['oidc_issuer'],
            settings: { oidc_enabled: false, oidc_issuer: 'https://idp' }
          }
        )
      )
    );
    expect(result.configured).toBe(false);
  });
});
