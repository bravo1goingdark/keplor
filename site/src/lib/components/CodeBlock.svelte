<script lang="ts">
  let { code, raw = '' }: { code: string; raw?: string } = $props();
  let copied = $state(false);

  function copy() {
    navigator.clipboard.writeText(raw || code.replace(/<[^>]*>/g, ''));
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }
</script>

<div class="relative group my-5">
  <pre class="bg-bg-alt border border-line rounded-[4px] p-6 overflow-x-auto text-[14px] leading-[1.75] font-mono text-ink-muted"><code>{@html code}</code></pre>
  <button
    onclick={copy}
    class="absolute top-4 right-4 text-[12px] text-ink-muted opacity-0 group-hover:opacity-100 transition-opacity hover:text-ink cursor-pointer"
  >
    {copied ? 'copied' : 'copy'}
  </button>
</div>
