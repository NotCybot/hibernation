import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { goto } from '$app/navigation';
import { Permission } from '$lib/permissions.svelte';

const api = { deleteGroup: vi.fn(), editGroup: vi.fn() };
vi.mock('$lib/client', async (orig) => ({
  ...(await orig<object>()),
  deleteGroup: (...a: unknown[]) => api.deleteGroup(...a),
  editGroup: (...a: unknown[]) => api.editGroup(...a)
}));
const toastSuccess = vi.fn();
vi.mock('@profidev/pleiades/components/util/general', async (orig) => ({
  ...(await orig<object>()),
  toast: { error: vi.fn(), success: (m: string) => toastSuccess(m) }
}));
const sonnerError = vi.fn();
vi.mock('svelte-sonner', async (orig) => ({
  ...(await orig<object>()),
  toast: { error: (m: string) => sonnerError(m) }
}));

const Page = (await import('$routes/groups/[uuid]/+page.svelte')).default;

const group = {
  caches: [{ access_type: 'View', name: 'c', uuid: 'c1' }],
  id: 'g1',
  name: 'Ops',
  permissions: ['group:view'],
  users: [{ id: 'u1', name: 'Bob' }]
};
const ALL = Object.values(Permission);

const data = (
  opts: {
    res?: object;
    perms?: string[];
    admin?: string;
  } = {}
) =>
  ({
    cachesPromise: Promise.resolve({ data: [{ name: 'c', uuid: 'c1' }] }),
    groupRes: Promise.resolve(
      opts.res ?? { data: { admin_group: opts.admin ?? 'adm', group } }
    ),
    user: Promise.resolve({ permissions: opts.perms ?? ALL, uuid: 'me' }),
    usersPromise: Promise.resolve({ data: [{ id: 'u1', name: 'Bob' }] }),
    uuid: 'g1'
  }) as never;

const save = async () =>
  fireEvent.click(screen.getByRole('button', { name: /save changes/i }));

afterEach(() => {
  api.deleteGroup.mockReset();
  api.editGroup.mockReset();
  toastSuccess.mockReset();
  sonnerError.mockReset();
});

describe('group detail page', () => {
  it.each([
    [404, '/groups?error=not_found'],
    [500, '/groups?error=other']
  ])('redirects a %d to the list', async (status, target) => {
    render(Page, { data: data({ res: { response: { status } } }) });
    await vi.waitFor(() => expect(goto).toHaveBeenCalledWith(target));
  });

  it('fills the form from the group', async () => {
    render(Page, { data: data() });
    expect(await screen.findByText('Group: Ops')).toBeInTheDocument();
    await vi.waitFor(() =>
      expect(screen.getByLabelText('Group Name')).toHaveValue('Ops')
    );
    expect(screen.getByLabelText('View Groups')).toBeChecked();
    expect(screen.getByLabelText('Edit Groups')).not.toBeChecked();
  });

  it('saves the edited group with its cache mappings', async () => {
    api.editGroup.mockResolvedValue({ data: {} });
    render(Page, { data: data() });
    await vi.waitFor(() =>
      expect(screen.getByLabelText('Group Name')).toHaveValue('Ops')
    );
    await fireEvent.input(screen.getByLabelText('Group Name'), {
      target: { value: 'Ops2' }
    });
    await fireEvent.click(screen.getByLabelText('Edit Groups'));
    await save();
    await vi.waitFor(() => expect(api.editGroup).toHaveBeenCalledOnce());
    const [[{ body }]] = api.editGroup.mock.calls as [
      [{ body: { name: string; permissions: string[]; caches: object[] } }]
    ];
    expect(body).toMatchObject({
      caches: group.caches,
      name: 'Ops2',
      uuid: 'g1'
    });
    expect(body.permissions.toSorted()).toEqual(['group:edit', 'group:view']);
    expect(toastSuccess).toHaveBeenCalledWith('Group Ops updated successfully');
  });

  it('keeps the admin group permissions untouched and hides the editor', async () => {
    api.editGroup.mockResolvedValue({ data: {} });
    render(Page, { data: data({ admin: 'g1' }) });
    await vi.waitFor(() =>
      expect(screen.getByLabelText('Group Name')).toHaveValue('Ops')
    );
    expect(screen.queryByLabelText('View Groups')).toBeNull();
    expect(screen.getByRole('button', { name: /delete/i })).toBeDisabled();
    await save();
    await vi.waitFor(() => expect(api.editGroup).toHaveBeenCalledOnce());
    const [[{ body }]] = api.editGroup.mock.calls as [
      [{ body: { permissions: string[] } }]
    ];
    expect(body.permissions).toEqual(['group:view']);
  });

  it.each([
    [409, 'This group name is already in use'],
    [406, 'Admin group must have at least 1 user']
  ])('maps a %d to a field error', async (status, message) => {
    api.editGroup.mockResolvedValue({ error: 'x', response: { status } });
    render(Page, { data: data() });
    await vi.waitFor(() =>
      expect(screen.getByLabelText('Group Name')).toHaveValue('Ops')
    );
    await save();
    expect(await screen.findByText(message)).toBeInTheDocument();
  });

  it('toasts other save errors', async () => {
    api.editGroup.mockResolvedValue({ error: 'x', response: { status: 500 } });
    render(Page, { data: data() });
    await vi.waitFor(() =>
      expect(screen.getByLabelText('Group Name')).toHaveValue('Ops')
    );
    await save();
    await vi.waitFor(() =>
      expect(sonnerError).toHaveBeenCalledWith('Failed to update group')
    );
  });

  it('is readonly without group:edit', async () => {
    render(Page, { data: data({ perms: [Permission.GROUP_VIEW] }) });
    await vi.waitFor(() =>
      expect(screen.getByLabelText('Group Name')).toBeDisabled()
    );
    expect(await screen.findByLabelText('View Groups')).toBeDisabled();
    expect(screen.getByRole('button', { name: /delete/i })).toBeDisabled();
    // Regression: Save and the member picker used to stay enabled.
    expect(
      screen.getByRole('button', { name: /save changes/i })
    ).toBeDisabled();
    // The first combobox is the member picker (cache mappings follow).
    expect(screen.getAllByRole('combobox')[0]).toBeDisabled();
  });

  it('deletes the group and returns to the list', async () => {
    api.deleteGroup.mockResolvedValue({ data: {} });
    render(Page, { data: data() });
    await screen.findByText('Group: Ops');
    await fireEvent.click(screen.getByRole('button', { name: /delete/i }));
    await fireEvent.click(
      screen.getAllByRole('button', { name: 'Delete' }).at(-1)!
    );
    await vi.waitFor(() =>
      expect(api.deleteGroup).toHaveBeenCalledWith({ body: { uuid: 'g1' } })
    );
    expect(toastSuccess).toHaveBeenCalledWith('Group Ops deleted successfully');
    await vi.waitFor(() => expect(goto).toHaveBeenCalledWith('/groups'));
  });

  it('toasts a failed delete', async () => {
    api.deleteGroup.mockResolvedValue({ error: 'x' });
    render(Page, { data: data() });
    await screen.findByText('Group: Ops');
    await fireEvent.click(screen.getByRole('button', { name: /delete/i }));
    await fireEvent.click(
      screen.getAllByRole('button', { name: 'Delete' }).at(-1)!
    );
    await vi.waitFor(() =>
      expect(sonnerError).toHaveBeenCalledWith('Failed to delete group')
    );
  });
});
