import { describe, expect, it, vi } from 'vitest';
import { load as listLoad } from '$routes/tokens/+page';
import { load as detailLoad } from '$routes/tokens/[uuid]/+page';
import { jsonFetch, runLoad } from '$test_helpers/load';

describe('tokens list load', () => {
  it('resolves tokens and exposes the error param', async () => {
    const result = await runLoad(listLoad, {
      fetch: jsonFetch([{ exp: '2099-01-01T00:00:00Z', uuid: 't1' }]),
      url: new URL('http://x/tokens?error=e')
    });
    expect(result.error).toBe('e');
    await expect(result.tokens).resolves.toEqual([
      { exp: new Date('2099-01-01T00:00:00Z'), uuid: 't1' }
    ]);
  });

  // Regression: the SDK must apply the generated date transformers, otherwise
  // `exp < new Date()` compares a string and expired tokens are never found.
  it('parses exp and last_used into Dates', async () => {
    const result = await runLoad(listLoad, {
      fetch: jsonFetch([
        {
          exp: '2020-01-01T00:00:00Z',
          last_used: '2019-06-01T00:00:00Z',
          uuid: 't1'
        },
        { exp: '2099-01-01T00:00:00Z', last_used: null, uuid: 't2' }
      ]),
      url: new URL('http://x/tokens')
    });
    const [old, fresh] = (await result.tokens) ?? [];
    expect(old?.exp).toBeInstanceOf(Date);
    expect(old?.last_used).toEqual(new Date('2019-06-01T00:00:00Z'));
    expect(old!.exp < new Date()).toBe(true);
    expect(fresh!.exp < new Date()).toBe(false);
    expect(fresh?.last_used).toBeNull();
  });

  it('resolves undefined tokens on an API error', async () => {
    const result = await runLoad(listLoad, {
      fetch: jsonFetch({}, 500),
      url: new URL('http://x/tokens')
    });
    expect(result.error).toBeNull();
    await expect(result.tokens).resolves.toBeUndefined();
  });
});

describe('token detail load', () => {
  it('requests the token by uuid', async () => {
    const fetch = vi.fn(jsonFetch({ exp: '2099-01-01T00:00:00Z', name: 'ci' }));
    const result = await runLoad(detailLoad, { fetch, params: { uuid: 't1' } });
    expect(result.uuid).toBe('t1');
    await expect(result.tokenRes).resolves.toMatchObject({
      data: { exp: new Date('2099-01-01T00:00:00Z'), name: 'ci' }
    });
    const req = fetch.mock.calls[0]?.[0] as Request;
    expect(new URL(req.url).pathname).toBe('/api/token/t1');
  });

  it('returns the error response for an unknown token', async () => {
    const result = await runLoad(detailLoad, {
      fetch: jsonFetch({}, 404),
      params: { uuid: 'nope' }
    });
    expect((await result.tokenRes).response.status).toBe(404);
  });
});
