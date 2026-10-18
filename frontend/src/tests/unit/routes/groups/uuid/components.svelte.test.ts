import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { Permission } from '$lib/permissions.svelte';
import type { CacheMapping, UserInfo } from '$lib/client';
import CacheAccess from '$routes/groups/[uuid]/CacheAccess.svelte';
import Harness from './PermissionsHarness.svelte';

const user = (...permissions: Permission[]) =>
  ({ permissions }) as unknown as UserInfo;

const ALL_LABELS = [
  'View Settings',
  'Edit Settings',
  'View Groups',
  'Edit Groups',
  'View Users',
  'Edit Users',
  'Create Caches',
  'View all Caches',
  'Edit all Caches'
];

describe('group Permissions', () => {
  it('offers every checkbox to a user holding every permission', () => {
    render(Harness, {
      onsubmit: () => {},
      readonly: false,
      user: user(...Object.values(Permission))
    });
    for (const label of ALL_LABELS) {
      expect(screen.getByLabelText(label)).toBeEnabled();
    }
  });

  it('only offers permissions the editing user holds', () => {
    render(Harness, {
      onsubmit: () => {},
      readonly: false,
      user: user(Permission.GROUP_VIEW, Permission.CACHE_VIEW)
    });
    expect(screen.getByLabelText('View Groups')).toBeInTheDocument();
    expect(screen.getByLabelText('View all Caches')).toBeInTheDocument();
    for (const label of ALL_LABELS.filter(
      (l) => l !== 'View Groups' && l !== 'View all Caches'
    )) {
      expect(screen.queryByLabelText(label)).toBeNull();
    }
    // Section headers for resources the user cannot touch are hidden too.
    expect(screen.queryByText('Settings')).toBeNull();
    expect(screen.getByText('Groups')).toBeInTheDocument();
  });

  it('shows no checkboxes without a user', () => {
    render(Harness, { onsubmit: () => {}, readonly: false });
    for (const label of ALL_LABELS) {
      expect(screen.queryByLabelText(label)).toBeNull();
    }
  });

  it('disables every checkbox when readonly', () => {
    render(Harness, {
      onsubmit: () => {},
      readonly: true,
      user: user(...Object.values(Permission))
    });
    for (const label of ALL_LABELS) {
      expect(screen.getByLabelText(label)).toBeDisabled();
    }
  });

  it('writes toggled checkboxes into the form value', async () => {
    const onsubmit = vi.fn();
    render(Harness, {
      onsubmit,
      readonly: false,
      user: user(Permission.CACHE_CREATE, Permission.USER_EDIT)
    });
    await fireEvent.click(screen.getByLabelText('Create Caches'));
    await fireEvent.click(screen.getByLabelText('Edit Users'));
    await fireEvent.click(screen.getByRole('button', { name: 'Submit' }));
    await vi.waitFor(() =>
      expect(onsubmit).toHaveBeenCalledWith(
        expect.objectContaining({ cache$create: true, user$edit: true })
      )
    );
  });
});

const caches = [
  { name: 'alpha', uuid: 'a' },
  { name: 'beta', uuid: 'b' }
];

describe('group CacheAccess', () => {
  const setup = (mappings: CacheMapping[], disabled = false) => {
    const props = $state({ caches, disabled, mappings });
    render(CacheAccess, props);
    return props;
  };

  it('adds the first unmapped cache with view access', async () => {
    const props = setup([]);
    const buttons = screen.getAllByRole('button');
    await fireEvent.click(buttons.at(-1)!);
    expect(props.mappings).toEqual([
      { access_type: 'View', name: 'alpha', uuid: 'a' }
    ]);
  });

  it('hides the add button once every cache is mapped', () => {
    setup([
      { access_type: 'View', name: 'alpha', uuid: 'a' },
      { access_type: 'Edit', name: 'beta', uuid: 'b' }
    ]);
    // One delete button per mapping and no add button.
    expect(
      screen
        .getAllByRole('button')
        .filter((b) => b.classList.contains('min-w-10'))
    ).toHaveLength(2);
    expect(
      screen.getAllByRole('button').some((b) => b.classList.contains('mt-2'))
    ).toBe(false);
  });

  it('removes a mapping', async () => {
    const props = setup([
      { access_type: 'View', name: 'alpha', uuid: 'a' },
      { access_type: 'Edit', name: 'beta', uuid: 'b' }
    ]);
    const del = screen
      .getAllByRole('button')
      .find((b) => b.classList.contains('min-w-10'));
    await fireEvent.click(del!);
    expect(props.mappings).toEqual([
      { access_type: 'Edit', name: 'beta', uuid: 'b' }
    ]);
  });

  it('shows the access type of each mapping', () => {
    setup([{ access_type: 'Edit', name: 'alpha', uuid: 'a' }]);
    expect(screen.getByText('Edit')).toBeInTheDocument();
  });

  it('disables every control when disabled', () => {
    setup([{ access_type: 'View', name: 'alpha', uuid: 'a' }], true);
    for (const b of screen
      .getAllByRole('button')
      .filter(
        (x) => x.classList.contains('min-w-10') || x.classList.contains('mt-2')
      )) {
      expect(b).toBeDisabled();
    }
  });

  it('switches the mapped cache through the cache picker', async () => {
    const props = setup([{ access_type: 'Edit', name: 'alpha', uuid: 'a' }]);
    await fireEvent.click(screen.getByRole('combobox'));
    await fireEvent.click(await screen.findByRole('option', { name: 'beta' }));
    expect(props.mappings).toEqual([
      { access_type: 'Edit', name: 'beta', uuid: 'b' }
    ]);
  });

  it('changes the access type through the select', async () => {
    const props = setup([{ access_type: 'View', name: 'alpha', uuid: 'a' }]);
    const trigger = screen.getByText('View').closest('button')!;
    await fireEvent.pointerDown(trigger, { button: 0, pointerType: 'mouse' });
    await fireEvent.pointerUp(trigger, { button: 0, pointerType: 'mouse' });
    const option = await screen.findByRole('option', { name: 'Edit' });
    await fireEvent.pointerUp(option, { button: 0, pointerType: 'mouse' });
    await fireEvent.click(option);
    await vi.waitFor(() =>
      expect(props.mappings).toEqual([
        { access_type: 'Edit', name: 'alpha', uuid: 'a' }
      ])
    );
  });
});
