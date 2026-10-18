import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { goto } from '$app/navigation';
import { Permission } from '$lib/permissions.svelte';

const api = {
  changeUserEmail: vi.fn(),
  convertOidcUser: vi.fn(),
  deleteUser: vi.fn(),
  editUser: vi.fn(),
  resetUserAvatar: vi.fn(),
  resetUserPassword: vi.fn()
};
vi.mock('$lib/client', async (orig) => ({
  ...(await orig<object>()),
  changeUserEmail: (...a: unknown[]) => api.changeUserEmail(...a),
  convertOidcUser: (...a: unknown[]) => api.convertOidcUser(...a),
  deleteUser: (...a: unknown[]) => api.deleteUser(...a),
  editUser: (...a: unknown[]) => api.editUser(...a),
  resetUserAvatar: (...a: unknown[]) => api.resetUserAvatar(...a),
  resetUserPassword: (...a: unknown[]) => api.resetUserPassword(...a)
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
const encrypt = { encrypt: (s: string) => `enc(${s})` };
const getEncrypt = vi.fn((): typeof encrypt | undefined => encrypt);
vi.mock('$lib/backend/auth.svelte.js', () => ({
  getEncrypt: () => getEncrypt()
}));

const Page = (await import('$routes/users/[uuid]/+page.svelte')).default;

const ALL = Object.values(Permission);
const target = (over: object = {}) => ({
  caches: [{ access_type: 'Edit', name: 'c', uuid: 'c1' }],
  email: 'bob@x.io',
  groups: [{ name: 'Ops', uuid: 'g1' }],
  name: 'Bob',
  oidc_user: false,
  permissions: ['user:view'],
  uuid: 'u1',
  ...over
});
const data = (opts: { res?: object; perms?: string[]; user?: object } = {}) =>
  ({
    cachesPromise: Promise.resolve({ data: [{ name: 'c', uuid: 'c1' }] }),
    groupsPromise: Promise.resolve([{ name: 'Ops', uuid: 'g1' }]),
    user: Promise.resolve({ permissions: opts.perms ?? ALL, uuid: 'me' }),
    userInfoPromise: Promise.resolve(opts.res ?? { data: target(opts.user) }),
    uuid: 'u1'
  }) as never;

const loaded = async () =>
  vi.waitFor(() =>
    expect(screen.getByLabelText('User Name')).toHaveValue('Bob')
  );
const btn = (name: RegExp | string) =>
  screen.getAllByRole('button', { name })[0];
const confirm = async (name: string) =>
  fireEvent.click(screen.getAllByRole('button', { name }).at(-1)!);
const type = async (label: string, value: string) =>
  fireEvent.input(await screen.findByLabelText(label), { target: { value } });

beforeEach(() => getEncrypt.mockImplementation(() => encrypt));
afterEach(() => {
  for (const f of Object.values(api)) {
    f.mockReset();
  }
  toast.error.mockReset();
  toast.success.mockReset();
  sonnerError.mockReset();
});

describe('user detail page: loading', () => {
  it.each([
    [404, '/users?error=not_found'],
    [500, '/users?error=other']
  ])('redirects a %d to the list', async (status, to) => {
    render(Page, { data: data({ res: { response: { status } } }) });
    await vi.waitFor(() => expect(goto).toHaveBeenCalledWith(to));
  });

  it('shows the user and the local-user actions', async () => {
    render(Page, { data: data() });
    await loaded();
    expect(screen.getByText('User: Bob')).toBeInTheDocument();
    expect(screen.getByDisplayValue('bob@x.io')).toBeInTheDocument();
    expect(btn(/reset password/i)).toBeEnabled();
    expect(btn(/change email/i)).toBeEnabled();
    expect(screen.queryByRole('button', { name: /convert/i })).toBeNull();
  });

  it('offers conversion instead of password/email for OIDC users', async () => {
    render(Page, { data: data({ user: { oidc_user: true } }) });
    await loaded();
    expect(btn(/convert to local user/i)).toBeEnabled();
    expect(
      screen.queryByRole('button', { name: /reset password/i })
    ).toBeNull();
    expect(screen.queryByRole('button', { name: /change email/i })).toBeNull();
  });

  it('hides special edits when the target outranks the editor', async () => {
    render(Page, {
      data: data({
        perms: [Permission.USER_EDIT],
        user: { permissions: ['settings:edit'] }
      })
    });
    await loaded();
    expect(
      screen.queryByRole('button', { name: /reset password/i })
    ).toBeNull();
    expect(screen.queryByRole('button', { name: /convert/i })).toBeNull();
  });

  it('is readonly without user:edit', async () => {
    render(Page, { data: data({ perms: [Permission.USER_VIEW] }) });
    await loaded();
    expect(screen.getByLabelText('User Name')).toBeDisabled();
    expect(btn(/^delete$/i)).toBeDisabled();
    expect(btn(/reset avatar/i)).toBeDisabled();
    expect(btn(/save changes/i)).toBeDisabled();
  });

  it("gates cache access on the editor's cache:edit, not the target's", async () => {
    render(Page, {
      data: data({
        perms: [Permission.USER_EDIT, Permission.USER_VIEW],
        user: { permissions: [Permission.CACHE_EDIT] }
      })
    });
    await loaded();
    for (const b of screen
      .getAllByRole('button')
      .filter((x) => x.classList.contains('min-w-10'))) {
      expect(b).toBeDisabled();
    }
  });

  it('enables cache access when the editor has cache:edit', async () => {
    render(Page, { data: data() });
    await loaded();
    await vi.waitFor(() => {
      const del = screen
        .getAllByRole('button')
        .find((x) => x.classList.contains('min-w-10'));
      expect(del).toBeEnabled();
    });
  });
});

describe('user detail page: save', () => {
  it('saves name, groups and cache mappings', async () => {
    api.editUser.mockResolvedValue({ data: {} });
    render(Page, { data: data() });
    await loaded();
    await type('User Name', 'Robert');
    await fireEvent.click(btn(/save changes/i));
    await vi.waitFor(() =>
      expect(api.editUser).toHaveBeenCalledWith({
        body: {
          caches: target().caches,
          groups: ['g1'],
          name: 'Robert',
          uuid: 'u1'
        }
      })
    );
    expect(toast.success).toHaveBeenCalledWith('User Bob updated successfully');
  });

  it('maps a 409 to a groups field error', async () => {
    api.editUser.mockResolvedValue({ error: 'x', response: { status: 409 } });
    render(Page, { data: data() });
    await loaded();
    await fireEvent.click(btn(/save changes/i));
    expect(
      await screen.findByText(
        'Cannot remove the last user from the admin group'
      )
    ).toBeInTheDocument();
  });

  it.each([
    [403, 'Cannot assign permissions that you do not have'],
    [500, 'Failed to update user']
  ])('toasts a %d', async (status, message) => {
    api.editUser.mockResolvedValue({ error: 'x', response: { status } });
    render(Page, { data: data() });
    await loaded();
    await fireEvent.click(btn(/save changes/i));
    await vi.waitFor(() => expect(sonnerError).toHaveBeenCalledWith(message));
  });
});

describe('user detail page: actions', () => {
  it.each([
    [{ data: {} }, 'success', 'Avatar reset successfully'],
    [{ error: 'x' }, 'error', 'Failed to reset avatar']
  ] as const)('resets the avatar (%#)', async (res, kind, message) => {
    api.resetUserAvatar.mockResolvedValue(res);
    render(Page, { data: data() });
    await loaded();
    await fireEvent.click(btn(/reset avatar/i));
    await vi.waitFor(() => expect(toast[kind]).toHaveBeenCalledWith(message));
    expect(api.resetUserAvatar).toHaveBeenCalledWith({ body: { uuid: 'u1' } });
  });

  it('deletes the user', async () => {
    api.deleteUser.mockResolvedValue({ data: {} });
    render(Page, { data: data() });
    await loaded();
    await fireEvent.click(btn(/^delete$/i));
    await confirm('Delete');
    await vi.waitFor(() => expect(goto).toHaveBeenCalledWith('/users'));
    expect(toast.success).toHaveBeenCalledWith('User Bob deleted successfully');
  });

  it.each([
    [409, 'Cannot delete the last user from the admin group'],
    [403, 'You do not have permission to delete this user'],
    [500, 'Failed to delete user']
  ])('toasts a failed delete (%d)', async (status, message) => {
    api.deleteUser.mockResolvedValue({ error: 'x', response: { status } });
    render(Page, { data: data() });
    await loaded();
    await fireEvent.click(btn(/^delete$/i));
    await confirm('Delete');
    await vi.waitFor(() => expect(sonnerError).toHaveBeenCalledWith(message));
  });

  const resetPw = async () => {
    render(Page, { data: data() });
    await loaded();
    await fireEvent.click(btn(/reset password/i));
    await type('New Password', 'secret1');
    await confirm('Reset');
  };

  it('resets the password encrypted', async () => {
    api.resetUserPassword.mockResolvedValue({ data: {} });
    await resetPw();
    await vi.waitFor(() =>
      expect(api.resetUserPassword).toHaveBeenCalledWith({
        body: { new_password: 'enc(secret1)', uuid: 'u1' }
      })
    );
    expect(toast.success).toHaveBeenCalledWith(
      'Password for user Bob reset successfully'
    );
  });

  it.each([
    [403, 'You do not have permission to reset this password'],
    [500, 'Failed to reset password']
  ])('toasts a failed password reset (%d)', async (status, message) => {
    api.resetUserPassword.mockResolvedValue({
      error: 'x',
      response: { status }
    });
    await resetPw();
    await vi.waitFor(() => expect(sonnerError).toHaveBeenCalledWith(message));
  });

  it('refuses a password reset without encryption', async () => {
    getEncrypt.mockImplementation(() => undefined);
    await resetPw();
    await vi.waitFor(() =>
      expect(sonnerError).toHaveBeenCalledWith(
        'Encryption function not available'
      )
    );
    expect(api.resetUserPassword).not.toHaveBeenCalled();
  });

  const convert = async () => {
    render(Page, { data: data({ user: { oidc_user: true } }) });
    await loaded();
    await fireEvent.click(btn(/convert to local user/i));
    await type('New Password', 'secret1');
    await confirm('Convert');
  };

  it('converts an OIDC user', async () => {
    api.convertOidcUser.mockResolvedValue({ data: {} });
    await convert();
    await vi.waitFor(() =>
      expect(api.convertOidcUser).toHaveBeenCalledWith({
        body: { new_password: 'enc(secret1)', uuid: 'u1' }
      })
    );
    expect(toast.success).toHaveBeenCalledWith(
      'User Bob converted to local user successfully'
    );
  });

  it.each([
    [403, 'You do not have permission to convert this user'],
    [500, 'Failed to convert user']
  ])('toasts a failed conversion (%d)', async (status, message) => {
    api.convertOidcUser.mockResolvedValue({ error: 'x', response: { status } });
    await convert();
    await vi.waitFor(() => expect(sonnerError).toHaveBeenCalledWith(message));
  });

  it('refuses conversion without encryption', async () => {
    getEncrypt.mockImplementation(() => undefined);
    await convert();
    await vi.waitFor(() =>
      expect(sonnerError).toHaveBeenCalledWith(
        'Encryption function not available'
      )
    );
  });

  const changeEmail = async () => {
    render(Page, { data: data() });
    await loaded();
    await fireEvent.click(btn(/change email/i));
    await type('New Email', 'new@x.io');
    await confirm('Change');
  };

  it('changes the email', async () => {
    api.changeUserEmail.mockResolvedValue({ data: {} });
    await changeEmail();
    await vi.waitFor(() =>
      expect(api.changeUserEmail).toHaveBeenCalledWith({
        body: { new_email: 'new@x.io', uuid: 'u1' }
      })
    );
    expect(toast.success).toHaveBeenCalledWith(
      'Email for user Bob changed successfully'
    );
  });

  it('maps a 409 on email change to a field error', async () => {
    api.changeUserEmail.mockResolvedValue({
      error: 'x',
      response: { status: 409 }
    });
    await changeEmail();
    expect(await screen.findByText('Email already in use')).toBeInTheDocument();
  });

  it.each([
    [403, 'You do not have permission to change this email'],
    [500, 'Failed to change email']
  ])('toasts a failed email change (%d)', async (status, message) => {
    api.changeUserEmail.mockResolvedValue({ error: 'x', response: { status } });
    await changeEmail();
    await vi.waitFor(() => expect(sonnerError).toHaveBeenCalledWith(message));
  });
});
