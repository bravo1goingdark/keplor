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

<aside class="hidden lg:block w-44 shrink-0">
  <nav class="sticky top-24 space-y-1">
    {#each nav as { href, label }}
      <a
        {href}
        class="block px-3 py-1.5 text-[14px] transition-colors rounded-[4px]
               {page.url.pathname === href
                 ? 'text-ink font-medium'
                 : 'text-ink-muted hover:text-ink'}"
      >
        {label}
      </a>
    {/each}
  </nav>
</aside>
