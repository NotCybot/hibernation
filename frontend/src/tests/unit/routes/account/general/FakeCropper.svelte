<script lang="ts">
  import type { Snippet } from 'svelte';

  // Test double for ImageCropper.Root: exposes its callbacks as buttons.
  let {
    src,
    onCropped,
    onUnsupportedFile,
    children
  }: {
    src?: string;
    onCropped: (url: string) => void;
    onUnsupportedFile: (file: File) => void;
    children?: Snippet;
  } = $props();
</script>

<img alt="avatar" {src} />
<button type="button" onclick={() => onCropped('blob:cropped')}>crop</button>
<button
  type="button"
  onclick={() =>
    onUnsupportedFile(new File([''], 'x.gif', { type: 'image/gif' }))}
>
  unsupported
</button>
{@render children?.()}
