import { type Page, expect } from '@playwright/test';
import * as gen from '$lib/client/msw.gen';
import { fail, json, test } from '$test_helpers/e2e-fixture';
import { setupSession } from '$test_helpers/session';
import { expectNoHorizontalOverflow, gotoReady } from '$test_helpers/layout';
import { cacheDetails, generalSettings, searchResults } from '$mocks/e2e/data';

const STORE_PATH = searchResults.default[0].store_path;

/** Captures every `editCache` body. */
const captureEdit = (status = 200) => {
  const bodies: Record<string, unknown>[] = [];
  const handler = gen.handleEditCache(async ({ request }) => {
    bodies.push(await request.json());
    return status === 200 ? json({}) : fail(status);
  });
  return { bodies, handler };
};

const openSettings = async (page: Page) => {
  await gotoReady(page, '/caches/cache-main/settings');
  await expect(page.getByPlaceholder('Enter cache name')).toHaveValue('main');
  // The downstream-cache suggestion list can pop open on load and cover the
  // Form; blur it by clicking a neutral element.
  await page.getByRole('heading', { name: /Cache:/ }).click();
  await expect(page.getByRole('listbox')).toHaveCount(0);
};

test.beforeEach(async ({ context }) => setupSession(context));

test.describe('caches list', () => {
  test('renders a card per cache with visibility and usage', async ({
    page
  }) => {
    await gotoReady(page, '/caches');

    const main = page.locator('a[href="/caches/cache-main"]');
    await expect(main).toContainText('Public');
    await expect(main).toContainText('1.00GiB / 10.00GiB (10.0%)');
    const priv = page.locator('a[href="/caches/cache-private"]');
    await expect(priv).toContainText('Private');
    // A cache without any size yet counts as empty.
    await expect(priv).toContainText('0.00GiB / 1.00GiB (0.0%)');
    await expectNoHorizontalOverflow(page);
  });

  test('opens a cache from its card', async ({ page }) => {
    await gotoReady(page, '/caches');
    await page.locator('a[href="/caches/cache-main"]').click();
    await expect(page).toHaveURL(/\/caches\/cache-main$/);
  });

  test('shows an empty state with no caches', async ({ context, page }) => {
    await setupSession(context, 'empty');
    await gotoReady(page, '/caches');

    await expect(
      page.getByText('No caches found. Create one to get started.')
    ).toBeVisible();
  });

  test('renders no cards when the list fails to load', async ({
    network,
    page
  }) => {
    network.use(gen.handleListCaches(() => fail(500)));
    await gotoReady(page, '/caches');

    await expect(page.getByRole('heading', { name: 'Caches' })).toBeVisible();
    // Regression: this used to stay on "Loading caches..." forever.
    await expect(page.getByText('Failed to load caches.')).toBeVisible();
    await expect(page.getByText('Loading caches...')).toHaveCount(0);
    await expect(page.locator('a[href^="/caches/cache-"]')).toHaveCount(0);
  });

  test('disables create without cache:create', async ({ context, page }) => {
    await setupSession(context, 'readonly');
    await gotoReady(page, '/caches');

    await expect(page.locator('a[href="/caches/cache-main"]')).toBeVisible();
    await expect(page.getByRole('link', { name: 'Create' })).toHaveAttribute(
      'aria-disabled',
      'true'
    );
  });
});

test.describe('cache create', () => {
  const fill = async (page: Page, name = 'my-cache') => {
    await page.getByPlaceholder('Cache name').fill(name);
    await page
      .getByPlaceholder('cache.example.com-1:mB9FSh9qf2...')
      .fill('my-cache-1:abc=');
  };

  test('creates a cache with the entered values', async ({ network, page }) => {
    let body = undefined as Record<string, unknown> | undefined;
    network.use(
      gen.handleCreateCache(async ({ request }) => {
        body = await request.json();
        return json({ uuid: 'cache-main' });
      })
    );
    await gotoReady(page, '/caches/create');
    await expectNoHorizontalOverflow(page);

    await fill(page);
    await page.getByPlaceholder('10240').fill('2048');
    await page.getByRole('switch', { name: 'Public Cache' }).click();
    await page.getByRole('button', { name: 'Create' }).click();

    await expect(page.getByText('Cache created successfully.')).toBeVisible();
    await expect(page).toHaveURL(/\/caches\/cache-main$/);
    // Quota is sent in MiB; the backend converts it to bytes.
    expect(body).toEqual({
      name: 'my-cache',
      public: true,
      quota: 2048,
      sig_key: 'my-cache-1:abc='
    });
  });

  // Mirrors the backend `CacheRegex`; assert behaviour, not wording.
  for (const name of ['-bad', 'bad-', 'has space', '123', 'a'.repeat(64)]) {
    test(`rejects the name "${name.slice(0, 10)}"`, async ({
      network,
      page
    }) => {
      let called = false;
      network.use(
        gen.handleCreateCache(() => {
          called = true;
          return json({ uuid: 'cache-main' });
        })
      );
      await gotoReady(page, '/caches/create');

      await fill(page, name);
      await page.getByRole('button', { name: 'Create' }).click();

      await expect(page.getByPlaceholder('Cache name')).toHaveAttribute(
        'aria-invalid',
        'true'
      );
      await expect(page).toHaveURL(/\/caches\/create$/);
      expect(called).toBe(false);
    });
  }

  for (const name of ['1cache', 'a'.repeat(63), 'My-Cache-2']) {
    test(`accepts the name "${name.slice(0, 10)}"`, async ({
      network,
      page
    }) => {
      let body = undefined as Record<string, unknown> | undefined;
      network.use(
        gen.handleCreateCache(async ({ request }) => {
          body = await request.json();
          return json({ uuid: 'cache-main' });
        })
      );
      await gotoReady(page, '/caches/create');

      await fill(page, name);
      await page.getByRole('button', { name: 'Create' }).click();

      await expect(page).toHaveURL(/\/caches\/cache-main$/);
      expect(body?.name).toBe(name);
    });
  }

  test('requires a signing key and a positive quota', async ({ page }) => {
    await gotoReady(page, '/caches/create');

    await page.getByPlaceholder('Cache name').fill('my-cache');
    await page.getByPlaceholder('cache.example.com-1:mB9FSh9qf2...').fill('x');
    await page.getByPlaceholder('cache.example.com-1:mB9FSh9qf2...').fill('');
    await page.getByPlaceholder('10240').fill('0');
    await page.getByRole('button', { name: 'Create' }).click();

    await expect(page.getByText('Signature key is required')).toBeVisible();
    await expect(
      page.getByText('Quota must be a positive number')
    ).toBeVisible();
  });

  for (const [status, message] of [
    [409, 'A cache with this name already exists.'],
    [406, 'Invalid signature key format.'],
    [500, 'Error creating cache.']
  ] as const) {
    test(`surfaces a ${status}`, async ({ network, page }) => {
      network.use(gen.handleCreateCache(() => fail(status)));
      await gotoReady(page, '/caches/create');

      await fill(page);
      await page.getByRole('button', { name: 'Create' }).click();

      await expect(page.getByText(message)).toBeVisible();
      await expect(page).toHaveURL(/\/caches\/create$/);
    });
  }
});

test.describe('cache overview', () => {
  test('shows usage and the nix configuration', async ({ page }) => {
    await gotoReady(page, '/caches/cache-main');

    await expect(page.getByRole('heading', { name: /Cache:/ })).toContainText(
      'main'
    );
    const main = page.getByRole('main');
    await expect(main).toContainText('Number of paths:');
    await expect(main).toContainText('42');
    await expect(main).toContainText('1.00GiB / 10.00GiB (10.0%)');
    await expect(main).toContainText('hibernation push main /nix/store/...');
    await expect(main).toContainText(
      'https://hibernation.example/api/nix/main'
    );
    await expect(main).toContainText(cacheDetails.sig_key);
    await expectNoHorizontalOverflow(page);
  });

  // Regression: a base path in site_url was dropped from the cache url.
  test('keeps the base path of the site url', async ({ network, page }) => {
    network.use(
      gen.handleGetGeneralSettings(() =>
        json({ ...generalSettings, site_url: 'https://example.org/hib/' })
      )
    );
    await gotoReady(page, '/caches/cache-main');

    await expect(page.getByRole('main')).toContainText(
      'https://example.org/hib/api/nix/main'
    );
  });

  test('uses a subdomain url with virtual host routing', async ({
    network,
    page
  }) => {
    network.use(
      gen.handleGetGeneralSettings(() =>
        json({ ...generalSettings, virtual_host_routing: true })
      )
    );
    await gotoReady(page, '/caches/cache-main');

    await expect(page.getByRole('main')).toContainText(
      'https://main.hibernation.example'
    );
    await expect(page.getByRole('main')).not.toContainText('/api/nix/main');
  });

  test('navigates between the cache tabs', async ({ page }) => {
    await gotoReady(page, '/caches/cache-main');

    await page.locator('a[href="/caches/cache-main/search"]').click();
    await expect(page).toHaveURL(/\/search$/);
    await page.locator('a[href="/caches/cache-main/settings"]').click();
    await expect(page).toHaveURL(/\/settings$/);
    await page.locator('a[href="/caches/cache-main"]').last().click();
    await expect(page).toHaveURL(/\/caches\/cache-main$/);
  });

  for (const [status, message] of [
    [404, 'Cache not found'],
    [500, 'Failed to load cache']
  ] as const) {
    test(`redirects to the list on a ${status}`, async ({ network, page }) => {
      network.use(gen.handleCacheDetails(() => fail(status)));
      await gotoReady(page, '/caches/cache-main');

      await expect(page).toHaveURL(/\/caches$/);
      await expect(page.getByText(message)).toBeVisible();
    });
  }
});

test.describe('cache search', () => {
  test('lists matching store paths', async ({ page }) => {
    await gotoReady(page, '/caches/cache-main/search');

    const row = page.getByRole('row').filter({ hasText: STORE_PATH });
    await expect(row).toContainText('0.00 MiB');
    await expect(row).toContainText('3');
    await expectNoHorizontalOverflow(page);
  });

  test('sends the query, sort and order', async ({ network, page }) => {
    const bodies: Record<string, string>[] = [];
    network.use(
      gen.handleSearchStorePaths(async ({ request }) => {
        bodies.push(await request.json());
        return json(searchResults.default);
      })
    );
    await gotoReady(page, '/caches/cache-main/search');
    await expect
      .poll(() => bodies.at(-1))
      .toEqual({
        order: 'Asc',
        query: '',
        sort: 'StorePath'
      });

    await page.getByPlaceholder('Search...').fill('hello');
    await expect.poll(() => bodies.at(-1)?.query).toBe('hello');

    await page.getByRole('button', { name: 'StorePath' }).click();
    await page.getByRole('option', { name: 'Size' }).click();
    await expect.poll(() => bodies.at(-1)?.sort).toBe('Size');

    await page.getByRole('button', { name: 'Asc' }).click();
    await page.getByRole('option', { name: 'Desc' }).click();
    await expect
      .poll(() => bodies.at(-1))
      .toEqual({
        order: 'Desc',
        query: 'hello',
        sort: 'Size'
      });
  });

  test('shows an empty table when nothing matches', async ({
    context,
    page
  }) => {
    await setupSession(context, 'empty');
    await gotoReady(page, '/caches/cache-main/search');

    await expect(page.getByText('No results.')).toBeVisible();
  });

  test('surfaces a failed search', async ({ network, page }) => {
    network.use(gen.handleSearchStorePaths(() => fail(500)));
    await gotoReady(page, '/caches/cache-main/search');

    await expect(page.getByText('Failed to search cache')).toBeVisible();
  });

  test('deletes a path and refreshes the results', async ({
    network,
    page
  }) => {
    let body = undefined as unknown;
    let searches = 0;
    network.use(
      gen.handleSearchStorePaths(() => {
        searches += 1;
        return json(searchResults.default);
      }),
      gen.handleDeletePath(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await gotoReady(page, '/caches/cache-main/search');
    await expect(page.getByText(STORE_PATH)).toBeVisible();
    const before = searches;

    await page.getByRole('button', { name: 'Delete store path' }).click();

    await expect.poll(() => body).toEqual({ store_path: STORE_PATH });
    await expect.poll(() => searches).toBeGreaterThan(before);
  });

  test('surfaces a failed delete', async ({ network, page }) => {
    network.use(gen.handleDeletePath(() => fail(500)));
    await gotoReady(page, '/caches/cache-main/search');

    await page.getByRole('button', { name: 'Delete store path' }).click();

    await expect(page.getByText('Failed to delete path')).toBeVisible();
  });

  test('disables deletion without write access', async ({ context, page }) => {
    await setupSession(context, 'readonly');
    await gotoReady(page, '/caches/cache-main/search');

    await expect(page.getByText(STORE_PATH)).toBeVisible();
    await expect(
      page.getByRole('button', { name: 'Delete store path' })
    ).toBeDisabled();
  });
});

test.describe('cache settings', () => {
  test('prefills the current configuration', async ({ page }) => {
    await openSettings(page);

    await expect(page.getByPlaceholder('Enter priority')).toHaveValue('40');
    await expect(
      page.getByRole('combobox').filter({ hasText: 'Least Recently Used' })
    ).toBeVisible();
    await expect(
      page.getByText('https://cache.nixos.org', { exact: true })
    ).toBeVisible();
    await expect(
      page.getByRole('switch', { name: 'Allow Force Push' })
    ).not.toBeChecked();
    await expectNoHorizontalOverflow(page);
  });

  test('saves priority, eviction, downstream caches and force push', async ({
    network,
    page
  }) => {
    const { bodies, handler } = captureEdit();
    network.use(handler);
    await openSettings(page);

    await page.getByPlaceholder('Enter priority').fill('10');
    await page
      .getByRole('combobox')
      .filter({ hasText: 'Least Recently Used' })
      .click();
    await page.getByRole('option', { name: 'Oldest First' }).click();
    const downstream = page.getByPlaceholder('Add a downstream cache');
    await downstream.fill('not a url');
    await downstream.press('Enter');
    await downstream.fill('https://cache.garnix.io/');
    await downstream.press('Enter');
    // Close the suggestion list so it doesn't cover the switch below.
    await downstream.press('Escape');
    await page.getByRole('heading', { name: /Cache:/ }).click();
    await page.getByRole('switch', { name: 'Allow Force Push' }).click();
    await page.getByRole('button', { name: 'Save Changes' }).click();

    await expect(
      page.getByText('Cache main updated successfully')
    ).toBeVisible();
    expect(bodies[0]).toMatchObject({
      allow_force_push: true,
      downstream_caches: [
        'https://cache.nixos.org',
        'https://cache.garnix.io/'
      ],
      eviction_policy: 'OldestFirst',
      name: 'main',
      priority: 10,
      uuid: 'cache-main'
    });
  });

  test('validates name and priority', async ({ page }) => {
    await openSettings(page);

    await page.getByPlaceholder('Enter cache name').fill('-bad');
    await page.getByPlaceholder('Enter priority').fill('-1');
    await page.getByRole('button', { name: 'Save Changes' }).click();

    await expect(page.getByPlaceholder('Enter cache name')).toHaveAttribute(
      'aria-invalid',
      'true'
    );
    await expect(
      page.getByText('Priority must be a non-negative integer')
    ).toBeVisible();
  });

  for (const [status, message] of [
    [409, 'This cache name is already in use'],
    [500, 'Failed to update cache']
  ] as const) {
    test(`surfaces a ${status} on save`, async ({ network, page }) => {
      network.use(captureEdit(status).handler);
      await openSettings(page);

      await page.getByRole('button', { name: 'Save Changes' }).click();

      await expect(page.getByText(message)).toBeVisible();
    });
  }

  test('makes a public cache private', async ({ network, page }) => {
    const { bodies, handler } = captureEdit();
    network.use(handler);
    await openSettings(page);

    await page.getByRole('button', { name: 'Make Private' }).click();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Make Private' })
      .click();

    await expect(page.getByText('Cache main is now private')).toBeVisible();
    expect(bodies[0]).toMatchObject({ public: false, uuid: 'cache-main' });
  });

  test('makes a private cache public', async ({ network, page }) => {
    const { bodies, handler } = captureEdit();
    network.use(
      handler,
      gen.handleCacheDetails(() => json({ ...cacheDetails, public: false }))
    );
    await openSettings(page);

    await page.getByRole('button', { name: 'Make Public' }).click();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Make Public' })
      .click();

    await expect(page.getByText('Cache main is now public')).toBeVisible();
    expect(bodies[0]).toMatchObject({ public: true });
  });

  test('surfaces a failed visibility change', async ({ network, page }) => {
    network.use(captureEdit(500).handler);
    await openSettings(page);

    await page.getByRole('button', { name: 'Make Private' }).click();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Make Private' })
      .click();

    await expect(
      page.getByText('Failed to change cache visibility for main')
    ).toBeVisible();
  });

  test('changes the quota (MiB -> bytes)', async ({ network, page }) => {
    const { bodies, handler } = captureEdit();
    network.use(handler);
    await openSettings(page);

    await page.getByRole('button', { name: 'Change Quota' }).click();
    const dialog = page.getByRole('dialog');
    const quota = dialog.getByPlaceholder('Enter new cache quota in MiB');
    await expect(quota).toHaveValue('10240');
    await quota.fill('2048');
    await dialog.getByRole('button', { name: 'Change' }).click();

    await expect(
      page.getByText('Cache main quota updated successfully')
    ).toBeVisible();
    expect(bodies[0]).toMatchObject({ quota: 2048 * 1024 * 1024 });
  });

  test('rejects a non-positive quota', async ({ page }) => {
    await openSettings(page);

    await page.getByRole('button', { name: 'Change Quota' }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByPlaceholder('Enter new cache quota in MiB').fill('0');
    await dialog.getByRole('button', { name: 'Change' }).click();

    await expect(
      dialog.getByText('Quota must be a positive number')
    ).toBeVisible();
  });

  test('surfaces a failed quota change', async ({ network, page }) => {
    network.use(captureEdit(500).handler);
    await openSettings(page);

    await page.getByRole('button', { name: 'Change Quota' }).click();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Change' })
      .click();

    await expect(page.getByText('Failed to change cache quota')).toBeVisible();
  });

  test('changes the signing key', async ({ network, page }) => {
    const { bodies, handler } = captureEdit();
    network.use(handler);
    await openSettings(page);

    await page.getByRole('button', { name: 'Change Key' }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByRole('button', { name: 'Change' }).click();
    await expect(dialog.getByText('Signature key is required')).toBeVisible();

    await dialog
      .getByPlaceholder('Enter new public signing key')
      .fill('main-2:bmV3');
    await dialog.getByRole('button', { name: 'Change' }).click();

    await expect(
      page.getByText('Cache main key updated successfully')
    ).toBeVisible();
    expect(bodies[0]).toMatchObject({ sig_key: 'main-2:bmV3' });
  });

  for (const [status, message] of [
    [406, 'The provided key is not a valid public signing key'],
    [500, 'Failed to change cache key']
  ] as const) {
    test(`surfaces a ${status} on key change`, async ({ network, page }) => {
      network.use(captureEdit(status).handler);
      await openSettings(page);

      await page.getByRole('button', { name: 'Change Key' }).click();
      const dialog = page.getByRole('dialog');
      await dialog
        .getByPlaceholder('Enter new public signing key')
        .fill('main-2:bmV3');
      await dialog.getByRole('button', { name: 'Change' }).click();

      await expect(page.getByText(message)).toBeVisible();
    });
  }

  test('clears the cache', async ({ network, page }) => {
    let called = false;
    network.use(
      gen.handleClearCache(() => {
        called = true;
        return json({});
      })
    );
    await openSettings(page);

    await page.getByRole('button', { name: 'Clear Cache' }).click();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Clear' })
      .click();

    await expect(
      page.getByText('Cache main cleared successfully')
    ).toBeVisible();
    expect(called).toBe(true);
  });

  test('surfaces a failed clear', async ({ network, page }) => {
    network.use(gen.handleClearCache(() => fail(500)));
    await openSettings(page);

    await page.getByRole('button', { name: 'Clear Cache' }).click();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Clear' })
      .click();

    await expect(page.getByText('Failed to clear cache')).toBeVisible();
  });

  test('deletes the cache and returns to the list', async ({
    network,
    page
  }) => {
    let body = undefined as unknown;
    network.use(
      gen.handleDeleteCache(async ({ request }) => {
        body = await request.json();
        return json({});
      })
    );
    await openSettings(page);

    await page.getByRole('button', { name: 'Delete Cache' }).click();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Delete' })
      .click();

    await expect(page).toHaveURL(/\/caches$/);
    expect(body).toEqual({ uuid: 'cache-main' });
  });

  test('surfaces a failed delete', async ({ network, page }) => {
    network.use(gen.handleDeleteCache(() => fail(500)));
    await openSettings(page);

    await page.getByRole('button', { name: 'Delete Cache' }).click();
    await page
      .getByRole('dialog')
      .getByRole('button', { name: 'Delete' })
      .click();

    await expect(page.getByText('Failed to delete cache')).toBeVisible();
  });

  test('is read-only without write access', async ({ context, page }) => {
    await setupSession(context, 'readonly');
    await openSettings(page);

    await expect(page.getByPlaceholder('Enter cache name')).toHaveAttribute(
      'readonly'
    );
    await expect(
      page.getByRole('button', { name: 'Save Changes' })
    ).toBeDisabled();
    await expect(
      page.getByRole('switch', { name: 'Allow Force Push' })
    ).toBeDisabled();
    // Regression: downstream caches and eviction policy stayed editable.
    await expect(
      page.getByPlaceholder('Add a downstream cache')
    ).toBeDisabled();
    await expect(
      page.getByRole('combobox').filter({ hasText: 'Least Recently Used' })
    ).toBeDisabled();
    for (const name of [
      'Make Private',
      'Change Quota',
      'Change Key',
      'Clear Cache',
      'Delete Cache'
    ]) {
      // oxlint-disable-next-line no-await-in-loop
      await expect(page.getByRole('button', { name })).toBeDisabled();
    }
  });
});
