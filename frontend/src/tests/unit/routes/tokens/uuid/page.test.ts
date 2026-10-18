import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { goto } from '$app/navigation';

const api = {
  deleteToken: vi.fn(),
  editToken: vi.fn(),
  tokenRegenerate: vi.fn()
};
vi.mock('$lib/client', async (orig) => ({
  ...(await orig<object>()),
  deleteToken: (...a: unknown[]) => api.deleteToken(...a),
  editToken: (...a: unknown[]) => api.editToken(...a),
  tokenRegenerate: (...a: unknown[]) => api.tokenRegenerate(...a)
}));
const toastSuccess = vi.fn();
vi.mock('@profidev/pleiades/components/util/general', async (orig) => ({
  ...(await orig<object>()),
  toast: { error: vi.fn(), success: (m: string) => toastSuccess(m) }
}));
const sonnerError = vi.fn();
vi.mock('svelte-sonner', async (orig) => ({
  ...(await orig<object>()),
  toast: { error: (m: string) => sonnerError(m) }
}));

const Page = (await import('$routes/tokens/[uuid]/+page.svelte')).default;

const exp = new Date(Date.now() + 5 * 86_400_000).toISOString();
const data = (res: object = { data: { exp, name: 'ci', uuid: 't1' } }) =>
  ({ tokenRes: Promise.resolve(res), uuid: 't1' }) as never;
const loaded = async () =>
  vi.waitFor(() =>
    expect(screen.getByLabelText('Token Name')).toHaveValue('ci')
  );
const confirm = async (name: string) =>
  fireEvent.click(screen.getAllByRole('button', { name }).at(-1)!);

afterEach(() => {
  for (const f of Object.values(api)) {
    f.mockReset();
  }
  toastSuccess.mockReset();
  sonnerError.mockReset();
  sessionStorage.clear();
});

describe('token detail page', () => {
  it.each([
    [404, '/tokens?error=not_found'],
    [500, '/tokens?error=other']
  ])('redirects a %d to the list', async (status, target) => {
    render(Page, { data: data({ response: { status } }) });
    await vi.waitFor(() => expect(goto).toHaveBeenCalledWith(target));
  });

  it('hides the secret unless it was just created', async () => {
    render(Page, { data: data() });
    await loaded();
    expect(screen.getByDisplayValue('Can not be viewed.')).toBeInTheDocument();
    expect(screen.queryByText('(Can not be viewed again!)')).toBeNull();
  });

  it('shows a freshly created secret once and forgets it', async () => {
    sessionStorage.setItem('newToken', 'sekret');
    render(Page, { data: data() });
    expect(await screen.findByText('sekret')).toBeInTheDocument();
    expect(screen.getByText('(Can not be viewed again!)')).toBeInTheDocument();
    expect(sessionStorage.getItem('newToken')).toBeNull();
  });

  it('saves name and expiry', async () => {
    api.editToken.mockResolvedValue({ data: {} });
    render(Page, { data: data() });
    await loaded();
    await fireEvent.input(screen.getByLabelText('Token Name'), {
      target: { value: 'ci2' }
    });
    await fireEvent.click(
      screen.getByRole('button', { name: /save changes/i })
    );
    await vi.waitFor(() =>
      expect(api.editToken).toHaveBeenCalledWith({
        body: { exp: new Date(exp), name: 'ci2', uuid: 't1' }
      })
    );
    expect(toastSuccess).toHaveBeenCalledWith('Token ci updated successfully');
  });

  it('maps a 409 to a name error', async () => {
    api.editToken.mockResolvedValue({ error: 'x', response: { status: 409 } });
    render(Page, { data: data() });
    await loaded();
    await fireEvent.click(
      screen.getByRole('button', { name: /save changes/i })
    );
    expect(
      await screen.findByText('This token name is already in use')
    ).toBeInTheDocument();
  });

  it('toasts other save errors', async () => {
    api.editToken.mockResolvedValue({ error: 'x', response: { status: 500 } });
    render(Page, { data: data() });
    await loaded();
    await fireEvent.click(
      screen.getByRole('button', { name: /save changes/i })
    );
    await vi.waitFor(() =>
      expect(sonnerError).toHaveBeenCalledWith('Failed to update token')
    );
  });

  it('regenerates and reveals the new secret', async () => {
    api.tokenRegenerate.mockResolvedValue({ data: { token: 'fresh' } });
    render(Page, { data: data() });
    await loaded();
    await fireEvent.click(screen.getByRole('button', { name: /regenerate/i }));
    await confirm('Regenerate');
    expect(await screen.findByText('fresh')).toBeInTheDocument();
    expect(api.tokenRegenerate).toHaveBeenCalledWith({ path: { uuid: 't1' } });
    expect(toastSuccess).toHaveBeenCalledWith(
      'Token ci regenerated successfully'
    );
  });

  it('toasts a failed regenerate', async () => {
    api.tokenRegenerate.mockResolvedValue({ error: 'x' });
    render(Page, { data: data() });
    await loaded();
    await fireEvent.click(screen.getByRole('button', { name: /regenerate/i }));
    await confirm('Regenerate');
    await vi.waitFor(() =>
      expect(sonnerError).toHaveBeenCalledWith('Failed to regenerate token')
    );
  });

  it('deletes and returns to the list', async () => {
    api.deleteToken.mockResolvedValue({ data: {} });
    render(Page, { data: data() });
    await loaded();
    await fireEvent.click(
      screen.getAllByRole('button', { name: /delete/i })[0]
    );
    await confirm('Delete');
    await vi.waitFor(() => expect(goto).toHaveBeenCalledWith('/tokens'));
    expect(api.deleteToken).toHaveBeenCalledWith({ body: { uuid: 't1' } });
  });

  it('toasts a failed delete', async () => {
    api.deleteToken.mockResolvedValue({ error: 'x' });
    render(Page, { data: data() });
    await loaded();
    await fireEvent.click(
      screen.getAllByRole('button', { name: /delete/i })[0]
    );
    await confirm('Delete');
    await vi.waitFor(() =>
      expect(sonnerError).toHaveBeenCalledWith('Failed to delete token')
    );
  });
});
