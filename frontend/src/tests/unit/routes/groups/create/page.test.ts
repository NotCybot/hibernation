import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { goto } from '$app/navigation';

const createGroup = vi.fn();
vi.mock('$lib/client', async (orig) => ({
  ...(await orig<object>()),
  createGroup: (...a: unknown[]) => createGroup(...a)
}));
const toastSuccess = vi.fn();
vi.mock('@profidev/pleiades/components/util/general', async (orig) => ({
  ...(await orig<object>()),
  toast: { error: vi.fn(), success: (m: string) => toastSuccess(m) }
}));

const toastError = vi.fn();
vi.mock('svelte-sonner', async (orig) => ({
  ...(await orig<object>()),
  toast: { error: (m: string) => toastError(m) }
}));

const Page = (await import('$routes/groups/create/+page.svelte')).default;

const submit = async (name: string) => {
  render(Page);
  await fireEvent.input(screen.getByLabelText('Group Name'), {
    target: { value: name }
  });
  await fireEvent.click(screen.getByRole('button', { name: /create/i }));
};

afterEach(() => createGroup.mockReset());

describe('groups create page', () => {
  it('creates the group, toasts and navigates to it', async () => {
    createGroup.mockResolvedValue({ data: { uuid: 'g1' } });
    await submit('Admins');
    await vi.waitFor(() =>
      expect(createGroup).toHaveBeenCalledWith({ body: { name: 'Admins' } })
    );
    expect(toastSuccess).toHaveBeenCalledWith('Group created successfully.');
    await vi.waitFor(() => expect(goto).toHaveBeenCalledWith('/groups/g1'));
  });

  it('shows a field error on a 409 conflict', async () => {
    createGroup.mockResolvedValue({ response: { status: 409 } });
    await submit('Admins');
    expect(
      await screen.findByText('A group with this name already exists.')
    ).toBeInTheDocument();
  });

  it('shows a generic error otherwise', async () => {
    createGroup.mockResolvedValue({ response: { status: 500 } });
    await submit('Admins');
    await vi.waitFor(() =>
      expect(toastError).toHaveBeenCalledWith('Error creating group.')
    );
  });

  it('does not submit an empty name', async () => {
    await submit('');
    await new Promise((r) => setTimeout(r, 50));
    expect(createGroup).not.toHaveBeenCalled();
  });
});
