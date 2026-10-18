import { beforeAll, vi } from 'vitest';
import type { Writable } from 'svelte/store';
import { client as appClient } from '$lib/client/client.gen';

// In the browser the client uses relative `/api/...` URLs, but Node's `fetch`
// (used under jsdom) rejects relative URLs, so requests must be absolute. Pin a
// Test origin: the generated MSW handlers read this same `baseUrl`, so the
// Request URL and the handler pattern stay aligned.
const TEST_BASE_URL = 'http://localhost';

// Start the MSW server once for the whole run. `onUnhandledRequest: 'error'`
// Makes any un-mocked `/api/...` call fail the test loudly; non-API requests
// (e.g. assets) are let through.
beforeAll(() => {
  appClient.setConfig({ ...appClient.getConfig(), baseUrl: TEST_BASE_URL });
});

// Superforms (SPA mode) finishes a submit with `applyAction(result)`, which in
// A real app writes the result into the `page` store; superforms subscribes to
// That store to apply returned errors. Re-mock `$app/stores` with a writable
// `page` and make `applyAction` feed it, so field errors reach the DOM.
vi.mock('$app/stores', async () => {
  const { readable, writable } = await import('svelte/store');
  const page = writable({
    data: {},
    error: null,
    form: null,
    params: {},
    route: { id: null },
    status: 200,
    url: new URL('http://localhost/')
  });
  const navigating = readable(null);
  const updated = {
    check: async () => Promise.resolve(false),
    subscribe: readable(false).subscribe
  };
  return {
    getStores: () => ({ navigating, page, updated }),
    navigating,
    page,
    updated
  };
});

vi.mock('$app/forms', async (importOriginal) => {
  const { page } = await import('$app/stores');
  return {
    ...(await importOriginal<object>()),
    applyAction: vi.fn(
      async (result: { type: string; status?: number; data?: unknown }) => {
        if (result.type === 'success' || result.type === 'failure') {
          (page as unknown as Writable<object>).update((p) => ({
            ...p,
            form: result.data,
            status: result.status ?? 200
          }));
        }
      }
    )
  };
});

// Jsdom lacks `elementFromPoint`; the pleiades OTP input calls it on input.
if (typeof document !== 'undefined' && !document.elementFromPoint) {
  document.elementFromPoint = () => null;
}

// Jsdom lacks the Web Animations API used by svelte transitions.
if (typeof Element !== 'undefined' && !Element.prototype.animate) {
  Element.prototype.animate = function animate() {
    const animation = {
      cancel: () => {},
      finished: Promise.resolve(),
      onfinish: null as null | (() => void),
      pause: () => {},
      play: () => {}
    };
    queueMicrotask(() => animation.onfinish?.());
    return animation as unknown as Animation;
  };
}

// Jsdom lacks pointer capture, which bits-ui Select calls on pointerdown.
if (typeof Element !== 'undefined' && !Element.prototype.hasPointerCapture) {
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
}
