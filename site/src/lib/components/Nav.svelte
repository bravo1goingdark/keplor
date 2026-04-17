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
  class="fixed top-0 w-full z-50 transition-all duration-300 {scrolled ? 'bg-bg/80 backdrop-blur-md border-b border-line' : ''}"
>
  <div class="max-w-[1280px] mx-auto px-6 md:px-20 h-16 flex items-center justify-between">
    <a href="{base}/" class="flex items-center gap-2.5 hover:opacity-80 transition-opacity">
      <!-- Prism K mark -->
      <svg class="w-6 h-6" viewBox="0 0 64 64" fill="none">
        <line x1="14" y1="9" x2="14" y2="55" stroke="currentColor" stroke-width="3.7" stroke-linecap="round"/>
        <line x1="16.6" y1="32" x2="50" y2="9" stroke="currentColor" stroke-width="3.7" stroke-linecap="round"/>
        <line x1="17.3" y1="37.1" x2="50" y2="17.3" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" opacity="0.28"/>
        <line x1="16.6" y1="32" x2="50" y2="55" stroke="currentColor" stroke-width="3.7" stroke-linecap="round"/>
        <line x1="17.3" y1="26.9" x2="50" y2="46.7" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" opacity="0.28"/>
        <circle cx="16.6" cy="32" r="3.2" class="fill-accent"/>
      </svg>
      <span class="text-[17px] font-semibold tracking-[-0.02em]" style="font-family: 'Space Grotesk', system-ui, sans-serif">
        kepl<span class="text-accent">o</span>r
      </span>
    </a>
    <div class="flex items-center gap-6 md:gap-8 text-[15px]">
      <a
        href="{base}/docs"
        class="hidden sm:block transition-colors {page.url.pathname.startsWith(`${base}/docs`) ? 'text-ink' : 'text-ink-muted hover:text-ink'}"
      >
        Docs
      </a>
      <a href="https://github.com/bravo1goingdark/keplor" target="_blank" rel="noopener" class="text-ink-muted hover:text-ink transition-colors" aria-label="GitHub">
        <svg class="w-5 h-5" fill="currentColor" viewBox="0 0 24 24"><path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405s2.04.135 3 .405c2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.3 0 .315.225.69.825.57A12.02 12.02 0 0024 12c0-6.63-5.37-12-12-12z"/></svg>
      </a>
      <ThemeToggle />
      <a href="{base}/docs/quickstart" class="px-5 py-2.5 bg-accent text-accent-ink text-[14px] font-medium rounded-[6px] hover:-translate-y-px hover:shadow-[0_4px_12px_rgba(255,91,46,0.2)] transition-all duration-200">
        Get started
      </a>
    </div>
  </div>
</nav>
