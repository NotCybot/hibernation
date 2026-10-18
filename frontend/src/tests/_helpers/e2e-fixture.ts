import { handlers } from '../mocks/e2e/handlers';
import { test as testBase } from '@playwright/test';
import { type AnyHandler, HttpResponse } from 'msw';
import { type NetworkFixture, defineNetworkFixture } from '@msw/playwright';

interface Fixtures {
  handlers: AnyHandler[];
  network: NetworkFixture;
}

export const test = testBase.extend<Fixtures>({
  // Initial list of the network handlers.
  handlers: [handlers, { option: true }],

  // A fixture you use to control the network in your tests.
  network: [
    // oxlint-disable-next-line no-shadow
    async ({ context, handlers }, use) => {
      const network = defineNetworkFixture({
        context,
        handlers
      });

      await network.enable();
      await use(network);
      await network.disable();
    },
    { auto: true }
  ]
});

/** JSON response for `network.use` overrides (cast to satisfy typed resolvers). */
export const json = (body: unknown, status = 200) =>
  HttpResponse.json(body as never, { status }) as never;

/** Empty error response for `network.use` overrides. */
export const fail = (status = 500) =>
  new HttpResponse(null, { status }) as never;
