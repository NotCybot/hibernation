import { Permission } from '$lib/permissions.svelte';

/** Scenario name read from the `mock_scenario` cookie (set by the e2e tests). */
export type Scenario = 'default' | 'empty' | 'readonly';

/**
 * List data only varies between `default` and `empty`; `readonly` reuses the
 * default lists but serves a permission-less user and caches without write
 * access, so view-only UI can be exercised without new list fixtures.
 */
export const scenarioOf = (
  cookies: Record<string, string>
): 'default' | 'empty' =>
  cookies.mock_scenario === 'empty' ? 'empty' : 'default';

export const isReadonly = (cookies: Record<string, string>): boolean =>
  cookies.mock_scenario === 'readonly';

/** Admin user with every permission, so admin pages render full controls. */
export const adminUser = {
  email: 'admin@example.com',
  name: 'Ada Admin',
  oidc_user: false,
  permissions: Object.values(Permission) as string[],
  uuid: 'user-admin'
};

/** Same user without any permission (the `readonly` scenario). */
export const readonlyUser = { ...adminUser, permissions: [] as string[] };

export const userOf = (cookies: Record<string, string>) =>
  isReadonly(cookies) ? readonlyUser : adminUser;

const simpleUser = { id: 'user-1', name: 'Bob User' };
const simpleGroup = { name: 'Admins', uuid: 'group-admins' };
const simpleCache = { name: 'main', uuid: 'cache-main' };

export const isSetup = {
  db_backend: 'sqlite',
  is_setup: true,
  storage_backend: 'local'
};

/** Returned when the `mock_setup=pending` cookie is set, so /setup renders. */
export const isSetupPending = { ...isSetup, is_setup: false };

export const isSetupOf = (cookies: Record<string, string>) =>
  cookies.mock_setup === 'pending' ? isSetupPending : isSetup;

export const authConfig = {
  instant_redirect: false,
  mail_enabled: true,
  sso_type: 'None'
};

/** `mock_mail=off` cookie disables mail (admin-managed user controls). */
export const mailActiveOf = (cookies: Record<string, string>) => ({
  active: cookies.mock_mail !== 'off'
});

export const oidcSettings = {
  from_env: [] as string[],
  settings: {},
  site_url: 'https://hibernation.example/'
};
export const generalSettings = {
  site_url: 'https://hibernation.example/',
  virtual_host_routing: false
};
export const userSettings = { from_env: [] as string[], settings: {} };
export const mailSettings = {
  from_env: [] as string[],
  settings: { smtp_enabled: false, smtp_from_name: 'Hibernation' }
};

export const caches = {
  default: [
    {
      name: 'main',
      public: true,
      quota: 10 * 1024 ** 3,
      size: 1024 ** 3,
      uuid: 'cache-main'
    },
    {
      name: 'private',
      public: false,
      quota: 1024 ** 3,
      size: null,
      uuid: 'cache-private'
    }
  ],
  empty: [] as unknown[]
};

export const cacheDetails = {
  allow_force_push: false,
  downstream_caches: ['https://cache.nixos.org'],
  eviction_policy: 'LeastRecentlyUsed',
  has_write_access: true,
  name: 'main',
  nar_count: 42,
  priority: 40,
  public: true,
  quota: 10 * 1024 ** 3,
  sig_key: 'main:dGVzdC1wdWJsaWMta2V5',
  size: 1024 ** 3,
  uuid: 'cache-main'
};

export const cacheDetailsOf = (cookies: Record<string, string>) =>
  isReadonly(cookies)
    ? { ...cacheDetails, has_write_access: false }
    : cacheDetails;

export const searchResults = {
  default: [
    {
      accessed: 3,
      created_at: '2026-01-01T00:00:00Z',
      last_accessed_at: '2026-01-02T00:00:00Z',
      size: 2048,
      store_path: '/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-hello-2.12'
    }
  ],
  empty: [] as unknown[]
};

export const groups = {
  default: {
    admin_group: 'group-admins',
    groups: [
      {
        id: 'group-admins',
        name: 'Admins',
        permissions: [Permission.USER_VIEW, Permission.GROUP_VIEW],
        users: [simpleUser]
      },
      { id: 'group-staff', name: 'Staff', permissions: [], users: [] }
    ]
  },
  empty: { admin_group: undefined, groups: [] as unknown[] }
};

export const groupDetails = {
  admin_group: 'group-admins',
  group: {
    caches: [{ access_type: 'Edit', ...simpleCache }],
    id: 'group-admins',
    name: 'Admins',
    permissions: [Permission.USER_VIEW],
    users: [simpleUser]
  }
};

/** Non-admin group, so the detail page renders the editable permissions. */
export const groupStaffDetails = {
  admin_group: 'group-admins',
  group: {
    caches: [] as unknown[],
    id: 'group-staff',
    name: 'Staff',
    permissions: [] as string[],
    users: [] as unknown[]
  }
};

export const users = {
  default: [
    {
      email: 'bob@example.com',
      groups: [simpleGroup],
      name: 'Bob User',
      uuid: 'user-1'
    },
    {
      email: 'cara@example.com',
      groups: [],
      name: 'Cara User',
      uuid: 'user-2'
    }
  ],
  empty: [] as unknown[]
};

export const userDetails = {
  caches: [{ access_type: 'View', ...simpleCache }],
  email: 'bob@example.com',
  groups: [simpleGroup],
  name: 'Bob User',
  oidc_user: false,
  permissions: [Permission.USER_VIEW],
  uuid: 'user-1'
};

export const tokens = {
  default: [
    {
      exp: '2099-01-01T00:00:00Z',
      last_used: '2026-01-01T00:00:00Z',
      name: 'ci',
      uuid: 'token-ci'
    },
    {
      exp: '2020-01-01T00:00:00Z',
      last_used: null,
      name: 'old',
      uuid: 'token-old'
    }
  ],
  empty: [] as unknown[]
};

export const simpleUsers = { default: [simpleUser], empty: [] as unknown[] };
export const simpleGroups = { default: [simpleGroup], empty: [] as unknown[] };
export const simpleCaches = { default: [simpleCache], empty: [] as unknown[] };
