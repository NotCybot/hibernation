<script lang="ts">
  import { Progress } from '@profidev/pleiades/components/ui/progress';
  import * as Code from '$lib/components/code';
  import { ScrollArea } from '@profidev/pleiades/components/ui/scroll-area';
  import { size_to_gib } from '$lib/backend/util.svelte.js';
  import type { CacheDetails, GeneralSettings } from '$lib/client/types.gen.js';
  import { page } from '$app/state';

  let { data } = $props();

  let cacheInfo: CacheDetails | undefined = $state();
  // `undefined` while loading, `null` when the settings could not be loaded.
  let generalSettings: GeneralSettings | null | undefined = $state();

  $effect(() => {
    data.cacheRes.then((res) => {
      cacheInfo = res.data;
    });
  });

  $effect(() => {
    data.generalSettings.then((res) => {
      generalSettings = res ?? null;
    });
  });

  let size_gib = $derived(size_to_gib(cacheInfo?.size ?? 0));
  let quota_gib = $derived(size_to_gib(cacheInfo?.quota ?? 0));
  let usage_percent = $derived((size_gib / quota_gib) * 100);

  const url = $derived.by(() => {
    if (!cacheInfo || generalSettings === undefined) return 'Loading...';
    // Fall back to the current origin if the settings failed to load.
    let siteUrl = new URL(generalSettings?.site_url || page.url.origin);
    if (generalSettings?.virtual_host_routing) {
      // The host router maps `<cache>.<host>/<path>` to `/api/nix/<cache>/<path>`.
      siteUrl.host = `${cacheInfo.name}.${siteUrl.host}`;
      return siteUrl.origin;
    }
    // Resolve relative to site_url so a base path (e.g. a proxy prefix) is kept.
    if (!siteUrl.pathname.endsWith('/')) {
      siteUrl.pathname += '/';
    }
    return new URL(`api/nix/${cacheInfo.name}`, siteUrl).href;
  });
  const nixConfig = $derived(`{
  nixConfig = {
    extra-substituters = [
      "${url}"
    ];
    extra-trusted-public-keys = [
      "${cacheInfo?.sig_key}"
    ];
  };

  ...
}`);
</script>

<ScrollArea class="h-full w-full">
  <div class="grid w-full grid-cols-1 gap-4 2xl:grid-cols-[1fr_auto_1fr]">
    <div class="flex min-w-0 flex-col">
      <p class="text-lg">Usage:</p>
      <div class="mt-2 mb-1 flex">
        <p>Number of paths:</p>
        <p class="text-muted-foreground ml-auto">
          {cacheInfo?.nar_count}
        </p>
      </div>
      <div class="mb-1 flex">
        <p>Cache size:</p>
        <p class="text-muted-foreground ml-auto">
          {size_gib.toFixed(2)}GiB / {quota_gib.toFixed(2)}GiB ({usage_percent.toFixed(
            1
          )}%)
        </p>
      </div>
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
      <p class="mt-8 text-lg">Pushing paths:</p>
      <Code.Root
        code={`hibernation push ${cacheInfo?.name} /nix/store/...`}
        lang="bash"
        class="mt-2 h-auto"
        hideLines
      >
        <Code.CopyButton />
      </Code.Root>
      <p class="mt-8 text-lg">Pulling paths:</p>
      <p class="text-md mt-2">Cache URL:</p>
      <Code.Root code={url} lang="nix" class="mt-2 h-auto" hideLines>
        <Code.CopyButton />
      </Code.Root>
      <p class="text-md mt-2">Cache Public Key:</p>
      <Code.Root
        code={cacheInfo?.sig_key ?? 'Loading...'}
        lang="nix"
        hideLines
        class="mt-2 h-auto"
      >
        <Code.CopyButton />
      </Code.Root>
      <p class="text-md mt-2">Nix config:</p>
      <Code.Root code={nixConfig} lang="nix" class="mt-2 h-auto">
        <Code.CopyButton />
      </Code.Root>
    </div>
  </div>
</ScrollArea>
