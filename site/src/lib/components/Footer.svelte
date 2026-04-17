<script lang="ts">
  import { base } from '$app/paths';
</script>

<footer class="border-t border-line">
  <div class="max-w-[1280px] mx-auto px-6 md:px-20 py-16">
    <div class="grid grid-cols-2 md:grid-cols-4 gap-10 mb-20">
      <div>
        <p class="text-[12px] uppercase tracking-[0.12em] text-ink-muted mb-4">Product</p>
        <div class="space-y-2.5 text-[14px]">
          <a href="{base}/docs/quickstart" class="block text-ink-muted hover:text-ink transition-colors">Quickstart</a>
          <a href="{base}/docs/integration" class="block text-ink-muted hover:text-ink transition-colors">Integration</a>
          <a href="{base}/docs/api-reference" class="block text-ink-muted hover:text-ink transition-colors">API</a>
          <a href="{base}/docs/configuration" class="block text-ink-muted hover:text-ink transition-colors">Configuration</a>
        </div>
      </div>
      <div>
        <p class="text-[12px] uppercase tracking-[0.12em] text-ink-muted mb-4">Resources</p>
        <div class="space-y-2.5 text-[14px]">
          <a href="{base}/docs" class="block text-ink-muted hover:text-ink transition-colors">Documentation</a>
          <a href="{base}/docs/cli" class="block text-ink-muted hover:text-ink transition-colors">CLI Reference</a>
        </div>
      </div>
      <div>
        <p class="text-[12px] uppercase tracking-[0.12em] text-ink-muted mb-4">Community</p>
        <div class="space-y-2.5 text-[14px]">
          <a href="https://github.com/bravo1goingdark/keplor" target="_blank" rel="noopener" class="block text-ink-muted hover:text-ink transition-colors">GitHub</a>
          <a href="https://github.com/bravo1goingdark/keplor/issues" target="_blank" rel="noopener" class="block text-ink-muted hover:text-ink transition-colors">Issues</a>
        </div>
      </div>
      <div>
        <p class="text-[12px] uppercase tracking-[0.12em] text-ink-muted mb-4">Legal</p>
        <div class="space-y-2.5 text-[14px]">
          <span class="block text-ink-muted">Apache-2.0</span>
        </div>
      </div>
    </div>

    <div class="flex flex-col sm:flex-row items-start sm:items-end justify-between gap-4">
      <div>
        <div class="flex items-center gap-2">
          <svg class="w-5 h-5" viewBox="0 0 64 64" fill="none">
            <line x1="14" y1="9" x2="14" y2="55" stroke="currentColor" stroke-width="3.7" stroke-linecap="round"/>
            <line x1="16.6" y1="32" x2="50" y2="9" stroke="currentColor" stroke-width="3.7" stroke-linecap="round"/>
            <line x1="17.3" y1="37.1" x2="50" y2="17.3" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" opacity="0.28"/>
            <line x1="16.6" y1="32" x2="50" y2="55" stroke="currentColor" stroke-width="3.7" stroke-linecap="round"/>
            <line x1="17.3" y1="26.9" x2="50" y2="46.7" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" opacity="0.28"/>
            <circle cx="16.6" cy="32" r="3.2" class="fill-accent"/>
          </svg>
          <span class="text-[15px] font-semibold tracking-[-0.02em]" style="font-family: 'Space Grotesk', system-ui, sans-serif">kepl<span class="text-accent">o</span>r</span>
        </div>
        <p class="text-[13px] text-ink-muted mt-1">Observe what's already there.</p>
      </div>
      <p class="text-[12px] text-ink-muted">&copy; 2026 Keplor. Open source under Apache-2.0.</p>
    </div>
  </div>
</footer>
