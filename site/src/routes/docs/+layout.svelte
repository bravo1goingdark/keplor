<script lang="ts">
  import DocsSidebar from '$lib/components/DocsSidebar.svelte';
  let { children } = $props();
</script>

<div class="max-w-6xl mx-auto px-6 pt-10 pb-16 flex gap-12">
  <DocsSidebar />
  <article class="flex-1 min-w-0 prose-custom">
    {@render children()}
  </article>
</div>

<style>
  :global(.prose-custom h2) {
    font-size: 1.375rem;
    font-weight: 600;
    margin-top: 2.5rem;
    margin-bottom: 0.75rem;
    padding-bottom: 0.5rem;
    border-bottom: 1px solid var(--color-border);
  }
  :global(.prose-custom h3) {
    font-size: 1.05rem;
    font-weight: 600;
    margin-top: 1.75rem;
    margin-bottom: 0.5rem;
  }
  :global(.prose-custom p) {
    color: #9a9a9a;
    line-height: 1.75;
    margin-bottom: 0.875rem;
    font-size: 0.9375rem;
  }
  :global(.prose-custom a) {
    color: var(--color-accent);
    text-decoration: none;
  }
  :global(.prose-custom a:hover) {
    text-decoration: underline;
  }
  :global(.prose-custom pre) {
    background: var(--color-bg-code);
    border: 1px solid var(--color-border);
    border-radius: 0.5rem;
    padding: 1.125rem;
    overflow-x: auto;
    font-size: 0.8125rem;
    line-height: 1.75;
    font-family: var(--font-mono);
    margin: 0.75rem 0 1.25rem;
  }
  :global(.prose-custom code) {
    background: var(--color-bg-code);
    padding: 0.1em 0.35em;
    border-radius: 4px;
    font-size: 0.85em;
    border: 1px solid var(--color-border);
    font-family: var(--font-mono);
  }
  :global(.prose-custom pre code) {
    background: none;
    padding: 0;
    border: none;
    font-size: inherit;
  }
  :global(.prose-custom table) {
    width: 100%;
    border-collapse: collapse;
    margin: 0.75rem 0 1.25rem;
    font-size: 0.8125rem;
  }
  :global(.prose-custom th) {
    text-align: left;
    padding: 0.625rem 0.875rem;
    border-bottom: 1px solid var(--color-border);
    color: var(--color-text-muted);
    font-weight: 500;
    font-size: 0.6875rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  :global(.prose-custom td) {
    padding: 0.625rem 0.875rem;
    border-bottom: 1px solid var(--color-border);
    color: #9a9a9a;
  }
  :global(.prose-custom .method) {
    display: inline-block;
    padding: 0.1rem 0.4rem;
    border-radius: 3px;
    font-size: 0.6875rem;
    font-weight: 600;
    font-family: var(--font-mono);
  }
  :global(.prose-custom .method-post) { background: #065f46; color: #6ee7b7; }
  :global(.prose-custom .method-get) { background: #1e3a5f; color: #7dd3fc; }
</style>
