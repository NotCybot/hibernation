import { renderComponent } from '@tanstack/svelte-table';
import type { RowData } from '@tanstack/table-core';
import {
  type TableColumnDef,
  createColumnCell
} from '@profidev/pleiades/components/table/helpers.svelte';
import TableHead from './TableHead.svelte';
import Actions from './Actions.svelte';
import type { SearchResult } from '$lib/client';

const createColumn = <C extends RowData>(
  key: string,
  title: string,
  formatter?: (value: any) => string
): TableColumnDef<C> => ({
  accessorKey: key,
  ...createColumnCell(key, formatter),
  header: () =>
    renderComponent(TableHead, {
      title
    })
});

export const columns = ({
  write_access,
  delete_path
}: {
  write_access: boolean;
  delete_path: (path: string) => void;
}): TableColumnDef<SearchResult>[] => [
  createColumn('store_path', 'StorePath'),
  createColumn(
    'size',
    'Size',
    (value: number) => `${(value / (1024 * 1024)).toFixed(2)} MiB`
  ),
  createColumn('created_at', 'Created At', (value: Date) =>
    new Date(value).toLocaleString(navigator.languages || [navigator.language])
  ),
  createColumn(
    'last_accessed_at',
    'Last Accessed At',
    (value: Date | null | undefined) =>
      value
        ? new Date(value).toLocaleString(
            navigator.languages || [navigator.language]
          )
        : 'Never'
  ),
  createColumn('accessed', 'Access Count'),
  {
    accessorKey: 'actions',
    cell: ({ row }) =>
      renderComponent(Actions, {
        delete_disabled: !write_access,
        remove: () => delete_path(row.original.store_path)
      }),
    enableHiding: false,
    header: () => {}
  }
];
