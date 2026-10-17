<script lang="ts">
  import { Input } from '@profidev/pleiades/components/ui/input';
  import { toast } from '@profidev/pleiades/components/util/general';
  import * as Select from '@profidev/pleiades/components/ui/select';
  import Table from '@profidev/pleiades/components/table/clean-table.svelte';
  import { columns } from './table.svelte';
  import {
    deletePath,
    SearchOrder,
    SearchSort,
    searchStorePaths,
    type CacheDetails,
    type SearchResult
  } from '$lib/client';

  let { data } = $props();

  let input = $state('');
  let sort: SearchSort = $state(SearchSort.STORE_PATH);
  let order: SearchOrder = $state(SearchOrder.ASC);
  let paths = $state<SearchResult[]>([]);
  let searchTrigger = $state(0);
  let cacheInfo: CacheDetails | undefined = $state();

  $effect(() => {
    data.cacheRes.then((res) => {
      if (!res.data) return;
      cacheInfo = res.data;
    });
  });

  $effect(() => {
    let _ = searchTrigger;
    if (!cacheInfo) return;
    searchStorePaths({
      path: { uuid: cacheInfo.uuid },
      body: {
        order,
        query: input,
        sort
      }
    }).then((result) => {
      if (!result.data) {
        toast.error('Failed to search cache');
        return;
      }
      paths = result.data;
    });
  });

  const delete_path = async (path: string) => {
    if (!cacheInfo) return;
    let res = await deletePath({
      path: { uuid: cacheInfo.uuid },
      body: { store_path: path }
    });

    if (res.error) {
      toast.error('Failed to delete path');
    } else {
      searchTrigger += 1;
    }
  };
</script>

<div class="flex w-full grow flex-col">
  <div class="flex w-full flex-none gap-2">
    <Input placeholder="Search..." bind:value={input} class="mb-1" />
    <Select.Root bind:value={sort} type="single" allowDeselect={false}>
      <Select.Trigger class="w-42">
        {sort}
      </Select.Trigger>
      <Select.Content>
        <Select.Group>
          {#each Object.values(SearchSort) as value}
            <Select.Item {value}>{value}</Select.Item>
          {/each}
        </Select.Group>
      </Select.Content>
    </Select.Root>
    <Select.Root bind:value={order} type="single" allowDeselect={false}>
      <Select.Trigger class="w-24">
        {order}
      </Select.Trigger>
      <Select.Content>
        <Select.Group>
          {#each Object.values(SearchOrder) as value}
            <Select.Item {value}>{value}</Select.Item>
          {/each}
        </Select.Group>
      </Select.Content>
    </Select.Root>
  </div>
  <Table
    data={paths}
    {columns}
    columnData={{
      write_access: cacheInfo?.has_write_access || false,
      delete_path
    }}
    class="mt-2 min-h-0 grow"
  />
</div>
