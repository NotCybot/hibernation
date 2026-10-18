import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { invalidate } from '$app/navigation';
import { Permission } from '$lib/permissions.svelte';
import { iconButtons } from '$test_helpers/load';

vi.mock('$app/navigation', async () =>
  (await import('$test_helpers/load')).navigationMock()
);
const api = {
  deleteExpiredTokens: vi.fn(),
  deleteGroup: vi.fn(),
  deleteToken: vi.fn(),
  deleteUser: vi.fn()
};
vi.mock('$lib/client', async (orig) => ({
  ...(await orig<object>()),
  deleteExpiredTokens: (...a: unknown[]) => api.deleteExpiredTokens(...a),
  deleteGroup: (...a: unknown[]) => api.deleteGroup(...a),
  deleteToken: (...a: unknown[]) => api.deleteToken(...a),
  deleteUser: (...a: unknown[]) => api.deleteUser(...a)
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

const Groups = (await import('$routes/groups/+page.svelte')).default;
const Users = (await import('$routes/users/+page.svelte')).default;
const Tokens = (await import('$routes/tokens/+page.svelte')).default;
const Caches = (await import('$routes/caches/+page.svelte')).default;

const me = async (...permissions: string[]) =>
  Promise.resolve({ name: 'Me', permissions, uuid: 'me' });

const replaceState = vi.spyOn(window.history, 'replaceState');

beforeEach(() => {
  window.history.pushState({}, '', '/list?error=x&keep=1');
});
afterEach(() => {
  for (const f of Object.values(api)) {
    f.mockReset();
  }
  toast.error.mockReset();
  toast.success.mockReset();
  sonnerError.mockReset();
  replaceState.mockClear();
});

/** Opens the delete dialog for the first row and confirms it. */
const confirmDelete = async () => {
  await fireEvent.click(iconButtons('trash')[0]);
  await fireEvent.click(await screen.findByRole('button', { name: 'Delete' }));
};

describe.each([
  ['groups', Groups, 'Group'],
  ['users', Users, 'User'],
  ['tokens', Tokens, 'Token'],
  ['caches', Caches, 'Cache']
])('%s list error query', (_n, Page, noun) => {
  const base = {
    admin_group: Promise.resolve(undefined),
    caches: Promise.resolve([]),
    groups: Promise.resolve([]),
    tokens: Promise.resolve([]),
    user: me(),
    users: Promise.resolve([])
  };

  it.each([
    ['not_found', `${noun} not found`],
    ['other', `Failed to load ${noun.toLowerCase()}`]
  ])('toasts %s and strips the param', (error, message) => {
    render(Page as never, { data: { ...base, error } } as never);
    expect(toast.error).toHaveBeenCalledWith(message);
    const url = replaceState.mock.calls[0]?.[2] as URL;
    expect(url.searchParams.has('error')).toBe(false);
    expect(url.searchParams.get('keep')).toBe('1');
  });

  it('strips an unknown error silently', () => {
    render(Page as never, { data: { ...base, error: 'weird' } } as never);
    expect(toast.error).not.toHaveBeenCalled();
    expect(replaceState).toHaveBeenCalledOnce();
  });

  it('does nothing without an error', () => {
    render(Page as never, { data: { ...base, error: null } } as never);
    expect(replaceState).not.toHaveBeenCalled();
  });
});

describe('groups list', () => {
  const groups = [
    { id: 'g1', name: 'Ops', permissions: ['user:view'], users: [] }
  ];
  const data = (perms: string[], admin_group?: string) =>
    ({
      admin_group: Promise.resolve(admin_group),
      error: null,
      groups: Promise.resolve(groups),
      user: me(...perms)
    }) as never;

  it('lists groups and gates create on group:edit', async () => {
    render(Groups, { data: data([Permission.GROUP_EDIT, 'user:view']) });
    await vi.waitFor(() => expect(screen.getByText('Ops')).toBeInTheDocument());
    expect(screen.getByRole('link', { name: /create/i })).toHaveAttribute(
      'href',
      '/groups/create'
    );
  });

  it('disables create without group:edit', async () => {
    render(Groups, { data: data([]) });
    await screen.findByText('Ops');
    expect(screen.getByText('Create').closest('a')).toHaveAttribute(
      'aria-disabled',
      'true'
    );
  });

  it('deletes a group and invalidates the group endpoints', async () => {
    api.deleteGroup.mockResolvedValue({ data: {} });
    render(Groups, { data: data([Permission.GROUP_EDIT, 'user:view']) });
    await screen.findByText('Ops');
    await confirmDelete();
    await vi.waitFor(() =>
      expect(api.deleteGroup).toHaveBeenCalledWith({ body: { uuid: 'g1' } })
    );
    expect(toast.success).toHaveBeenCalledWith(
      'Group Ops deleted successfully'
    );
    const predicate = vi.mocked(invalidate).mock.calls.at(-1)?.[0] as (
      u: URL
    ) => boolean;
    expect(predicate(new URL('http://x/api/group'))).toBe(true);
    expect(predicate(new URL('http://x/api/group/g1'))).toBe(true);
    expect(predicate(new URL('http://x/api/user'))).toBe(false);
  });

  it('toasts a failed delete', async () => {
    api.deleteGroup.mockResolvedValue({ error: 'x' });
    render(Groups, { data: data([Permission.GROUP_EDIT, 'user:view']) });
    await screen.findByText('Ops');
    await confirmDelete();
    await vi.waitFor(() =>
      expect(sonnerError).toHaveBeenCalledWith('Failed to delete group')
    );
  });
});

describe('users list', () => {
  const data = (perms: string[]) =>
    ({
      error: null,
      user: me(...perms),
      users: Promise.resolve([
        { email: 'b@x', groups: [], name: 'Bob', uuid: 'u1' }
      ])
    }) as never;

  it('enables create and row actions with user:edit', async () => {
    render(Users, { data: data([Permission.USER_EDIT]) });
    await vi.waitFor(() => expect(screen.getByText('Bob')).toBeInTheDocument());
    await vi.waitFor(() => expect(iconButtons('trash')[0]).toBeEnabled());
  });

  it('disables row actions without user:edit', async () => {
    render(Users, { data: data([]) });
    await screen.findByText('Bob');
    expect(iconButtons('trash')[0]).toBeDisabled();
  });

  it('deletes a user and invalidates the management endpoints', async () => {
    api.deleteUser.mockResolvedValue({ data: {} });
    render(Users, { data: data([Permission.USER_EDIT]) });
    await screen.findByText('Bob');
    await vi.waitFor(() => expect(iconButtons('trash')[0]).toBeEnabled());
    await confirmDelete();
    await vi.waitFor(() =>
      expect(api.deleteUser).toHaveBeenCalledWith({ body: { uuid: 'u1' } })
    );
    expect(toast.success).toHaveBeenCalledWith('User Bob deleted successfully');
    const predicate = vi.mocked(invalidate).mock.calls.at(-1)?.[0] as (
      u: URL
    ) => boolean;
    expect(predicate(new URL('http://x/api/user/management'))).toBe(true);
    expect(predicate(new URL('http://x/api/group'))).toBe(false);
  });

  it('toasts a failed delete', async () => {
    api.deleteUser.mockResolvedValue({ error: 'x' });
    render(Users, { data: data([Permission.USER_EDIT]) });
    await screen.findByText('Bob');
    await vi.waitFor(() => expect(iconButtons('trash')[0]).toBeEnabled());
    await confirmDelete();
    await vi.waitFor(() =>
      expect(sonnerError).toHaveBeenCalledWith('Failed to delete user')
    );
  });
});

describe('tokens list', () => {
  // The SDK transformers hand the page real `Date`s.
  const past = new Date(Date.now() - 86_400_000);
  const future = new Date(Date.now() + 86_400_000);
  const data = (tokens: object[] | undefined) =>
    ({ error: null, tokens: Promise.resolve(tokens) }) as never;
  const expiredBtn = () =>
    screen.getByRole('button', { name: /delete expired/i });

  it('disables delete-expired when nothing has expired', async () => {
    render(Tokens, { data: data([{ exp: future, name: 'ci', uuid: 't1' }]) });
    await screen.findByText('ci');
    expect(expiredBtn()).toBeDisabled();
  });

  it('treats a missing token list as empty', async () => {
    render(Tokens, { data: data(undefined) });
    expect(await screen.findByText('No results.')).toBeInTheDocument();
    expect(expiredBtn()).toBeDisabled();
  });

  it.each([
    [2, 'Deleted 2 expired token(s)'],
    [0, 'No expired tokens to delete']
  ])('deletes expired tokens (%d)', async (deleted, message) => {
    api.deleteExpiredTokens.mockResolvedValue({ data: { deleted } });
    render(Tokens, {
      data: data([
        { exp: past, name: 'old', uuid: 't1' },
        { exp: future, name: 'new', uuid: 't2' }
      ])
    });
    await screen.findByText('old');
    await vi.waitFor(() => expect(expiredBtn()).toBeEnabled());
    await fireEvent.click(expiredBtn());
    expect(
      await screen.findByText('Permanently delete 1 expired token(s)?')
    ).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Delete' }));
    await vi.waitFor(() => expect(toast.success).toHaveBeenCalledWith(message));
    expect(invalidate).toHaveBeenCalledWith('/api/token');
  });

  it('toasts a failed expired delete', async () => {
    api.deleteExpiredTokens.mockResolvedValue({ error: 'x' });
    render(Tokens, { data: data([{ exp: past, name: 'old', uuid: 't1' }]) });
    await screen.findByText('old');
    await vi.waitFor(() => expect(expiredBtn()).toBeEnabled());
    await fireEvent.click(expiredBtn());
    await fireEvent.click(
      await screen.findByRole('button', { name: 'Delete' })
    );
    await vi.waitFor(() =>
      expect(sonnerError).toHaveBeenCalledWith(
        'Failed to delete expired tokens'
      )
    );
  });

  it('deletes a single token and refreshes the list', async () => {
    api.deleteToken.mockResolvedValue({ data: {} });
    render(Tokens, { data: data([{ exp: future, name: 'ci', uuid: 't1' }]) });
    await screen.findByText('ci');
    await fireEvent.click(iconButtons('trash').at(-1)!);
    await fireEvent.click(
      await screen.findByRole('button', { name: 'Delete' })
    );
    await vi.waitFor(() =>
      expect(api.deleteToken).toHaveBeenCalledWith({ body: { uuid: 't1' } })
    );
    expect(toast.success).toHaveBeenCalledWith('Token ci deleted successfully');
    expect(invalidate).toHaveBeenCalledWith('/api/token');
  });

  it('toasts a failed single delete', async () => {
    api.deleteToken.mockResolvedValue({ error: 'x' });
    render(Tokens, { data: data([{ exp: future, name: 'ci', uuid: 't1' }]) });
    await screen.findByText('ci');
    await fireEvent.click(iconButtons('trash').at(-1)!);
    await fireEvent.click(
      await screen.findByRole('button', { name: 'Delete' })
    );
    await vi.waitFor(() =>
      expect(sonnerError).toHaveBeenCalledWith('Failed to delete token')
    );
  });
});

describe('caches list', () => {
  const GiB = 1024 ** 3;
  const data = (caches: object[] | undefined, perms: string[] = []) =>
    ({
      caches:
        caches === undefined ? new Promise(() => {}) : Promise.resolve(caches),
      error: null,
      user: me(...perms)
    }) as never;

  it('shows a loading state until caches arrive', () => {
    render(Caches, { data: data(undefined) });
    expect(screen.getByText('Loading caches...')).toBeInTheDocument();
  });

  // Regression: a failed load used to leave "Loading caches..." forever.
  it('shows an error state when the load failed', async () => {
    render(Caches, {
      data: {
        caches: Promise.resolve(undefined),
        error: null,
        user: me()
      } as never
    });
    expect(
      await screen.findByText('Failed to load caches.')
    ).toBeInTheDocument();
    expect(screen.queryByText('Loading caches...')).toBeNull();
    expect(screen.queryByText(/No caches found/)).toBeNull();
  });

  it('shows an empty state', async () => {
    render(Caches, { data: data([]) });
    expect(
      await screen.findByText('No caches found. Create one to get started.')
    ).toBeInTheDocument();
  });

  it.each([
    [0.5, ''],
    [7.5, 'yellow'],
    [9.5, 'red']
  ])('renders usage %d/10 GiB with colour %j', async (used, colour) => {
    const { container } = render(Caches, {
      data: data([
        {
          name: 'main',
          public: true,
          quota: 10 * GiB,
          size: used * GiB,
          uuid: 'c1'
        }
      ])
    });
    expect(await screen.findByText('main')).toBeInTheDocument();
    expect(screen.getByText('Public')).toBeInTheDocument();
    expect(
      screen.getByText(
        `${used.toFixed(2)}GiB / 10.00GiB (${(used * 10).toFixed(1)}%)`
      )
    ).toBeInTheDocument();
    expect(screen.getByText('main').closest('a')).toHaveAttribute(
      'href',
      '/caches/c1'
    );
    const bar = container.querySelector('[data-slot="progress"]')!;
    for (const c of ['yellow', 'red']) {
      expect(bar.className.includes(`bg-${c}-500`)).toBe(c === colour);
    }
  });

  it('treats a missing size as empty and marks private caches', async () => {
    render(Caches, {
      data: data([{ name: 'p', public: false, quota: GiB, uuid: 'c2' }])
    });
    expect(await screen.findByText('Private')).toBeInTheDocument();
    expect(screen.getByText('0.00GiB / 1.00GiB (0.0%)')).toBeInTheDocument();
  });

  it('gates create on cache:create', async () => {
    render(Caches, { data: data([], [Permission.CACHE_CREATE]) });
    await vi.waitFor(() =>
      expect(screen.getByText('Create').closest('a')).not.toHaveAttribute(
        'aria-disabled',
        'true'
      )
    );
  });
});
