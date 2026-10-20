<script lang="ts">
  import * as Card from '@profidev/pleiades/components/ui/card';
  import { Button } from '@profidev/pleiades/components/ui/button';
  import { Progress } from '@profidev/pleiades/components/ui/progress';
  import { ScrollArea } from '@profidev/pleiades/components/ui/scroll-area';
  import { size_to_gib } from '$lib/backend/util.svelte.js';
  import type {
    CacheInfo,
    CacheOverview,
    OverviewPath,
    TokenInfo
  } from '$lib/client/types.gen.js';

  const { data } = $props();

  // `undefined` while loading, `null` when the API call failed.
  let caches: CacheInfo[] | null | undefined = $state();
  let tokens: TokenInfo[] | null | undefined = $state();
  let overview: CacheOverview | null | undefined = $state();

  $effect(() => {
    data.caches.then((c) => (caches = c ?? null));
  });
  $effect(() => {
    data.tokens.then((t) => (tokens = t ?? null));
  });
  $effect(() => {
    data.overview.then((o) => (overview = o ?? null));
  });

  let size_gib = $derived(
    size_to_gib(caches?.reduce((sum, c) => sum + (c.size ?? 0), 0) ?? 0)
  );
  let quota_gib = $derived(
    size_to_gib(caches?.reduce((sum, c) => sum + c.quota, 0) ?? 0)
  );
  let activeTokens = $derived(
    tokens
      ?.filter((t) => new Date(t.exp) > new Date())
      .sort((a, b) => new Date(a.exp).getTime() - new Date(b.exp).getTime())
  );

  const formatDate = (value: Date | null | undefined) =>
    value
      ? new Date(value).toLocaleString(
          navigator.languages || [navigator.language]
        )
      : 'Never';

  // `/nix/store/<hash>-<name>` => `<name>`, the hash is noise in a short list
  const pathName = (storePath: string) =>
    storePath.replace(/^\/nix\/store\/[a-z0-9]{32}-/, '');
</script>

{#snippet usageBar(size: number, quota: number)}
  {@const usage_percent = (size / quota) * 100}
  <Progress
    min={0}
    max={quota}
    value={size}
    class={(usage_percent > 90
      ? '*:data-[slot=progress-indicator]:bg-red-500'
      : usage_percent > 70
        ? '*:data-[slot=progress-indicator]:bg-yellow-500'
        : '') + ' h-2'}
  />
{/snippet}

{#snippet pathList(
  paths: OverviewPath[] | undefined,
  empty: string,
  detail: (path: OverviewPath) => string
)}
  {#if overview === undefined}
    <p class="text-muted-foreground">Loading...</p>
  {:else if overview === null}
    <p class="text-destructive">Failed to load paths.</p>
  {:else if !paths?.length}
    <p class="text-muted-foreground">{empty}</p>
  {:else}
    <ul class="flex flex-col gap-2">
      {#each paths as path}
        <li class="flex min-w-0 items-center gap-2">
          <div class="min-w-0">
            <p class="truncate" title={path.store_path}>
              {pathName(path.store_path)}
            </p>
            <a
              class="text-muted-foreground text-sm hover:underline"
              href={'/caches/' + path.cache_uuid}
            >
              {path.cache_name}
            </a>
          </div>
          <p class="text-muted-foreground ml-auto shrink-0 text-sm">
            {detail(path)}
          </p>
        </li>
      {/each}
    </ul>
  {/if}
{/snippet}

<div class="flex h-full flex-col p-4">
  <h3 class="ml-7 text-xl font-medium md:m-0">Overview</h3>
  <ScrollArea class="mt-2 min-h-0 grow">
    <div class="flex flex-col gap-4">
      <div class="grid grid-cols-1 gap-4 sm:grid-cols-3">
        <Card.Root>
          <Card.Header>
            <Card.Description>Caches</Card.Description>
            <Card.Title class="text-2xl">{caches?.length ?? '-'}</Card.Title>
          </Card.Header>
        </Card.Root>
        <Card.Root>
          <Card.Header>
            <Card.Description>Storage</Card.Description>
            <Card.Title class="text-2xl">
              {caches ? `${size_gib.toFixed(2)} GiB` : '-'}
            </Card.Title>
          </Card.Header>
          {#if caches?.length}
            <Card.Content class="flex flex-col gap-1">
              <p class="text-muted-foreground text-sm">
                of {quota_gib.toFixed(2)} GiB quota
              </p>
              {@render usageBar(size_gib, quota_gib)}
            </Card.Content>
          {/if}
        </Card.Root>
        <Card.Root>
          <Card.Header>
            <Card.Description>Active tokens</Card.Description>
            <Card.Title class="text-2xl">
              {activeTokens?.length ?? '-'}
            </Card.Title>
          </Card.Header>
        </Card.Root>
      </div>

      <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <Card.Root>
          <Card.Header>
            <Card.Title>Caches</Card.Title>
          </Card.Header>
          <Card.Content>
            {#if caches === undefined}
              <p class="text-muted-foreground">Loading...</p>
            {:else if caches === null}
              <p class="text-destructive">Failed to load caches.</p>
            {:else if caches.length === 0}
              <p class="text-muted-foreground">No caches yet.</p>
              <Button class="mt-2" href="/caches/create">Create cache</Button>
            {:else}
              <ul class="flex flex-col gap-3">
                {#each caches as cache}
                  {@const size = size_to_gib(cache.size ?? 0)}
                  {@const quota = size_to_gib(cache.quota)}
                  <li>
                    <a
                      class="flex flex-col gap-1 hover:underline"
                      href={'/caches/' + cache.uuid}
                    >
                      <div class="flex">
                        <span>{cache.name}</span>
                        <span class="text-muted-foreground ml-auto text-sm">
                          {size.toFixed(2)} / {quota.toFixed(2)} GiB
                        </span>
                      </div>
                      {@render usageBar(size, quota)}
                    </a>
                  </li>
                {/each}
              </ul>
            {/if}
          </Card.Content>
        </Card.Root>

        <Card.Root>
          <Card.Header>
            <Card.Title>Active tokens</Card.Title>
            <Card.Description>Expiring first</Card.Description>
          </Card.Header>
          <Card.Content>
            {#if tokens === undefined}
              <p class="text-muted-foreground">Loading...</p>
            {:else if tokens === null}
              <p class="text-destructive">Failed to load tokens.</p>
            {:else if !activeTokens?.length}
              <p class="text-muted-foreground">No active tokens.</p>
            {:else}
              <ul class="flex flex-col gap-2">
                {#each activeTokens as token}
                  <li class="flex items-center gap-2">
                    <a class="hover:underline" href={'/tokens/' + token.uuid}>
                      {token.name}
                    </a>
                    <div
                      class="text-muted-foreground ml-auto text-right text-sm"
                    >
                      <p>Expires {formatDate(token.exp)}</p>
                      <p>Last used {formatDate(token.last_used)}</p>
                    </div>
                  </li>
                {/each}
              </ul>
            {/if}
          </Card.Content>
        </Card.Root>

        <Card.Root>
          <Card.Header>
            <Card.Title>Recent pushes</Card.Title>
          </Card.Header>
          <Card.Content>
            {@render pathList(
              overview?.recent_pushes,
              'Nothing pushed yet.',
              (path) => formatDate(path.created_at)
            )}
          </Card.Content>
        </Card.Root>

        <Card.Root>
          <Card.Header>
            <Card.Title>Most pulled</Card.Title>
          </Card.Header>
          <Card.Content>
            {@render pathList(
              overview?.most_accessed,
              'Nothing pulled yet.',
              (path) => `${path.accessed}×`
            )}
          </Card.Content>
        </Card.Root>
      </div>
    </div>
  </ScrollArea>
</div>
