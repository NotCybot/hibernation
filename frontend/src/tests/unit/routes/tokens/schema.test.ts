import { afterEach, describe, expect, it, vi } from 'vitest';
import { information } from '$routes/tokens/create/schema.svelte';
import {
  formatData,
  reformatData,
  tokenSettings
} from '$routes/tokens/[uuid]/schema.svelte';
import type { TokenInfo } from '$lib/client';

const DAY = 24 * 60 * 60 * 1000;

afterEach(() => vi.useRealTimers());

describe.each([
  ['create information', information, 'Name is required'],
  ['token settings', tokenSettings, 'Token name is required']
])('%s schema', (_label, schema, nameMessage) => {
  it('accepts a name and a date', () => {
    const exp = new Date('2030-01-01');
    expect(schema.safeParse({ exp, name: 'ci' }).data).toEqual({
      exp,
      name: 'ci'
    });
  });

  it('defaults the expiry to roughly 30 days from module load', () => {
    const r = schema.parse({ name: 'ci' });
    const diff = r.exp.getTime() - Date.now();
    expect(diff).toBeGreaterThan(29 * DAY);
    expect(diff).toBeLessThanOrEqual(30 * DAY);
  });

  it('rejects an empty name', () => {
    const r = schema.safeParse({ exp: new Date(), name: '' });
    expect(r.error?.issues[0]?.message).toBe(nameMessage);
  });

  it.each(['2030-01-01', 0, new Date('invalid')])(
    'rejects non-Date expiry %j',
    (exp) => {
      const r = schema.safeParse({ exp, name: 'ci' });
      expect(r.success).toBe(false);
      expect(r.error?.issues[0]?.message).toBe('Invalid date');
    }
  );
});

describe('token create default name', () => {
  it('defaults the form name to empty without validating the default', () => {
    // Zod returns defaults as-is; the form submits '' which then fails min(1).
    expect(information.parse({}).name).toBe('');
  });
});

describe('token formatData / reformatData', () => {
  it('parses the ISO expiry into a Date', () => {
    const token = {
      exp: '2030-05-06T07:08:09.000Z',
      name: 'ci'
    } as unknown as TokenInfo;
    const form = formatData(token);
    expect(form.name).toBe('ci');
    expect(form.exp).toEqual(new Date('2030-05-06T07:08:09.000Z'));
  });

  it('builds the edit request with the uuid', () => {
    const exp = new Date('2030-01-01');
    expect(reformatData({ exp, name: 'ci' }, 't1')).toEqual({
      exp,
      name: 'ci',
      uuid: 't1'
    });
  });
});
