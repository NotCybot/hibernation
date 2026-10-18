import { describe, expect, it } from 'vitest';
import {
  buildLoginUrl,
  getRedirectTarget,
  isSafeRedirectPath
} from '$lib/redirect';

describe('isSafeRedirectPath', () => {
  it.each([
    '/',
    '/caches',
    '/caches/abc/search?q=hello',
    '/password/reset?token=x',
    '/login-help',
    '/loginx',
    '/a#frag',
    '/%2Fevil.com'
  ])('accepts %s', (path) => {
    expect(isSafeRedirectPath(path)).toBe(true);
  });

  it.each([
    '',
    'caches',
    '//evil.com',
    '///evil.com',
    'http://evil.com',
    'https://evil.com/path',
    // oxlint-disable-next-line no-script-url
    'javascript:alert(1)',
    ' /caches',
    '/login',
    '/login?redirect=%2Fcaches',
    '/login#x',
    '/./login',
    '/caches/../login'
  ])('rejects %s', (path) => {
    expect(isSafeRedirectPath(path)).toBe(false);
  });
});

describe('buildLoginUrl', () => {
  it.each(['//evil.com', '/login', 'http://evil.com', ''])(
    'returns bare /login for unsafe %s',
    (path) => {
      expect(buildLoginUrl(path)).toBe('/login');
    }
  );

  it('appends an encoded redirect query for a safe path', () => {
    expect(buildLoginUrl('/caches')).toBe('/login?redirect=%2Fcaches');
    expect(buildLoginUrl('/groups/1?tab=2&x=y')).toBe(
      '/login?redirect=%2Fgroups%2F1%3Ftab%3D2%26x%3Dy'
    );
  });

  it('round-trips through getRedirectTarget', () => {
    const path = '/caches/a b?q=c&d=e';
    const url = new URL(buildLoginUrl(path), 'http://localhost');
    expect(getRedirectTarget(url.searchParams)).toBe(path);
  });
});

describe('getRedirectTarget', () => {
  it('returns a validated redirect param', () => {
    expect(getRedirectTarget(new URLSearchParams('redirect=%2Fcaches'))).toBe(
      '/caches'
    );
  });

  it.each(['', 'redirect=', 'redirect=//evil.com', 'redirect=/login'])(
    'falls back to / for %j',
    (query) => {
      expect(getRedirectTarget(new URLSearchParams(query))).toBe('/');
    }
  );

  it('uses the first redirect param when repeated', () => {
    expect(
      getRedirectTarget(new URLSearchParams('redirect=/a&redirect=//evil'))
    ).toBe('/a');
  });

  it('honours a custom fallback', () => {
    expect(getRedirectTarget(new URLSearchParams(), '/account')).toBe(
      '/account'
    );
    expect(
      getRedirectTarget(new URLSearchParams('redirect=//evil'), '/account')
    ).toBe('/account');
  });
});
