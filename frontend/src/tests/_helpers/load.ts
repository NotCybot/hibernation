/**
 * Test helpers for SvelteKit `load` functions. Not a test file (no `.test`
 * suffix) so the vitest glob ignores it.
 */

/** A `fetch` stand-in that always resolves to a canned JSON response. */
export const jsonFetch =
  (body: unknown, status = 200): typeof fetch =>
  async () =>
    new Response(status === 204 ? null : JSON.stringify(body), {
      headers: { 'content-type': 'application/json' },
      status
    });

/**
 * Invokes a SvelteKit `load` with a loosely-typed event and returns the result
 * as `any`, so tests can read data fields without fighting the `void`-union
 * return type of `PageLoad`/`LayoutLoad`.
 */
export const runLoad = async (
  load: (event: never) => unknown,
  event: unknown
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
): Promise<any> => load(event as never);

interface ThrownRedirect {
  status: number;
  location: string;
}

/**
 * Runs `fn` and returns the SvelteKit redirect it throws. Fails if `fn` does
 * not redirect.
 */
export const catchRedirect = async (
  fn: () => unknown
): Promise<ThrownRedirect> => {
  try {
    await fn();
  } catch (error) {
    return error as ThrownRedirect;
  }
  throw new Error('expected a redirect to be thrown');
};

/**
 * Renders a tanstack column's `cell` (or `header`) for one row through
 * `FlexRender` and returns the container, so formatter output can be asserted.
 */
export const renderCell = async (
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  def: any,
  original: Record<string, unknown>,
  part: 'cell' | 'header' = 'cell'
): Promise<HTMLElement> => {
  const { render } = await import('@testing-library/svelte');
  const { FlexRender } = await import('@tanstack/svelte-table');
  const row = { getValue: (key: string) => original[key], original };
  const column = { getIsSorted: () => false, toggleSorting: () => {} };
  // `context` is also a mount option, so props must be passed explicitly.
  return render(
    FlexRender as never,
    {
      props: { content: def[part], context: { column, row } }
    } as never
  ).container;
};

/** Buttons whose lucide icon matches `icon` (icon-only buttons have no name). */
export const iconButtons = (icon: string): HTMLButtonElement[] =>
  [...document.querySelectorAll<HTMLButtonElement>('button')].filter((b) =>
    b.querySelector(`.lucide-${icon}`)
  );

/**
 * A `$app/navigation` mock whose `afterNavigate` runs its callback at once,
 * for pages that read `?error=` there. Use as
 * `vi.mock('$app/navigation', async () => (await import('$test_helpers/load')).navigationMock())`.
 */
export const navigationMock = async () => {
  const { vi } = await import('vitest');
  return {
    afterNavigate: (fn: () => void) => fn(),
    beforeNavigate: vi.fn(),
    disableScrollHandling: vi.fn(),
    goto: vi.fn(async () => Promise.resolve()),
    invalidate: vi.fn(async () => Promise.resolve()),
    invalidateAll: vi.fn(async () => Promise.resolve()),
    onNavigate: vi.fn(),
    preloadCode: vi.fn(async () => Promise.resolve()),
    preloadData: vi.fn(async () => Promise.resolve()),
    pushState: vi.fn(),
    replaceState: vi.fn()
  };
};
