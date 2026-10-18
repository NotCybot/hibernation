import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { goto, invalidate } from '$app/navigation';

const completeSetup = vi.fn();
const initOidc = vi.fn();
vi.mock('$lib/client', async (orig) => ({
  ...(await orig<object>()),
  completeSetup: (...a: unknown[]) => completeSetup(...a),
  initOidc: (...a: unknown[]) => initOidc(...a)
}));
const toastError = vi.fn();
vi.mock('svelte-sonner', async (orig) => ({
  ...(await orig<object>()),
  toast: { error: (m: string) => toastError(m) }
}));
const encrypt = { encrypt: (s: string) => `enc(${s})` };
const getEncrypt = vi.fn((): typeof encrypt | undefined => encrypt);
const getOidcUrl = vi.fn(async (): Promise<string | undefined> => undefined);
vi.mock('$lib/backend/auth.svelte', () => ({
  getEncrypt: () => getEncrypt(),
  getOidcUrl: async () => getOidcUrl()
}));
const connectWebsocket = vi.fn();
vi.mock('$lib/backend/updater.svelte', () => ({
  connectWebsocket: (u: string) => connectWebsocket(u)
}));

const Page = (await import('$routes/setup/+page.svelte')).default;

const data = (configured = false, from_env: string[] = []) =>
  ({
    configured,
    db_backend: 'SQLite',
    settings: {
      from_env,
      settings: {},
      site_url: 'https://hib.example/'
    },
    storage_backend: 'Filesystem'
  }) as never;

const next = async () =>
  fireEvent.click(screen.getByRole('button', { name: /next/i }));
const finish = async () =>
  fireEvent.click(screen.getAllByRole('button', { name: /finish/i })[0]);
const type = async (label: string, value: string) =>
  fireEvent.input(screen.getByLabelText(label), { target: { value } });

const toUserStep = async () => {
  await fireEvent.click(screen.getByLabelText('I have read the disclaimer'));
  await next();
  await screen.findByText('User Setup');
};

const fillAdmin = async () => {
  await type('Username', 'admin');
  await type('Email', 'admin@example.com');
  await type('Password', 'secret1');
};

beforeEach(() => {
  getEncrypt.mockImplementation(() => encrypt);
});
afterEach(() => {
  completeSetup.mockReset();
  initOidc.mockReset();
  toastError.mockReset();
});

describe('setup wizard: database step', () => {
  it('shows the detected backends', () => {
    render(Page, { data: data() });
    expect(screen.getByText('SQLite')).toBeInTheDocument();
    expect(screen.getByText('Filesystem')).toBeInTheDocument();
  });

  it('refuses to continue until the disclaimer is accepted', async () => {
    render(Page, { data: data() });
    await next();
    expect(
      await screen.findByText('You must accept the disclaimer to proceed.')
    ).toBeInTheDocument();
    expect(screen.queryByText('User Setup')).toBeNull();
  });

  it('pre-accepts the disclaimer and opens the OIDC tab when already configured', async () => {
    render(Page, { data: data(true) });
    expect(screen.getByLabelText('I have read the disclaimer')).toBeChecked();
    await next();
    expect(
      await screen.findByRole('tab', { name: 'OpenID Connect' })
    ).toHaveAttribute('data-state', 'active');
  });
});

describe('setup wizard: navigation', () => {
  it('keeps the typed values when going back and forth', async () => {
    render(Page, { data: data() });
    await toUserStep();
    await type('Username', 'admin');
    await fireEvent.click(screen.getByRole('button', { name: /previous/i }));
    expect(
      await screen.findByLabelText('I have read the disclaimer')
    ).toBeChecked();
    await next();
    expect(await screen.findByLabelText('Username')).toHaveValue('admin');
  });
});

describe('setup wizard: admin user', () => {
  it('completes setup with the encrypted password and navigates home', async () => {
    completeSetup.mockResolvedValue({ data: { user: 'u1' } });
    render(Page, { data: data() });
    await toUserStep();
    await fillAdmin();
    await finish();
    await vi.waitFor(() =>
      expect(completeSetup).toHaveBeenCalledWith({
        body: {
          admin_email: 'admin@example.com',
          admin_password: 'enc(secret1)',
          admin_username: 'admin'
        }
      })
    );
    await vi.waitFor(() => expect(goto).toHaveBeenCalledWith('/'));
    expect(connectWebsocket).toHaveBeenCalledWith('u1');
    expect(invalidate).toHaveBeenCalledWith('/api/user/info');
  });

  it.each([
    [409, 'The setup was already completed.'],
    [500, 'The server failed to find the admin group.'],
    [418, 'An unknown error occurred.']
  ])('maps a %d to a toast', async (status, message) => {
    completeSetup.mockResolvedValue({ response: { status } });
    render(Page, { data: data() });
    await toUserStep();
    await fillAdmin();
    await finish();
    await vi.waitFor(() => expect(toastError).toHaveBeenCalledWith(message));
  });

  it('bails out without an encryption key', async () => {
    getEncrypt.mockImplementation(() => undefined);
    render(Page, { data: data() });
    await toUserStep();
    await fillAdmin();
    await finish();
    await vi.waitFor(() =>
      expect(toastError).toHaveBeenCalledWith(
        'Encryption function not available. Please try again later.'
      )
    );
    expect(completeSetup).not.toHaveBeenCalled();
  });

  it('validates the admin fields client side', async () => {
    render(Page, { data: data() });
    await toUserStep();
    await type('Username', 'ab');
    await type('Email', 'admin@example.com');
    await type('Password', '123');
    await finish();
    expect(
      await screen.findByText('Username must be at least 3 characters long.')
    ).toBeInTheDocument();
    expect(
      await screen.findByText('Password must be at least 6 characters long.')
    ).toBeInTheDocument();
    expect(completeSetup).not.toHaveBeenCalled();
  });
});

describe('setup wizard: OIDC', () => {
  const toOidc = async (from_env: string[] = []) => {
    render(Page, { data: data(true, from_env) });
    await next();
    await screen.findByText('User Setup');
    await type('OpenID Connect Config URL', 'https://idp.example');
    await type('OpenID Connect Client ID', 'client');
    // The OIDC tab is the second form, so its Finish button is the last one.
    const buttons = screen.getAllByRole('button', { name: /finish/i });
    await fireEvent.click(buttons.at(-1)!);
  };

  it('shows the callback url derived from the site url', async () => {
    render(Page, { data: data(true) });
    await next();
    expect(
      await screen.findByText('https://hib.example/api/auth/oidc/callback')
    ).toBeInTheDocument();
  });

  it('disables fields that come from the environment', async () => {
    render(Page, { data: data(true, ['oidc_client_id']) });
    await next();
    expect(
      await screen.findByLabelText('OpenID Connect Client ID')
    ).toBeDisabled();
    expect(screen.getByLabelText('OpenID Connect Config URL')).toBeEnabled();
  });

  it('initialises OIDC and follows the authorize url', async () => {
    initOidc.mockResolvedValue({ data: {} });
    getOidcUrl.mockResolvedValue('https://idp.example/authorize');
    const location = { href: '' };
    vi.stubGlobal('location', location);
    await toOidc();
    await vi.waitFor(() => expect(initOidc).toHaveBeenCalledOnce());
    const [[{ body }]] = initOidc.mock.calls as [[{ body: object }]];
    expect(body).toMatchObject({
      oidc_client_id: 'client',
      oidc_issuer: 'https://idp.example'
    });
    await vi.waitFor(() =>
      expect(location.href).toBe('https://idp.example/authorize')
    );
    vi.unstubAllGlobals();
  });

  it('strips env-provided keys from the request', async () => {
    initOidc.mockResolvedValue({ data: {} });
    getOidcUrl.mockResolvedValue(undefined);
    render(Page, { data: data(true, ['oidc_scopes']) });
    await next();
    await screen.findByText('User Setup');
    await type('OpenID Connect Config URL', 'https://idp.example');
    await type('OpenID Connect Client ID', 'client');
    await fireEvent.click(
      screen.getAllByRole('button', { name: /finish/i }).at(-1)!
    );
    await vi.waitFor(() => expect(initOidc).toHaveBeenCalledOnce());
    const [[{ body }]] = initOidc.mock.calls as [[{ body: object }]];
    expect(body).not.toHaveProperty('oidc_scopes');
  });

  it.each([
    ['OpenID Connect Config URL', 'OpenID Connect Client ID'],
    ['OpenID Connect Client ID', 'OpenID Connect Config URL']
  ])('requires %s on the OIDC path', async (missing, filled) => {
    render(Page, { data: data(true) });
    await next();
    await screen.findByText('User Setup');
    await type(filled, filled.includes('URL') ? 'https://idp.example' : 'c');
    await fireEvent.click(
      screen.getAllByRole('button', { name: /finish/i }).at(-1)!
    );
    expect(
      await screen.findByText(
        'This field is required for OpenID Connect setup.'
      )
    ).toBeInTheDocument();
    expect(initOidc).not.toHaveBeenCalled();
    expect(screen.getByLabelText(missing)).toHaveValue('');
  });

  it('does not require fields provided by the environment', async () => {
    initOidc.mockResolvedValue({ data: {} });
    getOidcUrl.mockResolvedValue(undefined);
    render(Page, { data: data(true, ['oidc_issuer', 'oidc_client_id']) });
    await next();
    await screen.findByText('User Setup');
    await fireEvent.click(
      screen.getAllByRole('button', { name: /finish/i }).at(-1)!
    );
    await vi.waitFor(() => expect(initOidc).toHaveBeenCalledOnce());
  });

  it('maps a 406 to an issuer field error', async () => {
    initOidc.mockResolvedValue({ error: 'x', response: { status: 406 } });
    await toOidc();
    expect(
      await screen.findByText(
        'Invalid OIDC configuration URL. Check the server logs for more information.'
      )
    ).toBeInTheDocument();
  });

  it('toasts an unknown init error', async () => {
    initOidc.mockResolvedValue({ error: 'x', response: { status: 500 } });
    await toOidc();
    await vi.waitFor(() =>
      expect(toastError).toHaveBeenCalledWith(
        'An unknown error occurred. Check the server logs for more information.'
      )
    );
  });

  it('toasts when no authorize url comes back', async () => {
    initOidc.mockResolvedValue({ data: {} });
    getOidcUrl.mockResolvedValue(undefined);
    await toOidc();
    await vi.waitFor(() =>
      expect(toastError).toHaveBeenCalledWith(
        'Failed to retrieve OIDC URL. Check the server logs for more information.'
      )
    );
  });
});
