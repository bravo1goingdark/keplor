<svelte:head>
  <title>Documentation - Keplor</title>
</svelte:head>

<h1 class="text-3xl font-bold mb-2">Documentation</h1>
<p class="text-lg text-text-muted mb-12">Everything you need to deploy and operate Keplor.</p>

<div class="grid sm:grid-cols-2 gap-3">
  {#each [
    { href: '/docs/quickstart', n: '01', title: 'Quickstart', desc: 'Install, run, and send your first event in under 2 minutes.' },
    { href: '/docs/api-reference', n: '02', title: 'API Reference', desc: 'Complete HTTP endpoint documentation with schemas.' },
    { href: '/docs/configuration', n: '03', title: 'Configuration', desc: 'TOML config file, environment variables, and defaults.' },
    { href: '/docs/cli', n: '04', title: 'CLI Reference', desc: 'Commands for server, querying, statistics, and maintenance.' },
  ] as { href, n, title, desc }}
    <a {href} class="bg-bg-raised border border-border rounded-xl p-6 hover:border-border-hover transition-colors no-underline block">
      <div class="text-[12px] font-mono text-accent mb-2">{n}</div>
      <h3 class="font-semibold mb-1 text-[15px] text-text">{title}</h3>
      <p class="text-[13px] text-text-muted">{desc}</p>
    </a>
  {/each}
</div>

<h2>Architecture</h2>
<p>Keplor is a Rust workspace with five crates:</p>
<table>
  <thead><tr><th>Crate</th><th>Purpose</th></tr></thead>
  <tbody>
    <tr><td><code>keplor-core</code></td><td>Event, Provider, Usage, Cost types</td></tr>
    <tr><td><code>keplor-server</code></td><td>HTTP server, ingestion pipeline</td></tr>
    <tr><td><code>keplor-store</code></td><td>SQLite storage, dedup, zstd compression</td></tr>
    <tr><td><code>keplor-pricing</code></td><td>LiteLLM pricing catalog, cost engine</td></tr>
    <tr><td><code>keplor-cli</code></td><td>The <code>keplor</code> binary</td></tr>
  </tbody>
</table>

<h2>Design principles</h2>
<table>
  <thead><tr><th>Principle</th><th>What it means</th></tr></thead>
  <tbody>
    <tr><td>Pure observational</td><td>Never reformat, rewrite, or modify payloads</td></tr>
    <tr><td>Heavy compression</td><td>zstd with trained dictionaries, content-addressed dedup</td></tr>
    <tr><td>Zero-dep default</td><td>SQLite works out of the box, no external services</td></tr>
    <tr><td>Lean binary</td><td>Single static musl binary under 10 MB</td></tr>
  </tbody>
</table>
