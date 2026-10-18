import { describe, expect, it, vi } from 'vitest';
import { columns, searchable } from '$routes/users/table.svelte';
import { renderCell } from '$test_helpers/load';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const cellProps = (cols: any[], key: string, original: unknown) =>
  cols.find((c) => c.accessorKey === key).cell({ row: { original } }).props;

describe('users table columns', () => {
  it('renders an avatar column wired to the user identity', () => {
    const cols = columns({ canEdit: true, deleteUser: () => {} });
    const props = cellProps(cols, 'avatar', { name: 'Bob', uuid: 'u1' });
    expect(props).toMatchObject({ userId: 'u1', username: 'Bob' });
  });

  it('enables actions when the user can edit', () => {
    const cols = columns({ canEdit: true, deleteUser: () => {} });
    const props = cellProps(cols, 'actions', { uuid: 'u1' });
    expect(props.edit_disabled).toBe(false);
    expect(props.delete_disabled).toBe(false);
    expect(props.edit).toBe('/users/u1');
  });

  it('disables actions when the user cannot edit', () => {
    const cols = columns({ canEdit: false, deleteUser: () => {} });
    const props = cellProps(cols, 'actions', { uuid: 'u1' });
    expect(props.edit_disabled).toBe(true);
    expect(props.delete_disabled).toBe(true);
  });
});

describe('users table formatters', () => {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const cols = columns({ canEdit: true, deleteUser: () => {} }) as any[];
  const col = (key: string) => cols.find((c) => c.accessorKey === key);

  it.each([
    ['groups', { groups: [{ name: 'A' }, { name: 'B' }] }, 'A, B'],
    ['groups', { groups: [] }, '-'],
    ['email', { email: 'a@b.c' }, 'a@b.c']
  ])('formats %s', async (key, row, text) => {
    expect((await renderCell(col(key), row)).textContent).toBe(text);
  });

  it('calls deleteUser with the row', () => {
    const deleteUser = vi.fn();
    const row = { uuid: 'u1' };
    cellProps(columns({ canEdit: true, deleteUser }), 'actions', row).remove();
    expect(deleteUser).toHaveBeenCalledWith(row);
  });
});

// Regression: the table search stringified the `groups` objects to
// "[object Object]", so users could not be found by group name.
describe('users searchable rows', () => {
  it('adds the group names as a searchable string', () => {
    const rows = searchable([
      {
        email: 'b@x.io',
        groups: [
          { name: 'Admins', uuid: 'g1' },
          { name: 'Staff', uuid: 'g2' }
        ],
        name: 'Bob',
        uuid: 'u1'
      },
      { email: 'c@x.io', groups: [], name: 'Cara', uuid: 'u2' }
    ]);
    expect(rows?.map((r) => r.group_names)).toEqual(['Admins, Staff', '']);
    expect(rows?.[0].groups).toHaveLength(2);
  });

  it('passes a missing list through', () => {
    expect(searchable(undefined)).toBeUndefined();
  });
});
