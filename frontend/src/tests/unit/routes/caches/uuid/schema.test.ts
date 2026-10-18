import { describe, expect, it } from 'vitest';
import {
  cacheSchema,
  keySchema,
  quotaSchema
} from '$routes/caches/[uuid]/settings/schema.svelte';
import { EvictionPolicy } from '$lib/client';

const base = { name: 'cache', priority: 0 };

describe('cacheSchema', () => {
  it('defaults force push off and eviction to LRU', () => {
    expect(cacheSchema.safeParse(base).data).toEqual({
      ...base,
      allow_force_push: false,
      eviction_policy: [EvictionPolicy.LEAST_RECENTLY_USED]
    });
  });

  it.each(Object.values(EvictionPolicy))(
    'accepts eviction policy %s',
    (policy) => {
      expect(
        cacheSchema.safeParse({ ...base, eviction_policy: [policy] }).success
      ).toBe(true);
    }
  );

  it('rejects an unknown eviction policy', () => {
    expect(
      cacheSchema.safeParse({ ...base, eviction_policy: ['Random'] }).success
    ).toBe(false);
  });

  it.each([0, 1, 1000])('accepts priority %d', (priority) => {
    expect(cacheSchema.safeParse({ ...base, priority }).success).toBe(true);
  });

  it.each([-1, 1.5, undefined])('rejects priority %j', (priority) => {
    expect(cacheSchema.safeParse({ ...base, priority }).success).toBe(false);
  });
});

describe('quotaSchema', () => {
  it('defaults the quota', () => {
    expect(quotaSchema.parse({}).quota).toBe(10 * 1024);
  });

  it.each([
    [1, true],
    [0, false],
    [-5, false]
  ])('quota %d valid=%s', (quota, ok) => {
    expect(quotaSchema.safeParse({ quota }).success).toBe(ok);
  });
});

describe('keySchema', () => {
  it('requires a non-empty key', () => {
    expect(keySchema.parse({}).sig_key).toBe('');
    expect(keySchema.safeParse({ sig_key: '' }).success).toBe(false);
    expect(keySchema.safeParse({ sig_key: 'k' }).success).toBe(true);
  });
});
