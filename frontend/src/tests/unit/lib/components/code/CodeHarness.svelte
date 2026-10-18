<script lang="ts">
  import * as Code from '$lib/components/code';
  import type { RootProps } from '$lib/components/code';

  let { overflow = false, ...props }: RootProps & { overflow?: boolean } =
    $props();
</script>

<!-- Test-only host composing Root + CopyButton (optionally inside Overflow). -->
{#if overflow}
  <Code.Overflow>
    <Code.Root {...props}><Code.CopyButton /></Code.Root>
  </Code.Overflow>
{:else}
  <Code.Root {...props}><Code.CopyButton /></Code.Root>
{/if}
