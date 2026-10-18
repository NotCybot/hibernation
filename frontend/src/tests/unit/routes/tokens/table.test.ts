import { describe, expect, it, vi } from 'vitest';
import { columns } from '$routes/tokens/table.svelte';
import type { TokenInfo } from '$lib/client';
import { renderCell } from '$test_helpers/load';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const col = (cols: any[], key: string) =>
  cols.find((c) => c.accessorKey === key);

describe('tokens table columns', () => {
  const deleteToken = vi.fn();
  const cols = columns({ deleteToken });

  it('defines name, expiry, last used, uuid and actions in order', () => {
    expect(cols.map((c) => ('accessorKey' in c ? c.accessorKey : ''))).toEqual([
      'name',
      'exp',
      'last_used',
      'uuid',
      'actions'
    ]);
  });

  it('always enables edit and delete and links to the token page', () => {
    const token = { name: 'ci', uuid: 't1' } as TokenInfo;
    const { props } = col(cols, 'actions').cell({ row: { original: token } });
    expect(props).toMatchObject({
      delete_disabled: false,
      edit: '/tokens/t1',
      edit_disabled: false
    });
    props.remove();
    expect(deleteToken).toHaveBeenCalledWith(token);
  });

  it('renders no header for the actions column', () => {
    expect(col(cols, 'actions').header()).toBeUndefined();
    expect(col(cols, 'actions').enableHiding).toBe(false);
  });
});

describe('tokens table formatters', () => {
  const cols = columns({ deleteToken: () => {} });

  it('renders the raw name', async () => {
    const el = await renderCell(col(cols, 'name'), { name: 'ci' });
    expect(el.textContent).toBe('ci');
  });

  it('localises the expiry date', async () => {
    const exp = '2030-01-02T03:04:05.000Z';
    const el = await renderCell(col(cols, 'exp'), { exp });
    expect(el.textContent).toBe(
      new Date(exp).toLocaleString(navigator.languages)
    );
  });

  it.each([
    [undefined, 'Never'],
    [
      '2030-01-02T03:04:05.000Z',
      new Date('2030-01-02T03:04:05.000Z').toLocaleString(navigator.languages)
    ]
  ])('renders last_used %j as %j', async (last_used, text) => {
    const el = await renderCell(col(cols, 'last_used'), { last_used });
    expect(el.textContent).toBe(text);
  });

  it('renders a sortable header with the column title', async () => {
    const el = await renderCell(col(cols, 'exp'), {}, 'header');
    expect(el.textContent).toContain('Expires At');
  });
});
