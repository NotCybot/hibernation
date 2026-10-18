import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { goto } from '$app/navigation';

const createUser = vi.fn();
vi.mock('$lib/client', async (orig) => ({
  ...(await orig<object>()),
  createUser: (...a: unknown[]) => createUser(...a)
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
const encrypt = { encrypt: vi.fn((s: string) => `enc(${s})`) };
const getEncrypt = vi.fn((): typeof encrypt | undefined => encrypt);
vi.mock('$lib/backend/auth.svelte', () => ({
  getEncrypt: () => getEncrypt()
}));

const Page = (await import('$routes/users/create/+page.svelte')).default;

const fill = async (mailActive: boolean, fields: Record<string, string>) => {
  render(Page, { data: { mailActive: Promise.resolve(mailActive) } as never });
  await new Promise((r) => setTimeout(r, 0));
  await Promise.all(
    Object.entries(fields).map(async ([label, value]) =>
      fireEvent.input(screen.getByLabelText(label), { target: { value } })
    )
  );
  await fireEvent.click(screen.getByRole('button', { name: /create/i }));
};

const base = { Email: 'bob@example.com', 'User Name': 'Bob' };

afterEach(() => {
  createUser.mockReset();
  toastError.mockReset();
  getEncrypt.mockImplementation(() => encrypt);
});

describe('users create page', () => {
  it('encrypts the password and navigates to the new user', async () => {
    createUser.mockResolvedValue({ data: { uuid: 'u1' } });
    await fill(false, { ...base, Password: 'secret1' });
    await vi.waitFor(() =>
      expect(createUser).toHaveBeenCalledWith({
        body: {
          email: 'bob@example.com',
          name: 'Bob',
          password: 'enc(secret1)'
        }
      })
    );
    expect(toastSuccess).toHaveBeenCalledWith('User created successfully.');
    await vi.waitFor(() => expect(goto).toHaveBeenCalledWith('/users/u1'));
  });

  it('rejects a short password when mail is inactive', async () => {
    await fill(false, { ...base, Password: 'short' });
    expect(
      await screen.findByText('Password must be at least 6 characters long.')
    ).toBeInTheDocument();
    expect(createUser).not.toHaveBeenCalled();
  });

  it('hides the password and allows none when mail is active', async () => {
    createUser.mockResolvedValue({ data: { uuid: 'u1' } });
    await fill(true, base);
    expect(screen.queryByLabelText('Password')).toBeNull();
    await vi.waitFor(() =>
      expect(createUser).toHaveBeenCalledWith({
        body: { email: 'bob@example.com', name: 'Bob', password: 'enc()' }
      })
    );
  });

  it('refuses to submit without an encryption key', async () => {
    getEncrypt.mockImplementation(() => undefined);
    await fill(false, { ...base, Password: 'secret1' });
    expect(
      await screen.findByText('Encryption function not available.')
    ).toBeInTheDocument();
    expect(createUser).not.toHaveBeenCalled();
  });

  it('maps 409 to an email field error', async () => {
    createUser.mockResolvedValue({ response: { status: 409 } });
    await fill(false, { ...base, Password: 'secret1' });
    expect(
      await screen.findByText('A user with this email already exists.')
    ).toBeInTheDocument();
  });

  it('toasts a generic error otherwise', async () => {
    createUser.mockResolvedValue({ response: { status: 500 } });
    await fill(false, { ...base, Password: 'secret1' });
    await vi.waitFor(() =>
      expect(toastError).toHaveBeenCalledWith('Error creating user.')
    );
  });
});
