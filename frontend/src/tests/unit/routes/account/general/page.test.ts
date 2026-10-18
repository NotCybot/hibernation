import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';

const api = { updateAccount: vi.fn(), updateAvatar: vi.fn() };
vi.mock('$lib/client', async (orig) => ({
  ...(await orig<object>()),
  updateAccount: (...a: unknown[]) => api.updateAccount(...a),
  updateAvatar: (...a: unknown[]) => api.updateAvatar(...a)
}));
const toast = { error: vi.fn(), success: vi.fn() };
vi.mock('@profidev/pleiades/components/util/general', async (orig) => ({
  ...(await orig<object>()),
  toast: {
    error: (m: string) => toast.error(m),
    success: (m: string) => toast.success(m)
  }
}));
vi.mock('@profidev/pleiades/components/ui-extra/image-cropper', async () => {
  const Root = (await import('./FakeCropper.svelte')).default;
  const Pass = (await import('./Passthrough.svelte')).default;
  return {
    Cancel: Pass,
    Controls: Pass,
    Crop: Pass,
    Cropper: Pass,
    Dialog: Pass,
    Preview: Pass,
    Root,
    UploadTrigger: Pass,
    getFileFromUrl: async () => ({
      arrayBuffer: async () => new Uint8Array([104, 105]).buffer
    })
  };
});

const Page = (await import('$routes/account/general/+page.svelte')).default;

const data = () =>
  ({ user: Promise.resolve({ name: 'Bob', uuid: 'u1' }) }) as never;
const loaded = async () =>
  vi.waitFor(() =>
    expect(screen.getByLabelText('Username')).toHaveValue('Bob')
  );

afterEach(() => {
  api.updateAccount.mockReset();
  api.updateAvatar.mockReset();
  toast.error.mockReset();
  toast.success.mockReset();
});

describe('account general page', () => {
  it('loads the user into the form and avatar', async () => {
    render(Page, { data: data() });
    await loaded();
    expect(screen.getByLabelText('Username')).toBeEnabled();
    expect(screen.getByAltText('avatar')).toHaveAttribute(
      'src',
      '/api/user/info/avatar/u1'
    );
  });

  it('disables the username until the user is known', () => {
    render(Page, { data: { user: new Promise(() => {}) } as never });
    expect(screen.getByLabelText('Username')).toBeDisabled();
  });

  it.each([
    [{ data: {} }, 'success', 'General settings saved successfully'],
    [{ error: 'x' }, 'error', 'Failed to save general settings']
  ] as const)('saves the username (%#)', async (result, kind, message) => {
    api.updateAccount.mockResolvedValue(result);
    render(Page, { data: data() });
    await loaded();
    await fireEvent.input(screen.getByLabelText('Username'), {
      target: { value: 'Robert' }
    });
    await fireEvent.click(
      screen.getByRole('button', { name: /save changes/i })
    );
    await vi.waitFor(() => expect(toast[kind]).toHaveBeenCalledWith(message));
    expect(api.updateAccount).toHaveBeenCalledWith({
      body: { username: 'Robert' }
    });
  });

  it.each([
    [{ data: {} }, 'success', 'Avatar updated successfully'],
    [
      { error: 'x', response: { status: 429 } },
      'error',
      'Rate limit exceeded. Please try again later.'
    ],
    [
      { error: 'x', response: { status: 413 } },
      'error',
      'File too large. Please upload a smaller file.'
    ],
    [
      { error: 'x', response: { status: 500 } },
      'error',
      'Failed to update avatar'
    ]
  ] as const)(
    'uploads a cropped avatar (%#)',
    async (result, kind, message) => {
      api.updateAvatar.mockResolvedValue(result);
      render(Page, { data: data() });
      await loaded();
      await fireEvent.click(screen.getByRole('button', { name: 'crop' }));
      await vi.waitFor(() => expect(toast[kind]).toHaveBeenCalledWith(message));
      expect(api.updateAvatar).toHaveBeenCalledWith({
        body: { avatar: 'aGk=' }
      });
    }
  );

  it('rejects unsupported files', async () => {
    render(Page, { data: data() });
    await fireEvent.click(screen.getByRole('button', { name: 'unsupported' }));
    expect(toast.error).toHaveBeenCalledWith(
      'Unsupported file type: image/gif'
    );
  });
});
