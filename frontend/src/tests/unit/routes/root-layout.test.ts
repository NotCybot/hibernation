import { afterEach, describe, expect, it, vi } from 'vitest';
import { createRawSnippet } from 'svelte';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { goto } from '$app/navigation';

const state = vi.hoisted(() => {
  const route: { id: string | null } = { id: '/caches' };
  return { page: { route, url: new URL('http://x/caches?q=1') } };
});
vi.mock('$app/state', () => state);
const api = { logout: vi.fn(), testToken: vi.fn() };
vi.mock('$lib/client', async (orig) => ({
  ...(await orig<object>()),
  logout: (...a: unknown[]) => api.logout(...a),
  testToken: (...a: unknown[]) => api.testToken(...a)
}));
const ws = { connectWebsocket: vi.fn(), disconnectWebsocket: vi.fn() };
vi.mock('$lib/backend/updater.svelte', () => ({
  connectWebsocket: (u: string) => ws.connectWebsocket(u),
  disconnectWebsocket: () => ws.disconnectWebsocket()
}));

const Layout = (await import('$routes/+layout.svelte')).default;
const children = createRawSnippet(() => ({ render: () => '<p>child</p>' }));

const user = { email: 'b@x', name: 'Bob', permissions: [], uuid: 'u1' };
const data = (setup: object = { data: { is_setup: true } }) =>
  ({
    setupStatus: Promise.resolve(setup),
    user: Promise.resolve(user)
  }) as never;
const at = (id: string | null, url = `http://x${id ?? '/'}`) => {
  state.page.route.id = id;
  state.page.url = new URL(url);
};

afterEach(() => {
  api.logout.mockReset();
  api.testToken.mockReset();
  ws.connectWebsocket.mockReset();
  ws.disconnectWebsocket.mockReset();
  at('/caches', 'http://x/caches?q=1');
});

describe('root layout', () => {
  it('connects the websocket for a valid token and renders the sidebar', async () => {
    api.testToken.mockResolvedValue({ data: { valid: true } });
    render(Layout, { children, data: data() });
    expect(screen.getByText('child')).toBeInTheDocument();
    expect(await screen.findByText('Hibernation')).toBeInTheDocument();
    await vi.waitFor(() =>
      expect(ws.connectWebsocket).toHaveBeenCalledWith('u1')
    );
    expect(goto).not.toHaveBeenCalled();
  });

  it.each([{ data: { valid: false } }, { error: 'x' }])(
    'redirects to login with the current path on an invalid token (%#)',
    async (result) => {
      api.testToken.mockResolvedValue(result);
      render(Layout, { children, data: data() });
      await vi.waitFor(() =>
        expect(goto).toHaveBeenCalledWith('/login?redirect=%2Fcaches%3Fq%3D1')
      );
      expect(ws.connectWebsocket).not.toHaveBeenCalled();
    }
  );

  it('does not redirect public paths and hides the sidebar there', async () => {
    at('/login');
    api.testToken.mockResolvedValue({ data: { valid: false } });
    render(Layout, { children, data: data() });
    expect(screen.getByText('child')).toBeInTheDocument();
    await new Promise((r) => setTimeout(r, 10));
    expect(screen.queryByText('Hibernation')).toBeNull();
    expect(goto).not.toHaveBeenCalled();
  });

  it('sends an unconfigured instance to /setup instead of /login', async () => {
    let finishToken: (v: unknown) => void = () => {};
    api.testToken.mockReturnValue(new Promise((r) => (finishToken = r)));
    // Navigating lands on /setup, a public path.
    vi.mocked(goto).mockImplementationOnce(async () => at('/setup'));
    render(Layout, { children, data: data({ data: { is_setup: false } }) });
    await vi.waitFor(() => expect(goto).toHaveBeenCalledWith('/setup'));
    finishToken({ data: { valid: false } });
    await new Promise((r) => setTimeout(r, 10));
    expect(goto).toHaveBeenCalledTimes(1);
  });

  it('stays on /setup and ignores setup errors', async () => {
    api.testToken.mockResolvedValue({ data: { valid: true } });
    at('/setup');
    render(Layout, { children, data: data({ data: { is_setup: false } }) });
    await new Promise((r) => setTimeout(r, 10));
    expect(goto).not.toHaveBeenCalled();
    at('/caches');
    render(Layout, { children, data: data({ error: 'x' }) });
    await new Promise((r) => setTimeout(r, 10));
    expect(goto).not.toHaveBeenCalledWith('/setup');
  });

  it.each([
    [{ data: {} }, 1],
    [{ error: 'x' }, 0]
  ])('logs out and disconnects only on success (%#)', async (result, calls) => {
    api.testToken.mockResolvedValue({ data: { valid: true } });
    api.logout.mockResolvedValue(result);
    render(Layout, { children, data: data() });
    await screen.findByText('Bob');
    await fireEvent.click(screen.getAllByText('Bob')[0]);
    await fireEvent.click(await screen.findByText(/log ?out/i));
    await vi.waitFor(() => expect(api.logout).toHaveBeenCalledOnce());
    await vi.waitFor(() =>
      expect(ws.disconnectWebsocket).toHaveBeenCalledTimes(calls)
    );
  });
});
