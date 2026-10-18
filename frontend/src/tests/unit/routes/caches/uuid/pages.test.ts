import { afterEach, describe, expect, it, vi } from 'vitest';
import { createRawSnippet } from 'svelte';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { goto } from '$app/navigation';
import { iconButtons } from '$test_helpers/load';

const api = {
  clearCache: vi.fn(),
  deleteCache: vi.fn(),
  deletePath: vi.fn(),
  editCache: vi.fn(),
  searchStorePaths: vi.fn()
};
vi.mock('$lib/client', async (orig) => ({
  ...(await orig<object>()),
  clearCache: (...a: unknown[]) => api.clearCache(...a),
  deleteCache: (...a: unknown[]) => api.deleteCache(...a),
  deletePath: (...a: unknown[]) => api.deletePath(...a),
  editCache: (...a: unknown[]) => api.editCache(...a),
  searchStorePaths: (...a: unknown[]) => api.searchStorePaths(...a)
}));
const toast = { error: vi.fn(), success: vi.fn() };
vi.mock('@profidev/pleiades/components/util/general', async (orig) => ({
  ...(await orig<object>()),
  toast: {
    error: (m: string) => toast.error(m),
    success: (m: string) => toast.success(m)
  }
}));
const sonnerError = vi.fn();
vi.mock('svelte-sonner', async (orig) => ({
  ...(await orig<object>()),
  toast: { error: (m: string) => sonnerError(m) }
}));

const Layout = (await import('$routes/caches/[uuid]/+layout.svelte')).default;
const Overview = (await import('$routes/caches/[uuid]/+page.svelte')).default;
const Search = (await import('$routes/caches/[uuid]/search/+page.svelte'))
  .default;
const Settings = (await import('$routes/caches/[uuid]/settings/+page.svelte'))
  .default;

const MiB = 1024 ** 2;
const cache = (over: object = {}) => ({
  allow_force_push: false,
  downstream_caches: ['https://cache.nixos.org/'],
  eviction_policy: 'LeastRecentlyUsed',
  has_write_access: true,
  name: 'main',
  nar_count: 3,
  priority: 40,
  public: false,
  quota: 1024 * MiB,
  sig_key: 'main-1:abc',
  size: 512 * MiB,
  uuid: 'c1',
  ...over
});
const res = async (over: object = {}) => Promise.resolve({ data: cache(over) });
const children = createRawSnippet(() => ({ render: () => '<p>child</p>' }));

afterEach(() => {
  for (const f of Object.values(api)) {
    f.mockReset();
  }
  toast.error.mockReset();
  toast.success.mockReset();
  sonnerError.mockReset();
});

describe('cache layout', () => {
  it('shows the name, the child and the section links', async () => {
    render(Layout, {
      children,
      data: { cacheRes: res(), uuid: 'c1' } as never
    });
    expect(await screen.findByText(/Cache:\s*main/)).toBeInTheDocument();
    expect(screen.getByText('child')).toBeInTheDocument();
    for (const [name, href] of [
      ['Overview', '/caches/c1'],
      ['Search', '/caches/c1/search'],
      ['Settings', '/caches/c1/settings']
    ]) {
      expect(screen.getByRole('link', { name })).toHaveAttribute('href', href);
    }
  });

  it.each([
    [404, '/caches?error=not_found'],
    [500, '/caches?error=other']
  ])('redirects a %d to the list', async (status, to) => {
    render(Layout, {
      children,
      data: {
        cacheRes: Promise.resolve({ response: { status } }),
        uuid: 'c1'
      } as never
    });
    await vi.waitFor(() => expect(goto).toHaveBeenCalledWith(to));
  });
});

describe('cache overview', () => {
  const codes = async (container: HTMLElement) => {
    await vi.waitFor(() =>
      expect(container.querySelectorAll('pre.shiki').length).toBe(4)
    );
    return [...container.querySelectorAll('pre.shiki')].map(
      (p) => p.textContent
    );
  };

  it('shows usage and path-based pull instructions', async () => {
    const { container } = render(Overview, {
      data: {
        cacheRes: res(),
        generalSettings: Promise.resolve({ site_url: 'https://hib.example/' })
      } as never
    });
    expect(
      await screen.findByText('0.50GiB / 1.00GiB (50.0%)')
    ).toBeInTheDocument();
    expect(screen.getByText('3')).toBeInTheDocument();
    const [push, url, key, config] = await codes(container);
    expect(push).toBe('hibernation push main /nix/store/...');
    expect(url).toBe('https://hib.example/api/nix/main');
    expect(key).toBe('main-1:abc');
    expect(config).toContain('"https://hib.example/api/nix/main"');
    expect(config).toContain('"main-1:abc"');
  });

  // Regression: the url dropped any base path in site_url.
  it.each([
    ['https://hib.example/prefix/', 'https://hib.example/prefix/api/nix/main'],
    ['https://hib.example/prefix', 'https://hib.example/prefix/api/nix/main']
  ])('keeps the base path of %s', async (site_url, expected) => {
    const { container } = render(Overview, {
      data: {
        cacheRes: res(),
        generalSettings: Promise.resolve({ site_url })
      } as never
    });
    const [, url] = await codes(container);
    expect(url).toBe(expected);
  });

  it('falls back to the current origin without settings', async () => {
    const { container } = render(Overview, {
      data: {
        cacheRes: res(),
        generalSettings: Promise.resolve(undefined)
      } as never
    });
    const [, url] = await codes(container);
    expect(url).toBe('http://localhost/api/nix/main');
  });

  it('uses a sub-domain with virtual host routing', async () => {
    const { container } = render(Overview, {
      data: {
        cacheRes: res(),
        generalSettings: Promise.resolve({
          site_url: 'https://hib.example/',
          virtual_host_routing: true
        })
      } as never
    });
    await screen.findByText('0.50GiB / 1.00GiB (50.0%)');
    const [, url] = await codes(container);
    expect(url).toBe('https://main.hib.example');
  });

  it.each([
    [800, 'yellow'],
    [1000, 'red']
  ])('colours %d MiB used as %s', async (used, colour) => {
    const { container } = render(Overview, {
      data: {
        cacheRes: res({ size: used * MiB }),
        generalSettings: Promise.resolve({ site_url: 'https://h/' })
      } as never
    });
    await vi.waitFor(() =>
      expect(
        container.querySelector('[data-slot="progress"]')?.className
      ).toContain(`bg-${colour}-500`)
    );
  });

  it('renders placeholders while loading', () => {
    render(Overview, {
      data: {
        cacheRes: new Promise(() => {}),
        generalSettings: new Promise(() => {})
      } as never
    });
    expect(screen.getByText('Cache size:')).toBeInTheDocument();
    // No placeholder host leaks into the instructions.
    expect(document.body.textContent).not.toContain('localhost:5173');
  });
});

describe('cache search', () => {
  const results = [
    {
      accessed: 2,
      created_at: '2030-01-01T00:00:00Z',
      last_accessed_at: undefined,
      size: MiB,
      store_path: 'abc-hello'
    }
  ];

  it('searches on load and when the query changes', async () => {
    api.searchStorePaths.mockResolvedValue({ data: results });
    render(Search, { data: { cacheRes: res() } as never });
    expect(await screen.findByText('abc-hello')).toBeInTheDocument();
    expect(api.searchStorePaths).toHaveBeenCalledWith({
      body: { order: 'Asc', query: '', sort: 'StorePath' },
      path: { uuid: 'c1' }
    });
    await fireEvent.input(screen.getByPlaceholderText('Search...'), {
      target: { value: 'hel' }
    });
    await vi.waitFor(() =>
      expect(api.searchStorePaths).toHaveBeenLastCalledWith(
        expect.objectContaining({
          body: expect.objectContaining({ query: 'hel' })
        })
      )
    );
  });

  it('re-searches when sort and order change', async () => {
    api.searchStorePaths.mockResolvedValue({ data: results });
    render(Search, { data: { cacheRes: res() } as never });
    await screen.findByText('abc-hello');
    const pick = async (current: string, next: string) => {
      const trigger = screen.getByText(current, { selector: 'button' });
      await fireEvent.pointerDown(trigger, { button: 0, pointerType: 'mouse' });
      await fireEvent.pointerUp(trigger, { button: 0, pointerType: 'mouse' });
      const option = await screen.findByRole('option', { name: next });
      await fireEvent.pointerUp(option, { button: 0, pointerType: 'mouse' });
      await fireEvent.click(option);
    };
    await pick('StorePath', 'Size');
    await vi.waitFor(() =>
      expect(api.searchStorePaths).toHaveBeenLastCalledWith(
        expect.objectContaining({
          body: expect.objectContaining({ sort: 'Size' })
        })
      )
    );
    await pick('Asc', 'Desc');
    await vi.waitFor(() =>
      expect(api.searchStorePaths).toHaveBeenLastCalledWith(
        expect.objectContaining({
          body: expect.objectContaining({ order: 'Desc' })
        })
      )
    );
  });

  it('does not search when the cache failed to load', async () => {
    render(Search, { data: { cacheRes: Promise.resolve({}) } as never });
    await new Promise((r) => setTimeout(r, 10));
    expect(api.searchStorePaths).not.toHaveBeenCalled();
  });

  it('toasts a failed search', async () => {
    api.searchStorePaths.mockResolvedValue({ error: 'x' });
    render(Search, { data: { cacheRes: res() } as never });
    await vi.waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith('Failed to search cache')
    );
  });

  it('deletes a path and searches again', async () => {
    api.searchStorePaths.mockResolvedValue({ data: results });
    api.deletePath.mockResolvedValue({ data: {} });
    render(Search, { data: { cacheRes: res() } as never });
    await screen.findByText('abc-hello');
    await fireEvent.click(
      screen.getByRole('button', { name: 'Delete store path' })
    );
    await vi.waitFor(() =>
      expect(api.deletePath).toHaveBeenCalledWith({
        body: { store_path: 'abc-hello' },
        path: { uuid: 'c1' }
      })
    );
    await vi.waitFor(() =>
      expect(api.searchStorePaths).toHaveBeenCalledTimes(2)
    );
  });

  it('toasts a failed path delete', async () => {
    api.searchStorePaths.mockResolvedValue({ data: results });
    api.deletePath.mockResolvedValue({ error: 'x' });
    render(Search, { data: { cacheRes: res() } as never });
    await screen.findByText('abc-hello');
    await fireEvent.click(
      screen.getByRole('button', { name: 'Delete store path' })
    );
    await vi.waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith('Failed to delete path')
    );
  });

  it('disables delete without write access', async () => {
    api.searchStorePaths.mockResolvedValue({ data: results });
    render(Search, {
      data: { cacheRes: res({ has_write_access: false }) } as never
    });
    await screen.findByText('abc-hello');
    expect(
      screen.getByRole('button', { name: 'Delete store path' })
    ).toBeDisabled();
  });
});

describe('cache settings', () => {
  const loaded = async () =>
    vi.waitFor(() =>
      expect(screen.getByLabelText('Cache Name')).toHaveValue('main')
    );
  const btn = (name: RegExp) => screen.getAllByRole('button', { name })[0];
  const confirm = async (name: string) =>
    fireEvent.click(screen.getAllByRole('button', { name }).at(-1)!);
  const renderSettings = (over: object = {}) =>
    render(Settings, { data: { cacheRes: res(over) } as never });

  it('fills the form and shows the downstream caches', async () => {
    renderSettings();
    await loaded();
    expect(screen.getByLabelText('Priority')).toHaveValue(40);
    expect(screen.getByText('https://cache.nixos.org/')).toBeInTheDocument();
    expect(btn(/make public/i)).toBeEnabled();
  });

  it('is readonly without write access', async () => {
    renderSettings({ has_write_access: false });
    await loaded();
    for (const name of [
      /make public/i,
      /change quota/i,
      /change key/i,
      /clear cache/i,
      /delete cache/i,
      /save changes/i
    ]) {
      expect(btn(name)).toBeDisabled();
    }
    // Regression: the downstream tags and eviction select stayed editable.
    expect(
      screen.getByPlaceholderText('Add a downstream cache')
    ).toBeDisabled();
    const evictionSelect = screen
      .getAllByRole('combobox')
      .find((el) => el.tagName === 'BUTTON');
    expect(evictionSelect).toBeDisabled();
  });

  it('saves the edited settings with a single eviction policy', async () => {
    api.editCache.mockResolvedValue({ data: {} });
    renderSettings();
    await loaded();
    await fireEvent.input(screen.getByLabelText('Priority'), {
      target: { value: '10' }
    });
    await fireEvent.click(btn(/save changes/i));
    await vi.waitFor(() => expect(api.editCache).toHaveBeenCalledOnce());
    const [[call]] = api.editCache.mock.calls as [
      [{ body: Record<string, unknown> }]
    ];
    expect(call).toMatchObject({
      body: {
        downstream_caches: ['https://cache.nixos.org/'],
        eviction_policy: 'LeastRecentlyUsed',
        name: 'main',
        priority: 10
      },
      path: { uuid: 'c1' }
    });
    expect(toast.success).toHaveBeenCalledWith(
      'Cache main updated successfully'
    );
  });

  it('maps a 409 on save to a name error', async () => {
    api.editCache.mockResolvedValue({ error: 'x', response: { status: 409 } });
    renderSettings();
    await loaded();
    await fireEvent.click(btn(/save changes/i));
    expect(
      await screen.findByText('This cache name is already in use')
    ).toBeInTheDocument();
  });

  it('toasts other save errors', async () => {
    api.editCache.mockResolvedValue({ error: 'x', response: { status: 500 } });
    renderSettings();
    await loaded();
    await fireEvent.click(btn(/save changes/i));
    await vi.waitFor(() =>
      expect(sonnerError).toHaveBeenCalledWith('Failed to update cache')
    );
  });

  it.each([
    [
      false,
      /make public/i,
      'Make Public',
      { data: {} },
      'success',
      'Cache main is now public'
    ],
    [
      true,
      /make private/i,
      'Make Private',
      { data: {} },
      'success',
      'Cache main is now private'
    ],
    [
      false,
      /make public/i,
      'Make Public',
      { error: 'x' },
      'error',
      'Failed to change cache visibility for main'
    ]
  ] as const)(
    'toggles visibility from public=%s',
    async (pub, open, confirmName, result, kind, message) => {
      api.editCache.mockResolvedValue(result);
      renderSettings({ public: pub });
      await loaded();
      await fireEvent.click(btn(open));
      await confirm(confirmName);
      await vi.waitFor(() => expect(toast[kind]).toHaveBeenCalledWith(message));
      expect(api.editCache).toHaveBeenCalledWith(
        expect.objectContaining({
          body: expect.objectContaining({ public: !pub })
        })
      );
    }
  );

  it('changes the quota converting MiB to bytes', async () => {
    api.editCache.mockResolvedValue({ data: {} });
    renderSettings();
    await loaded();
    await fireEvent.click(btn(/change quota/i));
    const input = await screen.findByLabelText('Quota (in MiB)');
    expect(input).toHaveValue(1024);
    await fireEvent.input(input, { target: { value: '2048' } });
    await confirm('Change');
    await vi.waitFor(() =>
      expect(api.editCache).toHaveBeenCalledWith(
        expect.objectContaining({
          body: expect.objectContaining({ quota: 2048 * MiB })
        })
      )
    );
    expect(toast.success).toHaveBeenCalledWith(
      'Cache main quota updated successfully'
    );
  });

  it('toasts a failed quota change', async () => {
    api.editCache.mockResolvedValue({ error: 'x' });
    renderSettings();
    await loaded();
    await fireEvent.click(btn(/change quota/i));
    await screen.findByLabelText('Quota (in MiB)');
    await confirm('Change');
    await vi.waitFor(() =>
      expect(sonnerError).toHaveBeenCalledWith('Failed to change cache quota')
    );
  });

  const changeKey = async () => {
    renderSettings();
    await loaded();
    await fireEvent.click(btn(/change key/i));
    await fireEvent.input(await screen.findByLabelText('Public Signing Key'), {
      target: { value: 'main-2:def' }
    });
    await confirm('Change');
  };

  it('changes the signing key', async () => {
    api.editCache.mockResolvedValue({ data: {} });
    await changeKey();
    await vi.waitFor(() =>
      expect(api.editCache).toHaveBeenCalledWith(
        expect.objectContaining({
          body: expect.objectContaining({ sig_key: 'main-2:def' })
        })
      )
    );
    expect(toast.success).toHaveBeenCalledWith(
      'Cache main key updated successfully'
    );
  });

  it('maps a 406 on key change to a field error', async () => {
    api.editCache.mockResolvedValue({ error: 'x', response: { status: 406 } });
    await changeKey();
    expect(
      await screen.findByText(
        'The provided key is not a valid public signing key'
      )
    ).toBeInTheDocument();
  });

  it('toasts other key errors', async () => {
    api.editCache.mockResolvedValue({ error: 'x', response: { status: 500 } });
    await changeKey();
    await vi.waitFor(() =>
      expect(sonnerError).toHaveBeenCalledWith('Failed to change cache key')
    );
  });

  it.each([
    [{ data: {} }, 'Cache main cleared successfully', undefined],
    [{ error: 'x' }, undefined, 'Failed to clear cache']
  ])('clears the cache (%#)', async (result, ok, fail) => {
    api.clearCache.mockResolvedValue(result);
    renderSettings();
    await loaded();
    await fireEvent.click(btn(/clear cache/i));
    await confirm('Clear');
    await vi.waitFor(() =>
      ok
        ? expect(toast.success).toHaveBeenCalledWith(ok)
        : expect(sonnerError).toHaveBeenCalledWith(fail)
    );
    expect(api.clearCache).toHaveBeenCalledWith({ path: { uuid: 'c1' } });
  });

  it('deletes the cache and returns to the list', async () => {
    api.deleteCache.mockResolvedValue({ data: {} });
    renderSettings();
    await loaded();
    await fireEvent.click(btn(/delete cache/i));
    await confirm('Delete');
    await vi.waitFor(() => expect(goto).toHaveBeenCalledWith('/caches'));
    expect(toast.success).toHaveBeenCalledWith(
      'Cache main deleted successfully'
    );
  });

  it('toasts a failed delete', async () => {
    api.deleteCache.mockResolvedValue({ error: 'x' });
    renderSettings();
    await loaded();
    await fireEvent.click(btn(/delete cache/i));
    await confirm('Delete');
    await vi.waitFor(() =>
      expect(sonnerError).toHaveBeenCalledWith('Failed to delete cache')
    );
  });

  it('ignores a failed cache load', async () => {
    render(Settings, { data: { cacheRes: Promise.resolve({}) } as never });
    await new Promise((r) => setTimeout(r, 10));
    expect(screen.getByLabelText('Cache Name')).toHaveValue('');
    expect(iconButtons('trash')[0]).toBeDisabled();
  });
});
