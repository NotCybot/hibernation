import { HttpResponse, http, ws } from 'msw';
import * as gen from '$lib/client/msw.gen';
import * as data from './data';

/**
 * No-op WebSocket mock for the updater channel. The app opens this socket on
 * every authenticated page (see `connectWebsocket`); without a handler the
 * preview server answers with `404`. Accept the connection and stay quiet so
 * no update events fire.
 */
const updaterWs = ws.link('*/api/ws/updater');

/**
 * Generated `handle*` factories match any origin (`*`) by default, so they
 * catch the browser's relative `/api/*` requests in the preview server.
 */
const j = (d: unknown) => HttpResponse.json(d as never) as never;
const scn = (cookies: Record<string, string>) => data.scenarioOf(cookies);
const anon = (cookies: Record<string, string>) => cookies.mock_anon === '1';

export const handlers = [
  updaterWs.addEventListener('connection', () => {}),

  // Setup / auth.
  gen.handleIsSetup(({ cookies }) => j(data.isSetupOf(cookies))),
  gen.handleGetOidcSettings(() => j(data.oidcSettings)),
  // Like the backend, an invalid session clears the auth cookie.
  gen.handleTestToken(({ cookies }) =>
    anon(cookies)
      ? (HttpResponse.json(
          { valid: false },
          { headers: { 'Set-Cookie': 'centaurus_jwt=; Max-Age=0; Path=/' } }
        ) as never)
      : j({ valid: true })
  ),
  gen.handleInfo(({ cookies }) =>
    anon(cookies)
      ? (new HttpResponse(null, { status: 401 }) as never)
      : j(data.userOf(cookies))
  ),
  gen.handleAuthConfig(() => j(data.authConfig)),
  gen.handleKey(() => j({ key: 'test-public-key' })),
  gen.handleMailActive(({ cookies }) => j(data.mailActiveOf(cookies))),

  // Settings.
  gen.handleGetGeneralSettings(() => j(data.generalSettings)),
  gen.handleGetUserSettings(() => j(data.userSettings)),
  gen.handleGetMailSettings(() => j(data.mailSettings)),

  // Lists (`mock_scenario=empty` cookie => empty state).
  gen.handleListCaches(({ cookies }) => j(data.caches[scn(cookies)])),
  gen.handleListTokens(({ cookies }) => j(data.tokens[scn(cookies)])),
  gen.handleListGroups(({ cookies }) => j(data.groups[scn(cookies)])),
  gen.handleListUsers(({ cookies }) => j(data.users[scn(cookies)])),
  gen.handleCacheOverview(({ cookies }) => j(data.overview[scn(cookies)])),
  gen.handleSearchStorePaths(({ cookies }) =>
    j(data.searchResults[scn(cookies)])
  ),

  // Simple lists used by detail/create pages.
  gen.handleListGroupsSimple(({ cookies }) =>
    j(data.simpleGroups[scn(cookies)])
  ),
  gen.handleListUsersSimple(({ cookies }) => j(data.simpleUsers[scn(cookies)])),
  gen.handleListCachesSimple(({ cookies }) =>
    j(data.simpleCaches[scn(cookies)])
  ),
  gen.handleListCachesSimpleGroup(({ cookies }) =>
    j(data.simpleCaches[scn(cookies)])
  ),

  // Details.
  gen.handleCacheDetails(({ cookies }) => j(data.cacheDetailsOf(cookies))),
  gen.handleGroupInfo(({ params }) =>
    j(
      (params as { uuid?: string }).uuid === 'group-staff'
        ? data.groupStaffDetails
        : data.groupDetails
    )
  ),
  gen.handleUserInfoDetail(() => j(data.userDetails)),
  gen.handleTokenInfo(() => j(data.tokens.default[0])),

  // Mutations return a generic success so submit flows resolve.
  gen.handleCreateCache(() => j({ uuid: 'cache-new' })),
  gen.handleCreateGroup(() => j({ uuid: 'group-new' })),
  gen.handleCreateUser(() => j({ uuid: 'user-new' })),
  gen.handleCreateToken(() => j({ token: 'secret-token', uuid: 'token-new' })),
  gen.handleTokenRegenerate(() => j({ token: 'secret-token-2' })),
  gen.handleNewCode(() => j({ code: 'cli-code' })),

  // Catch-all: any other `/api/*` call resolves with an empty 200 so unmocked
  // Endpoints never crash a page render.
  http.all('*/api/*', () => HttpResponse.json({}))
];
