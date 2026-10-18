import { renderComponent } from '@tanstack/svelte-table';
import {
  type TableColumnDef,
  createColumn
} from '@profidev/pleiades/components/table/helpers.svelte';
import { Permission } from '$lib/permissions.svelte';
import type { GroupInfo, SimpleUserInfo, UserInfo } from '$lib/client';
import Actions from '@profidev/pleiades/components/table/actions.svelte';

/** A list row plus the derived search field added by `searchable`. */
export type GroupRow = GroupInfo & { user_names?: string };

export const columns = ({
  deleteGroup,
  user,
  admin_group
}: {
  deleteGroup: (group: GroupInfo) => void;
  user?: UserInfo;
  admin_group?: string;
}): TableColumnDef<GroupRow>[] => [
  createColumn('name', 'Name'),
  createColumn(
    'permissions',
    'Permissions',
    (permissions: string[]) => permissions.join(', ') || '-'
  ),
  createColumn(
    'users',
    'Users',
    (users: SimpleUserInfo[]) => users.map((u) => u.name).join(', ') || '-'
  ),
  createColumn('id', 'UUID'),
  {
    accessorKey: 'actions',
    cell: ({ row }) => {
      const disabled = !user
        ? true
        : !user?.permissions.includes(Permission.GROUP_EDIT) ||
          row.original.permissions.some(
            (p) => !user?.permissions.includes(p) // oxlint-disable-line no-unsafe-type-assertion
          );

      return renderComponent(Actions, {
        delete_disabled: disabled || row.original.id === admin_group,
        edit: `/groups/${row.original.id}`,
        edit_disabled: disabled,
        remove: () => deleteGroup(row.original)
      });
    },
    enableHiding: false,
    header: () => {}
  }
];

/**
 * The table search stringifies each search column; `users` is an object
 * array, so expose the member names as a plain searchable string.
 */
export const searchable = (groups: GroupInfo[]): GroupRow[] =>
  groups.map((group) => ({
    ...group,
    user_names: group.users.map((u) => u.name).join(', ')
  }));
