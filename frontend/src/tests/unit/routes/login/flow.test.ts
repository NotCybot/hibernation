import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { goto, invalidate } from '$app/navigation';

vi.mock('$app/navigation', async () =>
  (await import('$test_helpers/load')).navigationMock()
);
const authenticate = vi.fn();
vi.mock('$lib/client', async (orig) => ({
  ...(await orig<object>()),
  authenticate: (...a: unknown[]) => authenticate(...a)
}));
const toastError = vi.fn();
vi.mock('@profidev/pleiades/components/util/general', async (orig) => ({
  ...(await orig<object>()),
  toast: { error: (m: string) => toastError(m), success: vi.fn() }
}));
const sonnerError = vi.fn();
vi.mock('svelte-sonner', async (orig) => ({
  ...(await orig<object>()),
  toast: { error: (m: string) => sonnerError(m) }
}));
const encrypt = { encrypt: (s: string) => `enc(${s})` };
const getEncrypt = vi.fn((): typeof encrypt | undefined => encrypt);
const getOidcUrl = vi.fn(
  async (_r?: string): Promise<string | undefined> => undefined
);
vi.mock('$lib/backend/auth.svelte', () => ({
  getEncrypt: () => getEncrypt(),
  getOidcUrl: async (r?: string) => getOidcUrl(r)
}));
const connectWebsocket = vi.fn();
vi.mock('$lib/backend/updater.svelte', () => ({
  connectWebsocket: (u: string) => connectWebsocket(u)
}));

const Page = (await import('$routes/login/+page.svelte')).default;

const config = async (over: object = {}) =>
  Promise.resolve({
    instant_redirect: false,
    mail_enabled: false,
    sso_type: 'Oidc',
    ...over
  });
const data = (over: object = {}) =>
  ({
    config: config(),
    redirectTo: '/caches',
    skip: false,
    ...over
  }) as never;
const location = { href: '' };

const login = async (email = 'a@b.io', password = 'pw') => {
  await fireEvent.input(screen.getByLabelText('Email'), {
    target: { value: email }
  });
  await fireEvent.input(screen.getByLabelText('Password'), {
    target: { value: password }
  });
  await fireEvent.click(screen.getByRole('button', { name: 'Login' }));
};

beforeEach(() => {
  location.href = '';
  vi.stubGlobal('location', location);
  getEncrypt.mockImplementation(() => encrypt);
});
afterEach(() => {
  vi.unstubAllGlobals();
  authenticate.mockReset();
  getOidcUrl.mockReset();
  toastError.mockReset();
  sonnerError.mockReset();
});

describe('login form', () => {
  it('logs in, connects the websocket and follows the redirect', async () => {
    authenticate.mockResolvedValue({ data: { user: 'u1' } });
    render(Page, { data: data() });
    await login();
    await vi.waitFor(() =>
      expect(authenticate).toHaveBeenCalledWith({
        body: { email: 'a@b.io', password: 'enc(pw)' },
        parseAs: 'json'
      })
    );
    await vi.waitFor(() => expect(goto).toHaveBeenCalledWith('/caches'));
    expect(connectWebsocket).toHaveBeenCalledWith('u1');
    expect(invalidate).toHaveBeenCalledWith('/api/user/info');
  });

  it('flags bad credentials on the password field', async () => {
    authenticate.mockResolvedValue({ response: { status: 401 } });
    render(Page, { data: data() });
    await login();
    expect(
      await screen.findByText('Invalid email or password.')
    ).toBeInTheDocument();
  });

  it.each([
    [429, 'Rate limit exceeded. Please try again later.'],
    [500, 'Login failed. Please try again.']
  ])('toasts a %d', async (status, message) => {
    authenticate.mockResolvedValue({ response: { status } });
    render(Page, { data: data() });
    await login();
    await vi.waitFor(() => expect(sonnerError).toHaveBeenCalledWith(message));
  });

  it('refuses without encryption', async () => {
    getEncrypt.mockImplementation(() => undefined);
    render(Page, { data: data() });
    await login();
    await vi.waitFor(() =>
      expect(sonnerError).toHaveBeenCalledWith(
        'Encryption function not available. Please try again later.'
      )
    );
    expect(authenticate).not.toHaveBeenCalled();
  });

  it('validates the email client side', async () => {
    render(Page, { data: data() });
    await login('nope', 'pw');
    await new Promise((r) => setTimeout(r, 20));
    expect(authenticate).not.toHaveBeenCalled();
  });

  it('links to the forgot page only with mail enabled', async () => {
    render(Page, {
      data: data({ config: config({ mail_enabled: true }) })
    });
    expect(await screen.findByText('Forgot your password?')).toHaveAttribute(
      'href',
      '/password/forgot'
    );
  });
});

describe('login SSO', () => {
  it('hides the SSO button when SSO is off', async () => {
    render(Page, {
      data: data({ config: config({ sso_type: 'None' }) })
    });
    await vi.waitFor(() =>
      expect(
        screen.queryByRole('button', { name: /oidc provider/i })
      ).toBeNull()
    );
  });

  // Regression: the provider url used to be requested even with SSO off.
  it('does not request a provider url when SSO is off', async () => {
    render(Page, {
      data: data({ config: config({ sso_type: 'None' }) })
    });
    await new Promise((r) => setTimeout(r, 10));
    expect(getOidcUrl).not.toHaveBeenCalled();
  });

  it('hides the SSO button while the auth config is unavailable', async () => {
    render(Page, { data: data({ config: Promise.resolve(undefined) }) });
    await new Promise((r) => setTimeout(r, 10));
    expect(screen.queryByRole('button', { name: /oidc provider/i })).toBeNull();
    expect(getOidcUrl).not.toHaveBeenCalled();
  });

  it('fetches the authorize url for the redirect target and follows it on click', async () => {
    getOidcUrl.mockResolvedValue('https://idp/auth');
    render(Page, { data: data() });
    await vi.waitFor(() => expect(getOidcUrl).toHaveBeenCalledWith('/caches'));
    expect(location.href).toBe('');
    await fireEvent.click(
      screen.getByRole('button', { name: /oidc provider/i })
    );
    expect(location.href).toBe('https://idp/auth');
  });

  it('redirects instantly when configured', async () => {
    getOidcUrl.mockResolvedValue('https://idp/auth');
    render(Page, {
      data: data({
        config: config({ instant_redirect: true })
      })
    });
    await vi.waitFor(() => expect(location.href).toBe('https://idp/auth'));
  });

  it('does not redirect instantly when skip is set, and strips skip', async () => {
    getOidcUrl.mockResolvedValue('https://idp/auth');
    location.href = 'http://localhost/login?skip=true&x=1';
    const replace = vi
      .spyOn(window.history, 'replaceState')
      .mockImplementation(() => {});
    render(Page, {
      data: data({
        config: config({ instant_redirect: true }),
        skip: true
      })
    });
    await vi.waitFor(() => expect(getOidcUrl).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 10));
    expect(location.href).toBe('http://localhost/login?skip=true&x=1');
    const url = replace.mock.calls.at(-1)?.[2] as URL;
    expect(url.searchParams.has('skip')).toBe(false);
    expect(url.searchParams.get('x')).toBe('1');
    replace.mockRestore();
  });

  it('shows a retry state when no url is available and recovers', async () => {
    getOidcUrl.mockResolvedValue(undefined);
    render(Page, { data: data() });
    await vi.waitFor(() => expect(getOidcUrl).toHaveBeenCalled());
    await fireEvent.click(
      screen.getByRole('button', { name: /oidc provider/i })
    );
    expect(toastError).toHaveBeenCalledWith('Failed to get OIDC URL.');
    const retry = await screen.findByRole('button', { name: /retry oidc/i });

    getOidcUrl.mockResolvedValue('https://idp/auth');
    await fireEvent.click(retry);
    await vi.waitFor(() => expect(location.href).toBe('https://idp/auth'));
    expect(
      await screen.findByRole('button', { name: /oidc provider/i })
    ).toBeInTheDocument();
  });

  it('stays in the retry state when the retry also fails', async () => {
    getOidcUrl.mockResolvedValue(undefined);
    render(Page, { data: data() });
    await vi.waitFor(() => expect(getOidcUrl).toHaveBeenCalled());
    await fireEvent.click(
      screen.getByRole('button', { name: /oidc provider/i })
    );
    await fireEvent.click(
      await screen.findByRole('button', { name: /retry oidc/i })
    );
    await vi.waitFor(() => expect(toastError).toHaveBeenCalledTimes(2));
    expect(location.href).toBe('');
  });
});
