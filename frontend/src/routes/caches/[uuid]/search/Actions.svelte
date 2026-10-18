<script lang="ts">
  import Trash from '@lucide/svelte/icons/trash';
  import { Button } from '@profidev/pleiades/components/ui/button';

  interface Props {
    delete_disabled: boolean;
    remove: () => void;
  }

  let { delete_disabled, remove }: Props = $props();
</script>

<div class="flex w-full space-x-4">
  <Button
    size="icon"
    class="cursor-pointer"
    variant="destructive"
    aria-label="Delete store path"
    onclick={remove}
    disabled={delete_disabled}
  >
    <Trash />
  </Button>
</div>
