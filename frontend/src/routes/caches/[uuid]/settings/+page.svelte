<script lang="ts">
  import { Button } from '@profidev/pleiades/components/ui/button';
  import BaseForm from '@profidev/pleiades/components/form/base-form.svelte';
  import { cacheSchema, keySchema, quotaSchema } from './schema.svelte.js';
  import FormInput from '@profidev/pleiades/components/form/form-input.svelte';
  import { Spinner } from '@profidev/pleiades/components/ui/spinner';
  import Save from '@lucide/svelte/icons/save';
  import FormSwitch from '@profidev/pleiades/components/form/form-switch.svelte';
  import type { FormValue } from '@profidev/pleiades/components/form/types';
  import { toast } from '@profidev/pleiades/components/util/general';
  import FormDialog from '@profidev/pleiades/components/form/form-dialog.svelte';
  import { z } from 'zod';
  import Trash from '@lucide/svelte/icons/trash';
  import Ban from '@lucide/svelte/icons/ban';
  import KeyRound from '@lucide/svelte/icons/key-round';
  import HardDrive from '@lucide/svelte/icons/hard-drive';
  import Lock from '@lucide/svelte/icons/lock';
  import LockOpen from '@lucide/svelte/icons/lock-open';
  import { goto } from '$app/navigation';
  import FormSelect from '@profidev/pleiades/components/form/form-select.svelte';
  import { TagsInput } from '@profidev/pleiades/components/ui-extra/tags-input';
  import { Label } from '@profidev/pleiades/components/ui/label';
  import { clearCache, deleteCache, editCache } from '$lib/client';
  import { EvictionPolicy, type CacheDetails } from '$lib/client/types.gen.js';
  import RotateCCW from '@lucide/svelte/icons/rotate-ccw';

  const { data } = $props();

  let cacheInfo: CacheDetails | undefined = $state();
  let form: BaseForm<typeof cacheSchema> | undefined = $state();

  $effect(() => {
    data.cacheRes.then((res) => {
      if (!res.data) return;

      cacheInfo = res.data;
      form?.setValue({
        ...res.data,
        eviction_policy: [
          res.data?.eviction_policy ?? EvictionPolicy.OLDEST_FIRST
        ]
      });
    });
  });

  let downstreamCaches = $derived(cacheInfo?.downstream_caches ?? []);
  let readonly = $derived(!cacheInfo?.has_write_access || !cacheInfo);
  let deleteOpen = $state(false);
  let clearOpen = $state(false);
  let keyOpen = $state(false);
  let quotaOpen = $state(false);
  let visibilityOpen = $state(false);

  const onsubmit = async (form: FormValue<typeof cacheSchema>) => {
    if (!cacheInfo) return;
    let res = await editCache({
      path: { uuid: cacheInfo.uuid },
      body: {
        ...cacheInfo,
        ...form,
        eviction_policy: form.eviction_policy[0],
        downstream_caches: downstreamCaches
      }
    });

    if (res.error) {
      if (res.response?.status === 409) {
        return {
          error: 'This cache name is already in use',
          field: 'name'
        } as const;
      } else {
        return { error: 'Failed to update cache' };
      }
    } else {
      toast.success(`Cache ${cacheInfo.name} updated successfully`);
      return { error: '' };
    }
  };

  const deleteConfirm = async () => {
    if (!cacheInfo) return;
    let res = await deleteCache({ body: { uuid: cacheInfo.uuid } });

    if (res.error) {
      return { error: 'Failed to delete cache' };
    } else {
      toast.success(`Cache ${cacheInfo.name} deleted successfully`);
      setTimeout(() => {
        goto('/caches');
      });
    }
  };

  const clearConfirm = async () => {
    if (!cacheInfo) return;
    let res = await clearCache({ path: { uuid: cacheInfo.uuid } });

    if (res.error) {
      return { error: 'Failed to clear cache' };
    } else {
      toast.success(`Cache ${cacheInfo.name} cleared successfully`);
    }
  };

  const visibilityConfirm = async () => {
    if (!cacheInfo) return;
    let res = await editCache({
      path: { uuid: cacheInfo.uuid },
      body: {
        ...cacheInfo,
        public: !cacheInfo.public
      }
    });

    if (res.error) {
      toast.error(`Failed to change cache visibility for ${cacheInfo.name}`);
    } else {
      toast.success(
        `Cache ${cacheInfo.name} is now ${cacheInfo.public ? 'private' : 'public'}`
      );
    }
  };

  const quotaConfirm = async (form: FormValue<typeof quotaSchema>) => {
    if (!cacheInfo) return;
    let res = await editCache({
      path: { uuid: cacheInfo.uuid },
      body: {
        ...cacheInfo,
        quota: form.quota * 1024 * 1024
      }
    });

    if (res.error) {
      return { error: 'Failed to change cache quota' };
    } else {
      toast.success(`Cache ${cacheInfo.name} quota updated successfully`);
    }
  };

  const keyConfirm = async (form: FormValue<typeof keySchema>) => {
    if (!cacheInfo) return;
    let res = await editCache({
      path: { uuid: cacheInfo.uuid },
      body: {
        ...cacheInfo,
        sig_key: form.sig_key
      }
    });

    if (res.error) {
      if (res.response?.status === 406) {
        return {
          error: 'The provided key is not a valid public signing key',
          field: 'sig_key'
        } as const;
      } else {
        return { error: 'Failed to change cache key' };
      }
    } else {
      toast.success(`Cache ${cacheInfo.name} key updated successfully`);
    }
  };
</script>

<div class="grow">
  <BaseForm
    schema={cacheSchema}
    {onsubmit}
    class="flex flex-1 flex-col"
    bind:this={form}
  >
    {#snippet children({ props })}
      <div class="grid grid-cols-1 gap-4 xl:grid-cols-[1fr_auto_1fr]">
        <div>
          <FormInput
            {...props}
            key="name"
            label="Cache Name"
            placeholder="Enter cache name"
            {readonly}
          />
          <FormInput
            {...props}
            key="priority"
            label="Priority"
            placeholder="Enter priority"
            type="number"
            {readonly}
          />
          <FormSelect
            {...props}
            disabled={readonly || props.disabled}
            key="eviction_policy"
            label="Eviction Policy"
            single
            data={[
              { value: EvictionPolicy.OLDEST_FIRST, label: 'Oldest First' },
              {
                value: EvictionPolicy.LEAST_RECENTLY_USED,
                label: 'Least Recently Used'
              },
              {
                value: EvictionPolicy.LEAST_FREQUENTLY_USED,
                label: 'Least Frequently Used'
              }
            ]}
          />
          <Label class="mb-2">Downstream Caches</Label>
          <TagsInput
            placeholder="Add a downstream cache"
            disabled={readonly}
            bind:value={downstreamCaches}
            suggestions={[
              'https://cache.nixos.org/',
              'https://nix-community.cachix.org/',
              'https://cache.garnix.io/'
            ]}
            validate={(val) => {
              let res = z.url().safeParse(val);
              if (!res.success) return undefined;
              return val;
            }}
          />
          <FormSwitch
            disabled={readonly || props.disabled}
            formData={props.formData}
            key="allow_force_push"
            label="Allow Force Push"
          />
        </div>
      </div>
    {/snippet}
    {#snippet footer({
      isLoading,
      isError
    }: {
      isLoading: boolean;
      isError: boolean;
    })}
      <div class="mt-4 grid w-full grid-cols-1 gap-8 xl:grid-cols-2">
        <Button
          class="ml-auto cursor-pointer"
          type="submit"
          disabled={isLoading || readonly}
        >
          {#if isLoading}
            <Spinner />
          {:else if isError}
            <RotateCCW class="mr-2 h-4 w-4" />
          {:else}
            <Save />
          {/if}
          Save Changes</Button
        >
      </div>
    {/snippet}
  </BaseForm>
  <p class="text-destructive mt-8">Danger Zone</p>
  <div class="grid grid-cols-1 gap-4 xl:grid-cols-[1fr_auto_1fr]">
    <div
      class="border-destructive/50 bg-destructive/10 mt-4 flex w-full flex-col items-start gap-4 rounded-md border p-4"
    >
      <div class="flex w-full items-center gap-4">
        <p>Change cache visibility</p>
        <Button
          class="ml-auto cursor-pointer"
          variant="destructive"
          onclick={() => (visibilityOpen = true)}
          disabled={readonly}
        >
          {#if cacheInfo?.public}
            <Lock />
            Make Private
          {:else}
            <LockOpen />
            Make Public
          {/if}
        </Button>
      </div>
      <div class="flex w-full items-center gap-4">
        <p>Change the cache quota</p>
        <Button
          class="ml-auto cursor-pointer"
          variant="destructive"
          onclick={() => (quotaOpen = true)}
          disabled={readonly}
        >
          <HardDrive />
          Change Quota
        </Button>
      </div>
      <div class="flex w-full items-center gap-4">
        <p>Change public signing key</p>
        <Button
          class="ml-auto cursor-pointer"
          variant="destructive"
          onclick={() => (keyOpen = true)}
          disabled={readonly}
        >
          <KeyRound />
          Change Key
        </Button>
      </div>
      <div class="flex w-full items-center gap-4">
        <p>Clear all data from the cache.</p>
        <Button
          class="ml-auto cursor-pointer"
          variant="destructive"
          onclick={() => (clearOpen = true)}
          disabled={readonly}
        >
          <Ban />
          Clear Cache
        </Button>
      </div>
      <div class="flex w-full items-center gap-4">
        <p>Delete the cache and all its data</p>
        <Button
          class="ml-auto cursor-pointer"
          variant="destructive"
          onclick={() => (deleteOpen = true)}
          disabled={readonly}
        >
          <Trash />
          Delete Cache
        </Button>
      </div>
    </div>
  </div>
</div>
<FormDialog
  title={`Change Cache Visibility`}
  description={`Do you really want to make the cache ${cacheInfo?.name} ${cacheInfo?.public ? 'private' : 'public'}? This might have implications on who can access the cache and its data.`}
  confirm={cacheInfo?.public ? 'Make Private' : 'Make Public'}
  confirmVariant="destructive"
  onsubmit={visibilityConfirm}
  bind:open={visibilityOpen}
  schema={z.object({})}
/>
<FormDialog
  title={`Change Quota`}
  description={`Do you really want to change the quota of cache ${cacheInfo?.name}? This can lead to the deletion of cache entries if the new quota is smaller than the current usage.`}
  confirm="Change"
  confirmVariant="destructive"
  initialValue={{ quota: (cacheInfo?.quota ?? 0) / (1024 * 1024) }}
  onsubmit={quotaConfirm}
  bind:open={quotaOpen}
  schema={quotaSchema}
>
  {#snippet children({ props })}
    <FormInput
      {...props}
      key="quota"
      label="Quota (in MiB)"
      placeholder="Enter new cache quota in MiB"
      type="number"
    />
  {/snippet}
</FormDialog>
<FormDialog
  title={`Change Key`}
  description={`Do you really want to change the key of cache ${cacheInfo?.name}? This prevents all pushes using the old key.`}
  confirm="Change"
  confirmVariant="destructive"
  onsubmit={keyConfirm}
  bind:open={keyOpen}
  schema={keySchema}
>
  {#snippet children({ props })}
    <FormInput
      {...props}
      key="sig_key"
      label="Public Signing Key"
      placeholder="Enter new public signing key"
    />
  {/snippet}
</FormDialog>
<FormDialog
  title={`Clear Cache`}
  description={`Do you really want to clear the cache ${cacheInfo?.name}? This action cannot be undone.`}
  confirm="Clear"
  confirmVariant="destructive"
  onsubmit={clearConfirm}
  bind:open={clearOpen}
  schema={z.object({})}
/>
<FormDialog
  title={`Delete Cache`}
  description={`Do you really want to delete the cache ${cacheInfo?.name}? This action cannot be undone.`}
  confirm="Delete"
  confirmVariant="destructive"
  onsubmit={deleteConfirm}
  bind:open={deleteOpen}
  schema={z.object({})}
/>
