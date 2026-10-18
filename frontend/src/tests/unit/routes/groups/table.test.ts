import { describe, expect, it, vi } from 'vitest';
import { columns, searchable } from '$routes/groups/table.svelte';
import { Permission } from '$lib/permissions.svelte';
import type { GroupInfo, UserInfo } from '$lib/client';
import { renderCell } from '$test_helpers/load';

const actionsProps = (
  args: Parameters<typeof columns>[0],
  original: Partial<GroupInfo>
) => {
  const cols = columns(args);
  const actions = cols.find(
    (c) => 'accessorKey' in c && c.accessorKey === 'actions'
  );
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const rendered = (actions as any).cell({ row: { original } });
  return rendered.props as {
    delete_disabled: boolean;
    edit: string;
    edit_disabled: boolean;
  };
};

const admin = {
  permissions: [Permission.GROUP_EDIT, Permission.USER_VIEW]
} as unknown as UserInfo;

describe('groups table columns', () => {
  it('builds the expected set of columns ending in actions', () => {
    const cols = columns({ deleteGroup: () => {} });
    expect(cols.length).toBe(5);
    expect(cols.at(-1)).toMatchObject({ accessorKey: 'actions' });
  });

  it('disables actions entirely when there is no user', () => {
    const props = actionsProps({ deleteGroup: () => {} }, { id: 'g1' });
    expect(props.edit_disabled).toBe(true);
    expect(props.delete_disabled).toBe(true);
  });

  it('disables actions when the user lacks group:edit', () => {
    const user = {
      permissions: [Permission.GROUP_VIEW]
    } as unknown as UserInfo;
    const props = actionsProps(
      { deleteGroup: () => {}, user },
      { id: 'g1', permissions: [] }
    );
    expect(props.edit_disabled).toBe(true);
  });

  it('disables actions when the group has a permission the user lacks', () => {
    const props = actionsProps(
      { deleteGroup: () => {}, user: admin },
      { id: 'g1', permissions: ['settings:edit'] }
    );
    expect(props.edit_disabled).toBe(true);
  });

  it('enables actions when the user can edit and outranks the group', () => {
    const props = actionsProps(
      { deleteGroup: () => {}, user: admin },
      { id: 'g1', permissions: ['user:view'] }
    );
    expect(props.edit_disabled).toBe(false);
    expect(props.delete_disabled).toBe(false);
    expect(props.edit).toBe('/groups/g1');
  });

  it('still blocks deletion of the configured admin group', () => {
    const props = actionsProps(
      { admin_group: 'g1', deleteGroup: () => {}, user: admin },
      { id: 'g1', permissions: ['user:view'] }
    );
    expect(props.edit_disabled).toBe(false);
    expect(props.delete_disabled).toBe(true);
  });
});

describe('groups table formatters', () => {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const cols = columns({ deleteGroup: () => {} }) as any[];
  const col = (key: string) => cols.find((c) => c.accessorKey === key);

  it.each([
    ['permissions', { permissions: ['a:b', 'c:d'] }, 'a:b, c:d'],
    ['permissions', { permissions: [] }, '-'],
    ['users', { users: [{ name: 'Ann' }, { name: 'Bob' }] }, 'Ann, Bob'],
    ['users', { users: [] }, '-'],
    ['name', { name: 'Admins' }, 'Admins']
  ])('formats %s', async (key, row, text) => {
    expect((await renderCell(col(key), row)).textContent).toBe(text);
  });

  it('calls deleteGroup with the row', () => {
    const deleteGroup = vi.fn();
    const group = { id: 'g1', permissions: [] } as unknown as GroupInfo;
    const c = columns({ deleteGroup, user: admin });
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (c.at(-1) as any).cell({ row: { original: group } }).props.remove();
    expect(deleteGroup).toHaveBeenCalledWith(group);
  });
});

// Regression: the table search stringified the `users` objects to
// "[object Object]", so groups could not be found by member name.
describe('groups searchable rows', () => {
  it('adds the member names as a searchable string', () => {
    const rows = searchable([
      {
        id: 'g1',
        name: 'Admins',
        permissions: [],
        users: [
          { id: 'u1', name: 'Bob' },
          { id: 'u2', name: 'Cara' }
        ]
      },
      { id: 'g2', name: 'Staff', permissions: [], users: [] }
    ]);
    expect(rows.map((r) => r.user_names)).toEqual(['Bob, Cara', '']);
  });
});
