import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';

const updatePassword = vi.fn();
const startEmailChange = vi.fn();
const confirmEmailChange = vi.fn();
vi.mock('$lib/client', async (orig) => ({
  ...(await orig<object>()),
  confirmEmailChange: (...a: unknown[]) => confirmEmailChange(...a),
  startEmailChange: (...a: unknown[]) => startEmailChange(...a),
  updatePassword: (...a: unknown[]) => updatePassword(...a)
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
const encrypt = { encrypt: (s: string) => `enc(${s})` };
const getEncrypt = vi.fn((): typeof encrypt | undefined => encrypt);
vi.mock('$lib/backend/auth.svelte', () => ({
  getEncrypt: () => getEncrypt()
}));

const Password = (await import('$routes/account/auth/Password.svelte')).default;
const Email = (await import('$routes/account/auth/Email.svelte')).default;

const trigger = (name: string) => screen.getByRole('button', { name });

const type = async (label: string, value: string) =>
  fireEvent.input(await screen.findByLabelText(label), { target: { value } });

/** OTP labels point at the wrapper div; type into the input inside it. */
// Regression: the OTP inputs had no accessible name; find them by it.
const typeOtp = async (label: string, value: string) => {
  const input = screen.getByRole('textbox', { name: label });
  await fireEvent.input(input, { target: { value } });
};

beforeEach(() => getEncrypt.mockImplementation(() => encrypt));
afterEach(() => {
  updatePassword.mockReset();
  startEmailChange.mockReset();
  confirmEmailChange.mockReset();
  toastError.mockReset();
  toastSuccess.mockReset();
});

describe('tooltip triggers', () => {
  it.each([
    [Password, { oidc: true }, 'Change Password'],
    [Email, { email: 'a@b.c', mailActive: false, oidc: false }, 'Change Email']
  ])('render a single, non-nested button (%#)', (Comp, props, name) => {
    // oxlint-disable-next-line typescript/no-explicit-any
    render(Comp as any, props);
    const buttons = screen.getAllByRole('button', { name });
    expect(buttons).toHaveLength(1);
    expect(buttons[0]?.parentElement?.closest('button')).toBeNull();
  });
});

describe('account Password', () => {
  const change = async (old: string, pw: string, confirm: string) => {
    render(Password, { oidc: false });
    await fireEvent.click(trigger('Change Password'));
    await type('Old Password', old);
    await type('New Password', pw);
    await type('Confirm New Password', confirm);
    await fireEvent.click(
      screen.getAllByRole('button', { name: 'Change Password' }).at(-1)!
    );
  };

  it('is disabled for OIDC users', () => {
    render(Password, { oidc: true });
    expect(trigger('Change Password')).toBeDisabled();
  });

  it('encrypts both passwords and toasts on success', async () => {
    updatePassword.mockResolvedValue({ response: { status: 200 } });
    await change('old', 'new', 'new');
    await vi.waitFor(() =>
      expect(updatePassword).toHaveBeenCalledWith({
        body: { new_password: 'enc(new)', old_password: 'enc(old)' }
      })
    );
    await vi.waitFor(() =>
      expect(toastSuccess).toHaveBeenCalledWith('Update successful')
    );
  });

  it('rejects mismatched confirmation without calling the API', async () => {
    await change('old', 'new', 'other');
    expect(
      await screen.findByText('Passwords are not equal')
    ).toBeInTheDocument();
    expect(updatePassword).not.toHaveBeenCalled();
  });

  it('flags a wrong old password (backend 403) on the old password field', async () => {
    updatePassword.mockResolvedValue({ response: { status: 403 } });
    await change('bad', 'new', 'new');
    expect(
      await screen.findByText('Old password is incorrect')
    ).toBeInTheDocument();
  });

  it('toasts other API errors', async () => {
    updatePassword.mockResolvedValue({ response: { status: 500 } });
    await change('old', 'new', 'new');
    await vi.waitFor(() =>
      expect(toastError).toHaveBeenCalledWith('Error while updating password')
    );
  });

  it('toasts when encryption is unavailable', async () => {
    getEncrypt.mockImplementation(() => undefined);
    await change('old', 'new', 'new');
    await vi.waitFor(() =>
      expect(toastError).toHaveBeenCalledWith(
        'Encryption function not available.'
      )
    );
    expect(updatePassword).not.toHaveBeenCalled();
  });
});

describe('account Email', () => {
  const open = async () => {
    render(Email, { email: 'old@example.com', mailActive: true, oidc: false });
    await fireEvent.click(trigger('Change Email'));
  };
  const confirmBtn = () =>
    screen.getAllByRole('button', { name: 'Change Email' }).at(-1)!;
  const start = async () => {
    startEmailChange.mockResolvedValue({ response: { status: 200 } });
    await open();
    await type('New Email', 'new@example.com');
    await fireEvent.click(confirmBtn());
    await screen.findByText('Code from new Email (new@example.com)');
  };

  it.each([
    [false, false],
    [true, true]
  ])('disables the trigger when mail is inactive (oidc=%s)', (oidc) => {
    render(Email, { email: 'a@b.c', mailActive: false, oidc });
    expect(trigger('Change Email')).toBeDisabled();
  });

  it('disables the trigger for OIDC users even with mail active', () => {
    render(Email, { email: 'a@b.c', mailActive: true, oidc: true });
    expect(trigger('Change Email')).toBeDisabled();
  });

  it('starts the change and switches to code entry', async () => {
    await start();
    expect(startEmailChange).toHaveBeenCalledWith({
      body: { new_email: 'new@example.com' }
    });
    expect(
      screen.getByText('Code from old Email (old@example.com)')
    ).toBeInTheDocument();
    expect(toastError).not.toHaveBeenCalled();
  });

  it('maps 409 on start to an email field error', async () => {
    startEmailChange.mockResolvedValue({ response: { status: 409 } });
    await open();
    await type('New Email', 'new@example.com');
    await fireEvent.click(confirmBtn());
    expect(
      await screen.findByText('This email is already in use.')
    ).toBeInTheDocument();
  });

  it('toasts other start errors', async () => {
    startEmailChange.mockResolvedValue({ response: { status: 500 } });
    await open();
    await type('New Email', 'new@example.com');
    await fireEvent.click(confirmBtn());
    await vi.waitFor(() =>
      expect(toastError).toHaveBeenCalledWith(
        'There was an error while sending your emails.'
      )
    );
  });

  it('validates both codes are 6 characters before confirming', async () => {
    await start();
    await fireEvent.click(confirmBtn());
    expect(
      await screen.findAllByText('Code must be 6 characters long')
    ).toHaveLength(2);
    expect(confirmEmailChange).not.toHaveBeenCalled();
  });

  const confirmWith = async (status: number) => {
    await start();
    confirmEmailChange.mockResolvedValue({ response: { status } });
    await typeOtp('Code from old Email (old@example.com)', '111111');
    await typeOtp('Code from new Email (new@example.com)', '222222');
    await fireEvent.click(confirmBtn());
    await vi.waitFor(() =>
      expect(confirmEmailChange).toHaveBeenCalledWith({
        body: { new_code: '222222', old_code: '111111' }
      })
    );
  };

  it('confirms the codes and toasts success', async () => {
    await confirmWith(200);
    await vi.waitFor(() =>
      expect(toastSuccess).toHaveBeenCalledWith('Email change successful')
    );
  });

  it.each([401, 403])(
    'shows the incorrect code error for %d',
    async (status) => {
      await confirmWith(status);
      expect(
        await screen.findByText('The code you entered is incorrect.')
      ).toBeInTheDocument();
    }
  );

  it('toasts other confirm errors', async () => {
    await confirmWith(500);
    await vi.waitFor(() =>
      expect(toastError).toHaveBeenCalledWith(
        'There was an error while confirming your email change.'
      )
    );
  });
});

describe('account auth page', () => {
  it('wires the user and mail state into both dialogs', async () => {
    const Page = (await import('$routes/account/auth/+page.svelte')).default;
    render(Page, {
      data: {
        mailActive: Promise.resolve(true),
        user: Promise.resolve({ email: 'me@x.io', oidc_user: false })
      } as never
    });
    await vi.waitFor(() => expect(trigger('Change Email')).toBeEnabled());
    expect(trigger('Change Password')).toBeEnabled();
  });

  it('disables both for an OIDC user', async () => {
    const Page = (await import('$routes/account/auth/+page.svelte')).default;
    render(Page, {
      data: {
        mailActive: Promise.resolve(true),
        user: Promise.resolve({ email: 'me@x.io', oidc_user: true })
      } as never
    });
    await vi.waitFor(() => expect(trigger('Change Password')).toBeDisabled());
    expect(trigger('Change Email')).toBeDisabled();
  });
});

describe('disabled-action tooltips', () => {
  const hover = async (name: string) => {
    const span = trigger(name).parentElement!;
    await fireEvent.pointerMove(span, { pointerType: 'mouse' });
    await fireEvent.focus(span);
  };

  it('explains why OIDC users cannot change their password', async () => {
    render(Password, { oidc: true });
    await hover('Change Password');
    expect(
      (
        await screen.findAllByText(/Oidc users can not change their password/)
      )[0]
    ).toBeInTheDocument();
  });

  it.each([
    [true, /Oidc users can not change their email/],
    [false, /Email can not be changed when mail support is not configured/]
  ])('explains the disabled email change (oidc=%s)', async (oidc, text) => {
    render(Email, { email: 'a@b.c', mailActive: false, oidc });
    await hover('Change Email');
    expect((await screen.findAllByText(text))[0]).toBeInTheDocument();
  });
});
