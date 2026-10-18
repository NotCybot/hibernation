import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { columns } from '$routes/caches/[uuid]/search/table.svelte';
import Actions from '$routes/caches/[uuid]/search/Actions.svelte';
import TableHead from '$routes/caches/[uuid]/search/TableHead.svelte';
import { renderCell } from '$test_helpers/load';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const col = (cols: any[], key: string) =>
  cols.find((c) => c.accessorKey === key);
const locale = (iso: string) =>
  new Date(iso).toLocaleString(navigator.languages);

describe('cache search columns', () => {
  const delete_path = vi.fn();
  const cols = columns({ delete_path, write_access: true });

  it('defines the columns in order', () => {
    expect(cols.map((c) => ('accessorKey' in c ? c.accessorKey : ''))).toEqual([
      'store_path',
      'size',
      'created_at',
      'last_accessed_at',
      'accessed',
      'actions'
    ]);
  });

  it.each([
    ['store_path', { store_path: 'abc-hello' }, 'abc-hello'],
    ['size', { size: 0 }, '0.00 MiB'],
    ['size', { size: 1024 * 1024 * 1.5 }, '1.50 MiB'],
    [
      'created_at',
      { created_at: '2030-01-02T03:04:05Z' },
      locale('2030-01-02T03:04:05Z')
    ],
    ['last_accessed_at', { last_accessed_at: undefined }, 'Never'],
    [
      'last_accessed_at',
      { last_accessed_at: '2030-01-02T03:04:05Z' },
      locale('2030-01-02T03:04:05Z')
    ],
    ['accessed', { accessed: 7 }, '7']
  ])('formats %s', async (key, row, text) => {
    const el = await renderCell(col(cols, key), row);
    expect(el.textContent).toBe(text);
  });

  it.each([
    ['store_path', 'StorePath'],
    ['size', 'Size'],
    ['created_at', 'Created At'],
    ['last_accessed_at', 'Last Accessed At'],
    ['accessed', 'Access Count']
  ])('renders the %s header as %s', async (key, title) => {
    const el = await renderCell(col(cols, key), {}, 'header');
    expect(el.textContent?.trim()).toBe(title);
  });

  it('wires the delete action to the row store path', () => {
    const { props } = col(cols, 'actions').cell({
      row: { original: { store_path: 'p1' } }
    });
    expect(props.delete_disabled).toBe(false);
    props.remove();
    expect(delete_path).toHaveBeenCalledWith('p1');
    expect(col(cols, 'actions').header()).toBeUndefined();
  });

  it('disables delete without write access', () => {
    const ro = columns({ delete_path, write_access: false });
    expect(
      col(ro, 'actions').cell({ row: { original: { store_path: 'p' } } }).props
        .delete_disabled
    ).toBe(true);
  });
});

describe('search Actions', () => {
  it('calls remove on click', async () => {
    const remove = vi.fn();
    render(Actions, { delete_disabled: false, remove });
    await fireEvent.click(
      screen.getByRole('button', { name: 'Delete store path' })
    );
    expect(remove).toHaveBeenCalledOnce();
  });

  it('is disabled when delete is not allowed', () => {
    render(Actions, { delete_disabled: true, remove: () => {} });
    expect(
      screen.getByRole('button', { name: 'Delete store path' })
    ).toBeDisabled();
  });
});

describe('search TableHead', () => {
  it('renders the title', () => {
    render(TableHead, { title: 'Size' });
    expect(screen.getByText('Size')).toBeInTheDocument();
  });
});
