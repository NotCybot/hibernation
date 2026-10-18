import { describe, expect, it, vi } from 'vitest';
import { load as listLoad } from '$routes/caches/+page';
import { load as layoutLoad } from '$routes/caches/[uuid]/+layout';
import { load as detailLoad } from '$routes/caches/[uuid]/+page';
import { jsonFetch, runLoad } from '$test_helpers/load';

describe('caches list load', () => {
  it('resolves caches and exposes the error param', async () => {
    const result = await runLoad(listLoad, {
      fetch: jsonFetch([{ name: 'c' }]),
      url: new URL('http://x/caches?error=nope')
    });
    expect(result.error).toBe('nope');
    await expect(result.caches).resolves.toEqual([{ name: 'c' }]);
  });

  it('resolves undefined caches on an API error and null error without param', async () => {
    const result = await runLoad(listLoad, {
      fetch: jsonFetch({ error: 'x' }, 500),
      url: new URL('http://x/caches')
    });
    expect(result.error).toBeNull();
    await expect(result.caches).resolves.toBeUndefined();
  });
});

describe('cache [uuid] layout load', () => {
  it('requests the details for the uuid and returns the raw response', async () => {
    const fetch = vi.fn(jsonFetch({ name: 'c' }));
    const result = await runLoad(layoutLoad, {
      fetch,
      params: { uuid: 'c1' }
    });
    expect(result.uuid).toBe('c1');
    await expect(result.cacheRes).resolves.toMatchObject({
      data: { name: 'c' }
    });
    const req = fetch.mock.calls[0]?.[0] as Request;
    expect(new URL(req.url).pathname).toBe('/api/cache/management/c1');
  });

  it('surfaces a 404 as an error response instead of throwing', async () => {
    const result = await runLoad(layoutLoad, {
      fetch: jsonFetch({ error: 'not found' }, 404),
      params: { uuid: 'missing' }
    });
    const res = await result.cacheRes;
    expect(res.data).toBeUndefined();
    expect(res.response.status).toBe(404);
  });
});

describe('cache [uuid] page load', () => {
  it('resolves the general settings', async () => {
    const result = await runLoad(detailLoad, {
      fetch: jsonFetch({ site_url: 'https://x/' })
    });
    await expect(result.generalSettings).resolves.toEqual({
      site_url: 'https://x/'
    });
  });

  it('resolves undefined on an API error', async () => {
    const result = await runLoad(detailLoad, { fetch: jsonFetch({}, 500) });
    await expect(result.generalSettings).resolves.toBeUndefined();
  });
});
