<script lang="ts">
  import { Button } from '@profidev/pleiades/components/ui/button';
  import Plus from '@lucide/svelte/icons/plus';
  import { toast } from '@profidev/pleiades/components/util/general';
  import { Permission } from '$lib/permissions.svelte.js';
  import { ScrollArea } from '@profidev/pleiades/components/ui/scroll-area';
  import Lock from '@lucide/svelte/icons/lock';
  import LockOpen from '@lucide/svelte/icons/lock-open';
  import { Badge } from '@profidev/pleiades/components/ui/badge';
  import { Progress } from '@profidev/pleiades/components/ui/progress';
  import { size_to_gib } from '$lib/backend/util.svelte.js';
  import type { CacheInfo, UserInfo } from '$lib/client/types.gen.js';
  import { afterNavigate } from '$app/navigation';

  const { data } = $props();

  let user: UserInfo | undefined = $state();
  let caches: CacheInfo[] | undefined = $state();

  $effect(() => {
    data.user.then((u) => (user = u));
  });

  let loadFailed = $state(false);

  $effect(() => {
    data.caches.then((c) => {
      // The load resolves `undefined` when the API call failed.
      loadFailed = !c;
      caches = c ?? [];
    });
  });

  let errorToasted = false;
  afterNavigate(() => {
    if (!data.error) return;

    if (!errorToasted) {
      errorToasted = true;
      if (data.error === 'not_found') {
        toast.error('Cache not found');
      } else if (data.error === 'other') {
        toast.error('Failed to load cache');
      }
    }

    const url = new URL(window.location.href);
    url.searchParams.delete('error');
    window.history.replaceState({}, '', url);
  });
</script>

<div class="p-4">
  <div class="ml-7 flex items-center md:m-0">
    <h3 class="text-xl font-medium">Caches</h3>
    <Button
      class="ml-auto cursor-pointer"
      href="/caches/create"
      disabled={!user?.permissions.includes(Permission.CACHE_CREATE)}
    >
      <Plus />
      Create
    </Button>
  </div>
  <ScrollArea class="mt-2 min-h-0 grow">
    <div
      class="grid size-full auto-rows-min grid-cols-[repeat(auto-fill,minmax(24rem,1fr))] gap-2"
    >
      {#if !caches}
        <p class="text-muted-foreground">Loading caches...</p>
      {:else if loadFailed}
        <p class="text-destructive">Failed to load caches.</p>
      {:else if (caches?.length ?? 0) === 0}
        <p class="text-muted-foreground">
          No caches found. Create one to get started.
        </p>
      {/if}
      {#each caches as cache}
        {@const size_gib = size_to_gib(cache.size ?? 0)}
        {@const quota_gib = size_to_gib(cache.quota)}
        {@const usage_percent = (size_gib / quota_gib) * 100}
        <Button
          class="flex h-24 w-full max-w-150 cursor-pointer flex-col items-start rounded-xl border p-2"
          variant="outline"
          href={'/caches/' + cache.uuid}
        >
          <div class="flex w-full items-center">
            <h4 class="text-xl font-medium">{cache.name}</h4>
            <Badge
              variant="outline"
              class={'ml-auto flex items-center ' +
                (cache.public
                  ? 'border-green-500 text-green-500'
                  : 'border-orange-400 text-orange-400')}
            >
              {#if cache.public}
                <LockOpen />
                Public
              {:else}
                <Lock />
                Private
              {/if}
            </Badge>
          </div>
          <p class="text-muted-foreground mt-2 ml-auto">
            {size_gib.toFixed(2)}GiB / {quota_gib.toFixed(2)}GiB ({usage_percent.toFixed(
              1
            )}%)
          </p>
          <Progress
            min={0}
            max={quota_gib}
            value={size_gib}
            class={(usage_percent > 90
              ? '*:data-[slot=progress-indicator]:bg-red-500'
              : usage_percent > 70
                ? '*:data-[slot=progress-indicator]:bg-yellow-500'
                : '') + ' h-2'}
          />
        </Button>
      {/each}
    </div>
  </ScrollArea>
</div>
