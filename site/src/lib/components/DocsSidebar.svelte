<script lang="ts">
  import { page } from '$app/state';

  const nav = [
    { href: '/docs', label: 'Overview' },
    { href: '/docs/quickstart', label: 'Quickstart' },
    { href: '/docs/api-reference', label: 'API Reference' },
    { href: '/docs/configuration', label: 'Configuration' },
    { href: '/docs/cli', label: 'CLI' },
  ];
</script>

<aside class="hidden lg:block w-52 shrink-0">
  <nav class="sticky top-20 space-y-0.5">
    {#each nav as { href, label }}
      <a
        {href}
        class="block px-3 py-1.5 rounded-md text-[13px] transition-colors
               {page.url.pathname === href
                 ? 'text-text bg-bg-raised'
                 : 'text-text-muted hover:text-text hover:bg-bg-raised/50'}"
      >
        {label}
      </a>
    {/each}
  </nav>
</aside>
