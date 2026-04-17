<script lang="ts">
  import { base } from '$app/paths';
</script>

<svelte:head>
  <title>Documentation - Keplor</title>
</svelte:head>

<h1>Documentation</h1>
<p class="text-[17px] text-ink-muted mb-16">Everything you need to deploy and operate Keplor.</p>

<div class="grid sm:grid-cols-2 gap-4 mb-16">
  {#each [
    { href: `${base}/docs/quickstart`, title: 'Quickstart', desc: 'Install and send your first event.' },
    { href: `${base}/docs/integration`, title: 'Integration Guide', desc: 'Schema, auth, examples for any proxy.' },
    { href: `${base}/docs/api-reference`, title: 'API Reference', desc: 'Endpoints, schemas, and errors.' },
    { href: `${base}/docs/configuration`, title: 'Configuration', desc: 'TOML config and env vars.' },
    { href: `${base}/docs/cli`, title: 'CLI', desc: 'Server, query, stats, and gc.' },
  ] as { href, title, desc }}
    <a {href} class="block p-6 bg-bg-alt rounded-[4px] border border-line hover:border-ink-muted/30 transition-colors no-underline">
      <h3 class="text-[15px] font-medium mb-1 text-ink">{title}</h3>
      <p class="text-[14px] text-ink-muted !mb-0">{desc}</p>
    </a>
  {/each}
</div>

<h2>Architecture</h2>
<table>
  <thead><tr><th>Crate</th><th>Purpose</th></tr></thead>
  <tbody>
    <tr><td><code>keplor-core</code></td><td>Event, Provider, Usage, Cost types</td></tr>
    <tr><td><code>keplor-server</code></td><td>HTTP server, ingestion pipeline</td></tr>
    <tr><td><code>keplor-store</code></td><td>SQLite, dedup, zstd compression</td></tr>
    <tr><td><code>keplor-pricing</code></td><td>LiteLLM pricing catalog</td></tr>
    <tr><td><code>keplor-cli</code></td><td>The <code>keplor</code> binary</td></tr>
  </tbody>
</table>
