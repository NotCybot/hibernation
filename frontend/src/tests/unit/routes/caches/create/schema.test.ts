import { describe, expect, it } from 'vitest';
import { information } from '$routes/caches/create/schema.svelte';

const base = { name: 'cache', public: true, quota: 5, sig_key: 'key' };

describe('cache create information schema', () => {
  it('accepts a full valid payload', () => {
    expect(information.safeParse(base)).toMatchObject({
      data: base,
      success: true
    });
  });

  it('defaults public to false and quota to 10 GiB worth of MiB', () => {
    const r = information.safeParse({ name: 'c', sig_key: 'k' });
    expect(r.data).toMatchObject({ public: false, quota: 10 * 1024 });
  });

  it.each([0, -1, 0.5])('rejects quota %d below 1', (quota) => {
    expect(information.safeParse({ ...base, quota }).success).toBe(false);
  });

  it('accepts quota of exactly 1', () => {
    expect(information.safeParse({ ...base, quota: 1 }).success).toBe(true);
  });

  it('requires a signature key', () => {
    const r = information.safeParse({ ...base, sig_key: '' });
    expect(r.success).toBe(false);
    expect(r.error?.issues[0]?.message).toBe('Signature key is required');
  });

  it('reports the length message for a 64 char name', () => {
    const r = information.safeParse({ ...base, name: 'a'.repeat(64) });
    expect(r.error?.issues.map((i) => i.message)).toContain(
      'Cache name is too long (max 63 characters)'
    );
  });
});
