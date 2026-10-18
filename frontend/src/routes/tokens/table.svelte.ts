import { renderComponent } from '@tanstack/svelte-table';
import {
  type TableColumnDef,
  createColumn
} from '@profidev/pleiades/components/table/helpers.svelte';
import type { TokenInfo } from '$lib/client';
import Actions from '@profidev/pleiades/components/table/actions.svelte';

export const columns = ({
  deleteToken
}: {
  deleteToken: (token: TokenInfo) => void;
}): TableColumnDef<TokenInfo>[] => [
  createColumn('name', 'Name'),
  createColumn('exp', 'Expires At', (value: Date) =>
    new Date(value).toLocaleString(navigator.languages || [navigator.language])
  ),
  createColumn('last_used', 'Last Used', (value: Date | null | undefined) =>
    value
      ? new Date(value).toLocaleString(
          navigator.languages || [navigator.language]
        )
      : 'Never'
  ),
  createColumn('uuid', 'UUID'),
  {
    accessorKey: 'actions',
    cell: ({ row }) =>
      renderComponent(Actions, {
        delete_disabled: false,
        edit: `/tokens/${row.original.uuid}`,
        edit_disabled: false,
        remove: () => deleteToken(row.original)
      }),
    enableHiding: false,
    header: () => {}
  }
];
