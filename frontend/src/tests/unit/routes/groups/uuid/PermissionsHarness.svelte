<script lang="ts">
  import BaseForm from '@profidev/pleiades/components/form/base-form.svelte';
  import Permissions from '$routes/groups/[uuid]/Permissions.svelte';
  import { groupSettings } from '$routes/groups/[uuid]/schema.svelte';
  import type { UserInfo } from '$lib/client';

  interface Props {
    user?: UserInfo;
    readonly: boolean;
    onsubmit: (value: unknown) => void;
  }

  let { user, readonly, onsubmit }: Props = $props();
</script>

<!-- Test-only host: Permissions needs a live superform from BaseForm. -->
<BaseForm
  schema={groupSettings}
  initialValue={{ name: 'g', users: [] }}
  {onsubmit}
>
  {#snippet children({ props })}
    <Permissions {...props} {user} {readonly} />
  {/snippet}
</BaseForm>
