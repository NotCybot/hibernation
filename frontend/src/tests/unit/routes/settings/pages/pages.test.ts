import { afterEach, describe, expect, it, vi } from 'vitest';
import { createRawSnippet } from 'svelte';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { Permission } from '$lib/permissions.svelte';

const api = {
  saveMailSettings: vi.fn(),
  saveUserSettings: vi.fn(),
  testMail: vi.fn()
};
vi.mock('$lib/client', async (orig) => ({
  ...(await orig<object>()),
  saveMailSettings: (...a: unknown[]) => api.saveMailSettings(...a),
  saveUserSettings: (...a: unknown[]) => api.saveUserSettings(...a),
  testMail: (...a: unknown[]) => api.testMail(...a)
}));
const toast = { error: vi.fn(), success: vi.fn() };
vi.mock('@profidev/pleiades/components/util/general', async (orig) => ({
  ...(await orig<object>()),
  toast: {
    error: (m: string) => toast.error(m),
    success: (m: string) => toast.success(m)
  }
}));

const UserPage = (await import('$routes/settings/user/+page.svelte')).default;
const MailPage = (await import('$routes/settings/mail/+page.svelte')).default;
const SettingsLayout = (await import('$routes/settings/+layout.svelte'))
  .default;
const AccountLayout = (await import('$routes/account/+layout.svelte')).default;

const admin = Promise.resolve({ permissions: [Permission.SETTINGS_EDIT] });
const viewer = Promise.resolve({ permissions: [Permission.SETTINGS_VIEW] });
const save = () => screen.getByRole('button', { name: /save changes/i });

afterEach(() => {
  for (const f of Object.values(api)) {
    f.mockReset();
  }
  toast.error.mockReset();
  toast.success.mockReset();
});

describe('settings and account layouts', () => {
  const children = createRawSnippet(() => ({ render: () => '<p>child</p>' }));

  it.each([
    [
      SettingsLayout,
      'Settings',
      [
        ['User', '/settings/user'],
        ['Mail', '/settings/mail']
      ]
    ],
    [
      AccountLayout,
      'Account',
      [
        ['General', '/account/general'],
        ['Authentication', '/account/auth']
      ]
    ]
  ] as const)(
    'renders the %#th layout with its links',
    (Layout, title, links) => {
      render(Layout, { children });
      expect(screen.getByRole('heading', { name: title })).toBeInTheDocument();
      expect(screen.getByText('child')).toBeInTheDocument();
      for (const [name, href] of links) {
        expect(screen.getByRole('link', { name })).toHaveAttribute(
          'href',
          href
        );
      }
    }
  );
});

describe('user settings page', () => {
  const data = (
    user = admin,
    settings: object | null = {
      from_env: [],
      settings: {
        oidc_client_id: 'cid',
        oidc_enabled: true,
        oidc_issuer: 'https://idp'
      }
    }
  ) =>
    ({
      generalSettingsPromise: Promise.resolve({ site_url: 'https://hib/' }),
      settingsPromise: Promise.resolve(settings),
      user
    }) as never;
  const loaded = async () =>
    vi.waitFor(() =>
      expect(screen.getByLabelText('OpenID Connect Client ID')).toHaveValue(
        'cid'
      )
    );

  it('fills the form and shows the callback url', async () => {
    render(UserPage, { data: data() });
    await loaded();
    expect(
      screen.getByLabelText('Enable SSO via OpenID Connect')
    ).toBeChecked();
    expect(
      screen.getByText('https://hib/api/auth/oidc/callback')
    ).toBeInTheDocument();
    expect(screen.queryByText(/environment variables/)).toBeNull();
  });

  it('defaults to SSO user creation and instant redirect without settings', async () => {
    render(UserPage, { data: data(admin, null) });
    await vi.waitFor(() =>
      expect(
        screen.getByLabelText('Create missing users on SSO login')
      ).toBeChecked()
    );
  });

  it('saves the settings without env-provided keys', async () => {
    api.saveUserSettings.mockResolvedValue({ data: {} });
    render(UserPage, {
      data: data(admin, {
        from_env: ['oidc_scopes'],
        settings: {
          oidc_client_id: 'cid',
          oidc_enabled: true,
          oidc_issuer: 'https://idp'
        }
      })
    });
    await loaded();
    expect(
      screen.getByText(
        'Values loaded from environment variables cannot be edited here.'
      )
    ).toBeInTheDocument();
    expect(
      screen.getByLabelText('OpenID Connect Scopes (space separated)')
    ).toBeDisabled();
    await fireEvent.click(save());
    await vi.waitFor(() => expect(api.saveUserSettings).toHaveBeenCalledOnce());
    const [[{ body }]] = api.saveUserSettings.mock.calls as [
      [{ body: object }]
    ];
    expect(body).toMatchObject({
      oidc_client_id: 'cid',
      oidc_issuer: 'https://idp'
    });
    expect(body).not.toHaveProperty('oidc_scopes');
    expect(toast.success).toHaveBeenCalledWith(
      'User settings saved successfully'
    );
  });

  it('maps a 406 to an issuer error', async () => {
    api.saveUserSettings.mockResolvedValue({
      error: 'x',
      response: { status: 406 }
    });
    render(UserPage, { data: data() });
    await loaded();
    await fireEvent.click(save());
    expect(
      await screen.findByText(
        'Invalid OIDC configuration URL. Check the server logs for more information.'
      )
    ).toBeInTheDocument();
  });

  it('toasts other save errors', async () => {
    api.saveUserSettings.mockResolvedValue({
      error: 'x',
      response: { status: 500 }
    });
    render(UserPage, { data: data() });
    await loaded();
    await fireEvent.click(save());
    await vi.waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith('Failed to save user settings')
    );
  });

  it('requires issuer and client id when OIDC is enabled', async () => {
    render(UserPage, {
      data: data(admin, { from_env: [], settings: { oidc_enabled: true } })
    });
    await vi.waitFor(() =>
      expect(
        screen.getByLabelText('Enable SSO via OpenID Connect')
      ).toBeChecked()
    );
    await fireEvent.click(save());
    expect(
      await screen.findAllByText('This field is required when OIDC is enabled.')
    ).toHaveLength(2);
    expect(api.saveUserSettings).not.toHaveBeenCalled();
  });

  it('is fully readonly without settings:edit', async () => {
    render(UserPage, { data: data(viewer) });
    await loaded();
    expect(screen.getByLabelText('OpenID Connect Client ID')).toBeDisabled();
    expect(save()).toBeDisabled();
  });
});

describe('mail settings page', () => {
  const settings = {
    smtp_enabled: true,
    smtp_from_address: 'no-reply@x.io',
    smtp_from_name: 'Hib',
    smtp_port: 587,
    smtp_server: 'smtp.x.io',
    smtp_use_tls: true,
    smtp_username: 'u'
  };
  const data = (user = admin, s: object | null = { from_env: [], settings }) =>
    ({ settings: Promise.resolve(s), user }) as never;
  const loaded = async () =>
    vi.waitFor(() =>
      expect(screen.getByLabelText('SMTP Host')).toHaveValue('smtp.x.io')
    );

  it('fills the form and offers a test mail when enabled', async () => {
    render(MailPage, { data: data() });
    await loaded();
    expect(screen.getByLabelText('SMTP Port')).toHaveValue(587);
    expect(
      screen.getByRole('button', { name: /send test email/i })
    ).toBeEnabled();
  });

  it('hides the test mail button when SMTP is off or readonly', async () => {
    render(MailPage, { data: data(admin, null) });
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.getByLabelText('Enable SMTP')).not.toBeChecked();
    expect(
      screen.queryByRole('button', { name: /send test email/i })
    ).toBeNull();
  });

  it('is readonly without settings:edit', async () => {
    render(MailPage, { data: data(viewer) });
    await loaded();
    expect(screen.getByLabelText('SMTP Host')).toBeDisabled();
    expect(save()).toBeDisabled();
    expect(
      screen.queryByRole('button', { name: /send test email/i })
    ).toBeNull();
  });

  it('saves without env-provided keys', async () => {
    api.saveMailSettings.mockResolvedValue({ data: {} });
    render(MailPage, {
      data: data(admin, { from_env: ['smtp_password'], settings })
    });
    await loaded();
    expect(
      screen.getByText(
        'Values loaded from environment variables cannot be edited here.'
      )
    ).toBeInTheDocument();
    await fireEvent.click(save());
    await vi.waitFor(() => expect(api.saveMailSettings).toHaveBeenCalledOnce());
    const [[{ body }]] = api.saveMailSettings.mock.calls as [
      [{ body: object }]
    ];
    expect(body).toMatchObject({ smtp_server: 'smtp.x.io' });
    expect(body).not.toHaveProperty('smtp_password');
    expect(toast.success).toHaveBeenCalledWith(
      'Mail settings saved successfully'
    );
  });

  it.each([
    [406, 'Invalid From Address provided'],
    [400, 'Failed to create SMTP transport with provided settings']
  ])('maps a %d to a field error', async (status, message) => {
    api.saveMailSettings.mockResolvedValue({
      error: 'x',
      response: { status }
    });
    render(MailPage, { data: data() });
    await loaded();
    await fireEvent.click(save());
    expect(await screen.findByText(message)).toBeInTheDocument();
  });

  it('toasts other save errors', async () => {
    api.saveMailSettings.mockResolvedValue({
      error: 'x',
      response: { status: 500 }
    });
    render(MailPage, { data: data() });
    await loaded();
    await fireEvent.click(save());
    await vi.waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith('Failed to save mail settings')
    );
  });

  it('requires the SMTP fields when enabled', async () => {
    render(MailPage, {
      data: data(admin, { from_env: [], settings: { smtp_enabled: true } })
    });
    await vi.waitFor(() =>
      expect(screen.getByLabelText('Enable SMTP')).toBeChecked()
    );
    await fireEvent.click(save());
    expect(
      (
        await screen.findAllByText(
          'This field is required when SMTP is enabled.'
        )
      ).length
    ).toBeGreaterThanOrEqual(3);
    expect(api.saveMailSettings).not.toHaveBeenCalled();
  });

  it.each([
    [{ data: {} }, 'success', 'Test email sent successfully.'],
    [
      { error: 'x', response: { status: 429 } },
      'error',
      'Rate limit exceeded. Please try again later.'
    ],
    [
      { error: 'x', response: { status: 500 } },
      'error',
      'Failed to send test email. Check SMTP settings.'
    ]
  ] as const)('sends a test mail (%#)', async (result, kind, message) => {
    api.testMail.mockResolvedValue(result);
    render(MailPage, { data: data() });
    await loaded();
    await fireEvent.click(
      screen.getByRole('button', { name: /send test email/i })
    );
    await vi.waitFor(() => expect(toast[kind]).toHaveBeenCalledWith(message));
  });
});
