<script lang="ts">
  import { Button } from '@profidev/pleiades/components/ui/button';
  import FormDialog from '@profidev/pleiades/components/form/form-dialog.svelte';
  import Plus from '@lucide/svelte/icons/plus';
  import Trash from '@lucide/svelte/icons/trash';
  import Table from '@profidev/pleiades/components/table/clean-table.svelte';
  import { columns } from './table.svelte';
  import { z } from 'zod';
  import { toast } from '@profidev/pleiades/components/util/general';
  import { afterNavigate, invalidate } from '$app/navigation';
  import {
    deleteExpiredTokens,
    deleteToken,
    type TokenInfo
  } from '$lib/client';

  const { data } = $props();

  let tokens: TokenInfo[] = $state([]);
  let selected: TokenInfo | undefined = $state();
  let deleteOpen = $state(false);
  let deleteExpiredOpen = $state(false);
  let isLoading = $state(false);
  let expiredCount = $derived(
    tokens.filter((token) => token.exp < new Date()).length
  );

  $effect(() => {
    data.tokens.then((items) => {
      tokens = items ?? [];
    });
  });

  let errorToasted = false;
  afterNavigate(() => {
    if (!data.error) return;

    if (!errorToasted) {
      errorToasted = true;
      if (data.error === 'not_found') {
        toast.error('Token not found');
      } else if (data.error === 'other') {
        toast.error('Failed to load token');
      }
    }

    const url = new URL(window.location.href);
    url.searchParams.delete('error');
    window.history.replaceState({}, '', url);
  });

  const deleteItemConfirm = async () => {
    if (!selected) return;

    isLoading = true;
    let ret = await deleteToken({
      body: {
        uuid: selected.uuid
      }
    });
    isLoading = false;

    if (ret.error) {
      return { error: 'Failed to delete token' };
    } else {
      toast.success(`Token ${selected.name} deleted successfully`);
      invalidate('/api/token');
    }
  };

  const deleteExpiredConfirm = async () => {
    isLoading = true;
    let ret = await deleteExpiredTokens();
    isLoading = false;

    if (ret.error) {
      return { error: 'Failed to delete expired tokens' };
    }

    invalidate('/api/token');
    const deleted = ret.data?.deleted ?? 0;
    if (deleted === 0) {
      toast.success('No expired tokens to delete');
    } else {
      toast.success(`Deleted ${deleted} expired token(s)`);
    }
  };

  const startDeleteToken = (item: TokenInfo) => {
    selected = item;
    deleteOpen = true;
  };
</script>

<div class="p-4">
  <div class="ml-7 flex items-center gap-2 md:m-0">
    <h3 class="text-xl font-medium">Token</h3>
    <Button
      class="ml-auto cursor-pointer"
      variant="destructive"
      disabled={expiredCount === 0}
      onclick={() => (deleteExpiredOpen = true)}
    >
      <Trash />
      Delete Expired
    </Button>
    <Button class="cursor-pointer" href="/tokens/create">
      <Plus />
      Create
    </Button>
  </div>
  <Table
    data={data.tokens}
    {columns}
    class="mt-4"
    columnData={{
      deleteToken: startDeleteToken
    }}
    searchColumns={['name', 'uuid']}
  />
</div>
<FormDialog
  title="Delete Token"
  description={`Do you really want to delete the token ${selected?.name}?`}
  confirm="Delete"
  confirmVariant="destructive"
  onsubmit={deleteItemConfirm}
  bind:open={deleteOpen}
  bind:isLoading
  schema={z.object({})}
/>
<FormDialog
  title="Delete Expired Tokens"
  description={`Permanently delete ${expiredCount} expired token(s)?`}
  confirm="Delete"
  confirmVariant="destructive"
  onsubmit={deleteExpiredConfirm}
  bind:open={deleteExpiredOpen}
  bind:isLoading
  schema={z.object({})}
/>
