<script lang="ts">
  import { base } from '$app/paths';
  import { page } from '$app/state';
  import ThemeToggle from './ThemeToggle.svelte';

  let scrolled = $state(false);

  $effect(() => {
    const handler = () => { scrolled = window.scrollY > 10; };
    window.addEventListener('scroll', handler, { passive: true });
    return () => window.removeEventListener('scroll', handler);
  });
</script>

<nav
  class="fixed top-0 w-full z-50 transition-all duration-300"
  class:bg-bg={scrolled}
  class:border-b={scrolled}
  class:border-line={scrolled}
>
  <div class="max-w-[1280px] mx-auto px-6 md:px-20 h-16 flex items-center justify-between">
    <a href="{base}/" class="font-serif text-[17px]">keplor</a>
    <div class="flex items-center gap-8 text-[15px]">
      <a
        href="{base}/docs"
        class="transition-colors {page.url.pathname.startsWith(`${base}/docs`) ? 'text-ink' : 'text-ink-muted hover:text-ink'}"
      >
        Docs
      </a>
      <a href="https://github.com/bravo1goingdark/keplor" target="_blank" rel="noopener" class="text-ink-muted hover:text-ink transition-colors">
        GitHub
      </a>
      <ThemeToggle />
      <a href="{base}/docs/quickstart" class="px-[22px] py-[12px] bg-accent text-accent-ink text-[15px] font-medium rounded-[6px] hover:-translate-y-px transition-transform">
        Get started
      </a>
    </div>
  </div>
</nav>
