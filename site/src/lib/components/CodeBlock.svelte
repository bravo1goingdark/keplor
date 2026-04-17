<script lang="ts">
  let { code, lang = '' }: { code: string; lang?: string } = $props();
  let copied = $state(false);

  function copy() {
    navigator.clipboard.writeText(code);
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }
</script>

<div class="relative group">
  <pre class="bg-bg-code border border-border rounded-lg p-5 overflow-x-auto text-[13px] leading-7 font-mono"><code>{@html code}</code></pre>
  <button
    onclick={copy}
    class="absolute top-3 right-3 px-2 py-1 text-[11px] font-mono rounded bg-border/50 text-text-muted opacity-0 group-hover:opacity-100 transition-opacity hover:text-text cursor-pointer"
  >
    {copied ? 'copied' : 'copy'}
  </button>
</div>
