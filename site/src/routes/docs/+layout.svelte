<script lang="ts">
  import DocsSidebar from '$lib/components/DocsSidebar.svelte';
  let { children } = $props();
</script>

<div class="max-w-[1280px] mx-auto px-6 md:px-20 flex gap-16 h-[calc(100vh-3.5rem)] overflow-hidden">
  <DocsSidebar />
  <article class="flex-1 min-w-0 prose-custom overflow-y-auto pt-12 pb-20 no-scrollbar">
    {@render children()}
  </article>
</div>

<style>
  :global(.prose-custom h1) {
    font-family: var(--font-serif);
    font-size: 2rem;
    font-weight: 400;
    letter-spacing: -0.02em;
    margin-bottom: 0.5rem;
  }
  :global(.prose-custom h2) {
    font-size: 1.125rem;
    font-weight: 500;
    letter-spacing: -0.01em;
    margin-top: 3rem;
    margin-bottom: 0.875rem;
    padding-bottom: 0.625rem;
    border-bottom: 1px solid var(--color-line);
  }
  :global(.prose-custom h3) {
    font-size: 0.9375rem;
    font-weight: 500;
    margin-top: 1.75rem;
    margin-bottom: 0.5rem;
  }
  :global(.prose-custom p) {
    color: var(--color-ink-muted);
    line-height: 1.7;
    margin-bottom: 1rem;
    font-size: 0.9375rem;
    max-width: 58ch;
  }
  :global(.prose-custom a) {
    color: var(--color-accent);
    text-decoration: none;
  }
  :global(.prose-custom a:hover) {
    text-decoration: underline;
  }
  :global(.prose-custom pre) {
    background: var(--color-bg-alt);
    border: 1px solid var(--color-line);
    border-radius: 4px;
    padding: 1.25rem 1.5rem;
    overflow-x: auto;
    font-size: 0.8125rem;
    line-height: 1.8;
    font-family: var(--font-mono);
    margin: 0.75rem 0 1.5rem;
    color: var(--color-ink-muted);
  }
  :global(.prose-custom code) {
    background: var(--color-bg-alt);
    padding: 0.15em 0.4em;
    border-radius: 3px;
    font-size: 0.825em;
    font-family: var(--font-mono);
    color: var(--color-ink);
  }
  :global(.prose-custom pre code) {
    background: none;
    padding: 0;
    font-size: inherit;
    color: inherit;
  }
  :global(.prose-custom table) {
    width: 100%;
    border-collapse: collapse;
    margin: 0.75rem 0 1.5rem;
    font-size: 0.8125rem;
  }
  :global(.prose-custom th) {
    text-align: left;
    padding: 0.625rem 0.875rem;
    color: var(--color-ink-muted);
    font-weight: 500;
    font-size: 0.6875rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    border-bottom: 1px solid var(--color-line);
  }
  :global(.prose-custom td) {
    padding: 0.625rem 0.875rem;
    color: var(--color-ink-muted);
    border-bottom: 1px solid var(--color-line);
  }
  :global(.prose-custom .method) {
    display: inline-block;
    padding: 0.1rem 0.45rem;
    border-radius: 3px;
    font-size: 0.625rem;
    font-weight: 600;
    font-family: var(--font-mono);
    letter-spacing: 0.03em;
  }
  :global(.prose-custom .method-post) { background: #e8f5e9; color: #2e7d32; }
  :global(.prose-custom .method-get) { background: #e3f2fd; color: #1565c0; }
  :global(html.dark .prose-custom .method-post) { background: #1b3a20; color: #66bb6a; }
  :global(html.dark .prose-custom .method-get) { background: #152a3f; color: #64b5f6; }
</style>
