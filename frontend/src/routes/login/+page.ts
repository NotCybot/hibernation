import type { PageLoad } from './$types';
import { authConfig } from '$lib/client';
import { getRedirectTarget } from '$lib/redirect';

export const load: PageLoad = ({ fetch, url }) => {
  const error = url.searchParams.get('error') || undefined,
    redirectTo = getRedirectTarget(url.searchParams),
    // After an SSO failure never bounce straight back to the provider, or an
    // `instant_redirect` setup would loop.
    skip = url.searchParams.get('skip') === 'true' || error !== undefined,
    config = authConfig({ fetch }).then(({ data }) => data);
  return { config, error, redirectTo, skip };
};
