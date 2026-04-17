<script lang="ts">
  import { page } from '$app/state';

  const links = [
    { href: '/docs', label: 'Docs' },
    { href: '/docs/api-reference', label: 'API' },
    { href: 'https://github.com/bravo1goingdark/keplor', label: 'GitHub', external: true },
  ];
</script>

<nav class="fixed top-0 w-full z-50 border-b border-border bg-bg/85 backdrop-blur-xl">
  <div class="max-w-6xl mx-auto px-6 h-14 flex items-center justify-between">
    <a href="/" class="flex items-center gap-2.5 group">
      <img src="/favicon.svg" alt="" class="w-5 h-5 transition-transform group-hover:rotate-12" />
      <span class="font-semibold tracking-tight text-[15px]">keplor</span>
    </a>
    <div class="flex items-center gap-6 text-[13px]">
      {#each links as { href, label, external }}
        <a
          {href}
          class="text-text-muted hover:text-text transition-colors"
          target={external ? '_blank' : undefined}
          rel={external ? 'noopener' : undefined}
          class:text-text={!external && page.url.pathname.startsWith(href)}
        >
          {label}
        </a>
      {/each}
      <a href="/docs/quickstart" class="px-3.5 py-1.5 bg-accent text-white text-[13px] font-medium rounded-md hover:bg-accent-dim transition-colors">
        Get Started
      </a>
    </div>
  </div>
</nav>
