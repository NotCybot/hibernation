import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { goto } from '$app/navigation';

const createToken = vi.fn();
vi.mock('$lib/client', async (orig) => ({
  ...(await orig<object>()),
  createToken: (...a: unknown[]) => createToken(...a)
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

const Page = (await import('$routes/tokens/create/+page.svelte')).default;

const submit = async (name: string) => {
  render(Page);
  await fireEvent.input(screen.getByLabelText('Token Name'), {
    target: { value: name }
  });
  await fireEvent.click(screen.getByRole('button', { name: /create/i }));
};

afterEach(() => {
  createToken.mockReset();
  toastError.mockReset();
  sessionStorage.clear();
});

describe('tokens create page', () => {
  it('sends the expiry as ISO, stashes the secret and navigates', async () => {
    createToken.mockResolvedValue({ data: { token: 'secret', uuid: 't1' } });
    await submit('ci');
    await vi.waitFor(() => expect(createToken).toHaveBeenCalledOnce());
    const [[{ body }]] = createToken.mock.calls as [
      [{ body: { exp: string; name: string } }]
    ];
    expect(body.name).toBe('ci');
    expect(new Date(body.exp).toISOString()).toBe(body.exp);
    expect(toastSuccess).toHaveBeenCalledWith('Token created successfully.');
    expect(sessionStorage.getItem('newToken')).toBe('secret');
    await vi.waitFor(() => expect(goto).toHaveBeenCalledWith('/tokens/t1'));
  });

  it('shows a field error on 409', async () => {
    createToken.mockResolvedValue({ response: { status: 409 } });
    await submit('ci');
    expect(
      await screen.findByText('A token with this name already exists.')
    ).toBeInTheDocument();
    expect(sessionStorage.getItem('newToken')).toBeNull();
  });

  it('toasts a generic error otherwise', async () => {
    createToken.mockResolvedValue({ response: { status: 500 } });
    await submit('ci');
    await vi.waitFor(() =>
      expect(toastError).toHaveBeenCalledWith('Error creating token.')
    );
  });
});
