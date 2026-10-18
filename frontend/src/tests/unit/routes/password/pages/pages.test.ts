import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { goto } from '$app/navigation';

const api = { resetPassword: vi.fn(), sendResetLink: vi.fn() };
vi.mock('$lib/client', async (orig) => ({
  ...(await orig<object>()),
  resetPassword: (...a: unknown[]) => api.resetPassword(...a),
  sendResetLink: (...a: unknown[]) => api.sendResetLink(...a)
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
const encrypt = { encrypt: (s: string) => `enc(${s})` };
const getEncrypt = vi.fn((): typeof encrypt | undefined => encrypt);
vi.mock('$lib/backend/auth.svelte', () => ({ getEncrypt: () => getEncrypt() }));

const Forgot = (await import('$routes/password/forgot/+page.svelte')).default;
const Reset = (await import('$routes/password/reset/+page.svelte')).default;

const type = async (label: string, value: string) =>
  fireEvent.input(screen.getByLabelText(label), { target: { value } });

beforeEach(() => getEncrypt.mockImplementation(() => encrypt));
afterEach(() => {
  api.resetPassword.mockReset();
  api.sendResetLink.mockReset();
  toastSuccess.mockReset();
  sonnerError.mockReset();
});

describe('forgot password page', () => {
  const submit = async () => {
    render(Forgot);
    await type('Email', 'a@b.io');
    await fireEvent.click(
      screen.getByRole('button', { name: 'Send Reset Link' })
    );
  };

  it('sends the reset link', async () => {
    api.sendResetLink.mockResolvedValue({ data: {} });
    await submit();
    await vi.waitFor(() =>
      expect(toastSuccess).toHaveBeenCalledWith(
        'Reset link sent to your email address.'
      )
    );
    expect(api.sendResetLink).toHaveBeenCalledWith({
      body: { email: 'a@b.io' }
    });
  });

  it.each([
    [429, 'Rate limit exceeded. Please try again later.'],
    [500, 'Failed to send reset link.']
  ])('toasts a %d', async (status, message) => {
    api.sendResetLink.mockResolvedValue({ error: 'x', response: { status } });
    await submit();
    await vi.waitFor(() => expect(sonnerError).toHaveBeenCalledWith(message));
  });
});

describe('reset password page', () => {
  const submit = async (
    token: string | null = 'tok',
    pw = 'secret1',
    confirm = pw
  ) => {
    render(Reset, { data: { token } as never });
    await type('New Password', pw);
    await type('Confirm Password', confirm);
    await fireEvent.click(
      screen.getByRole('button', { name: 'Reset Password' })
    );
  };

  it('prefills the token from the url', () => {
    render(Reset, { data: { token: 'tok' } as never });
    expect(screen.getByLabelText('Token')).toHaveValue('tok');
  });

  it('starts with an empty token when none is given', () => {
    render(Reset, { data: { token: null } as never });
    expect(screen.getByLabelText('Token')).toHaveValue('');
  });

  it('resets with the encrypted password and goes to login', async () => {
    api.resetPassword.mockResolvedValue({ data: {} });
    await submit();
    await vi.waitFor(() =>
      expect(api.resetPassword).toHaveBeenCalledWith({
        body: { new_password: 'enc(secret1)', token: 'tok' }
      })
    );
    expect(toastSuccess).toHaveBeenCalledWith(
      'Password has been reset successfully. You can now log in.'
    );
    await vi.waitFor(() => expect(goto).toHaveBeenCalledWith('/login'));
  });

  it('blocks mismatched passwords', async () => {
    await submit('tok', 'secret1', 'secret2');
    expect(
      await screen.findByText('Passwords do not match')
    ).toBeInTheDocument();
    expect(api.resetPassword).not.toHaveBeenCalled();
  });

  it.each([
    [429, 'Rate limit exceeded. Please try again later.'],
    [500, 'Failed to reset password.']
  ])('toasts a %d', async (status, message) => {
    api.resetPassword.mockResolvedValue({ error: 'x', response: { status } });
    await submit();
    await vi.waitFor(() => expect(sonnerError).toHaveBeenCalledWith(message));
  });

  it('refuses without encryption', async () => {
    getEncrypt.mockImplementation(() => undefined);
    await submit();
    await vi.waitFor(() =>
      expect(sonnerError).toHaveBeenCalledWith(
        'Encryption function not available. Please try again later.'
      )
    );
    expect(api.resetPassword).not.toHaveBeenCalled();
  });
});
