import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { goto } from '$app/navigation';

const createCache = vi.fn();
vi.mock('$lib/client', async (orig) => ({
  ...(await orig<object>()),
  createCache: (...a: unknown[]) => createCache(...a)
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

const Page = (await import('$routes/caches/create/+page.svelte')).default;

const fill = async (fields: Record<string, string>) => {
  render(Page);
  await Promise.all(
    Object.entries(fields).map(async ([label, value]) =>
      fireEvent.input(screen.getByLabelText(label), { target: { value } })
    )
  );
  await fireEvent.click(screen.getByRole('button', { name: /create/i }));
};

const valid = {
  'Cache Name': 'my-cache',
  'Quota (MiB)': '2048',
  'Signing Key': 'cache.example.com-1:abc'
};

afterEach(() => {
  createCache.mockReset();
  toastError.mockReset();
});

describe('caches create page', () => {
  it('renders every field with defaults', () => {
    render(Page);
    expect(screen.getByLabelText('Quota (MiB)')).toHaveValue(10 * 1024);
    expect(screen.getByLabelText('Public Cache')).not.toBeChecked();
  });

  it('submits the typed values and navigates to the new cache', async () => {
    createCache.mockResolvedValue({ data: { uuid: 'c1' } });
    await fill(valid);
    await vi.waitFor(() =>
      expect(createCache).toHaveBeenCalledWith({
        body: {
          name: 'my-cache',
          public: false,
          quota: 2048,
          sig_key: 'cache.example.com-1:abc'
        }
      })
    );
    expect(toastSuccess).toHaveBeenCalledWith('Cache created successfully.');
    await vi.waitFor(() => expect(goto).toHaveBeenCalledWith('/caches/c1'));
  });

  it.each([
    [409, 'A cache with this name already exists.'],
    [406, 'Invalid signature key format.']
  ])('maps %d to a field error', async (status, message) => {
    createCache.mockResolvedValue({ response: { status } });
    await fill(valid);
    expect(await screen.findByText(message)).toBeInTheDocument();
  });

  it('toasts a generic error for other failures', async () => {
    createCache.mockResolvedValue({ response: { status: 500 } });
    await fill(valid);
    await vi.waitFor(() =>
      expect(toastError).toHaveBeenCalledWith('Error creating cache.')
    );
  });

  it('blocks an invalid cache name client side', async () => {
    await fill({ ...valid, 'Cache Name': '-bad-' });
    expect(
      await screen.findByText(
        'Cache name may only contain letters, numbers and hyphens, and cannot start or end with a hyphen'
      )
    ).toBeInTheDocument();
    expect(createCache).not.toHaveBeenCalled();
  });

  it('blocks a zero quota client side', async () => {
    await fill({ ...valid, 'Quota (MiB)': '0' });
    expect(
      await screen.findByText('Quota must be a positive number')
    ).toBeInTheDocument();
    expect(createCache).not.toHaveBeenCalled();
  });
});
