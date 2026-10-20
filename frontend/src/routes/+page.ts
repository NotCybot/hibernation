import { cacheOverview, listCaches, listTokens } from '$lib/client';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch }) => {
  const caches = listCaches({ fetch }).then(({ data }) => data);
  const tokens = listTokens({ fetch }).then(({ data }) => data);
  const overview = cacheOverview({ fetch }).then(({ data }) => data);

  return {
    caches,
    overview,
    tokens
  };
};
