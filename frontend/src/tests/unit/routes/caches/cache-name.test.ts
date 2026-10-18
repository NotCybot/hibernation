import { describe, expect, it } from 'vitest';
import { information } from '$routes/caches/create/schema.svelte';
import { cacheSchema } from '$routes/caches/[uuid]/settings/schema.svelte';

// Both schemas must accept exactly what the backend `CacheRegex` accepts:
// `^[a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?$`, at most 63 characters and not
// Purely numeric.
const createName = (name: string) =>
  information.safeParse({ name, quota: 1, sig_key: 'k' }).success;
const editName = (name: string) =>
  cacheSchema.safeParse({
    eviction_policy: ['OldestFirst'],
    name,
    priority: 0
  }).success;

describe.each([
  ['create', createName],
  ['settings', editName]
])('%s cache name', (_label, valid) => {
  it.each([
    'a',
    'Z',
    'a1',
    'cache',
    'My-Cache-01',
    'a-b',
    'a--b',
    '1cache',
    '0-0',
    'a'.repeat(63)
  ])('accepts %j', (name) => {
    expect(valid(name)).toBe(true);
  });

  it.each([
    '',
    '-',
    '-cache',
    'cache-',
    'my_cache',
    'my cache',
    'my.cache',
    'caché',
    '7',
    '123',
    '0000000000',
    'a'.repeat(64)
  ])('rejects %j', (name) => {
    expect(valid(name)).toBe(false);
  });
});

describe('cache name messages', () => {
  const messages = (name: string) =>
    information
      .safeParse({ name, quota: 1, sig_key: 'k' })
      .error?.issues.map((i) => i.message);

  it('explains the allowed characters', () => {
    expect(messages('-x')).toEqual([
      'Cache name may only contain letters, numbers and hyphens, and cannot start or end with a hyphen'
    ]);
  });

  it('rejects purely numeric names with a dedicated message', () => {
    expect(messages('123')).toEqual(['Cache name cannot be purely numeric']);
  });
});
