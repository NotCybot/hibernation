import { describe, expect, it, vi } from 'vitest';

const get = vi.fn();
vi.mock('@profidev/pleiades/backend', () => ({
  get: (...args: unknown[]) => get(...args)
}));

const { sendCliCode, size_to_gib } = await import('$lib/backend/util.svelte');

describe('size_to_gib', () => {
  it.each([
    [0, 0],
    [1024 ** 3, 1],
    [1.5 * 1024 ** 3, 1.5],
    [512 * 1024 ** 2, 0.5]
  ])('converts %d bytes to %d GiB', (bytes, gib) => {
    expect(size_to_gib(bytes)).toBe(gib);
  });
});

describe('sendCliCode', () => {
  it('hands the code to the local cli listener and returns its result', async () => {
    get.mockResolvedValue('ok');
    expect(await sendCliCode('abc123')).toBe('ok');
    expect(get).toHaveBeenCalledWith('http://localhost:16401?code=abc123');
  });
});
