<script lang="ts">
  import { base } from '$app/paths';
  import { page } from '$app/state';

  const nav = [
    {
      href: `${base}/docs`,
      label: 'Overview',
      sections: [
        { hash: 'architecture', label: 'Architecture' },
      ],
    },
    {
      href: `${base}/docs/quickstart`,
      label: 'Quickstart',
      sections: [
        { hash: 'install',      label: 'Install' },
        { hash: 'start',        label: 'Start server' },
        { hash: 'first-event',  label: 'First event' },
        { hash: 'query',        label: 'Query' },
        { hash: 'stats',        label: 'Stats' },
        { hash: 'next',         label: 'Next steps' },
      ],
    },
    {
      href: `${base}/docs/integration`,
      label: 'Integration Guide',
      sections: [
        { hash: 'overview',    label: 'How it works' },
        { hash: 'quickstart',  label: 'Minimal example' },
        { hash: 'auth',        label: 'Authentication' },
        { hash: 'schema',      label: 'What to send' },
        { hash: 'what-you-get', label: 'What you get back' },
        { hash: 'providers',   label: 'Providers' },
        { hash: 'cost',        label: 'Cost accounting' },
        { hash: 'batch',       label: 'Batch ingestion' },
        { hash: 'querying',    label: 'Querying' },
        { hash: 'errors',      label: 'Error handling' },
        { hash: 'examples',    label: 'Examples' },
        { hash: 'operations',  label: 'Operations' },
        { hash: 'metrics',     label: 'Metrics' },
        { hash: 'next',        label: 'Next steps' },
      ],
    },
    {
      href: `${base}/docs/api-reference`,
      label: 'API Reference',
      sections: [
        { hash: 'auth',    label: 'Authentication' },
        { hash: 'ingest',  label: 'POST /v1/events' },
        { hash: 'batch',   label: 'POST /v1/events/batch' },
        { hash: 'query',   label: 'GET /v1/events' },
        { hash: 'quota',   label: 'GET /v1/quota' },
        { hash: 'rollups', label: 'GET /v1/rollups' },
        { hash: 'stats',   label: 'GET /v1/stats' },
        { hash: 'health',  label: 'GET /health' },
        { hash: 'metrics', label: 'GET /metrics' },
        { hash: 'headers', label: 'Headers' },
        { hash: 'errors',  label: 'Errors' },
      ],
    },
    {
      href: `${base}/docs/configuration`,
      label: 'Configuration',
      sections: [
        { hash: 'config-file',  label: 'Config file' },
        { hash: 'example',      label: 'Full example' },
        { hash: 'server',       label: '[server]' },
        { hash: 'storage',      label: '[storage]' },
        { hash: 'auth',         label: '[auth]' },
        { hash: 'retention',    label: '[retention]' },
        { hash: 'archive',      label: '[archive]' },
        { hash: 'cors',         label: '[cors]' },
        { hash: 'pipeline',     label: '[pipeline]' },
        { hash: 'idempotency',  label: '[idempotency]' },
        { hash: 'rate-limit',   label: '[rate_limit]' },
        { hash: 'pricing',      label: '[pricing]' },
        { hash: 'tls',          label: '[tls]' },
        { hash: 'env',          label: 'Env vars' },
        { hash: 'perf-build',   label: 'Perf build flags' },
        { hash: 'validation',   label: 'Validation' },
      ],
    },
    {
      href: `${base}/docs/blob-storage`,
      label: 'Event Archival',
      sections: [
        { hash: 'how-it-works',       label: 'How it works' },
        { hash: 'archive-lifecycle',  label: 'Archive lifecycle' },
        { hash: 'r2',                 label: 'Cloudflare R2' },
        { hash: 's3',                 label: 'AWS S3' },
        { hash: 'minio',              label: 'MinIO' },
        { hash: 'config-ref',         label: 'Config reference' },
        { hash: 'retention-warning',  label: 'Archive vs. retention' },
        { hash: 'gc',                 label: 'GC & cleanup' },
        { hash: 'cli',                label: 'CLI commands' },
      ],
    },
    {
      href: `${base}/docs/cli`,
      label: 'CLI',
      sections: [
        { hash: 'run',             label: 'keplor run' },
        { hash: 'migrate',         label: 'keplor migrate' },
        { hash: 'query',           label: 'keplor query' },
        { hash: 'stats',           label: 'keplor stats' },
        { hash: 'gc',              label: 'keplor gc' },
        { hash: 'rollup',          label: 'keplor rollup' },
        { hash: 'archive',         label: 'keplor archive' },
        { hash: 'archive-status',  label: 'keplor archive-status' },
      ],
    },
    {
      href: `${base}/docs/benchmarks`,
      label: 'Benchmarks',
      sections: [
        { hash: 'caveat',           label: 'What you are looking at' },
        { hash: 'setup',            label: 'Methodology' },
        { hash: 'writes',           label: 'Writes' },
        { hash: 'queries',          label: 'Queries' },
        { hash: 'rollups',          label: 'Rollups' },
        { hash: 'wal',              label: 'WAL' },
        { hash: 'compaction',       label: 'Compaction' },
        { hash: 'http-tier',        label: 'HTTP tier (sharded BatchWriter)' },
        { hash: 'caveats',          label: 'Caveats' },
      ],
    },
  ];
</script>

<aside class="hidden lg:block w-44 shrink-0 pt-12 h-full overflow-y-auto no-scrollbar">
  <nav class="space-y-1">
    {#each nav as { href, label, sections }}
      {@const active = page.url.pathname === href}
      <a
        {href}
        class="block px-3 py-1.5 text-[14px] transition-colors rounded-[4px]
               {active ? 'text-ink font-medium' : 'text-ink-muted hover:text-ink'}"
      >
        {label}
      </a>
      {#if active}
        <div class="ml-3 border-l border-line pl-3 space-y-1 mb-1">
          {#each sections as { hash, label: sectionLabel }}
            <a
              href="{href}#{hash}"
              class="block py-0.5 text-[12px] text-ink-muted hover:text-ink transition-colors leading-snug"
            >
              {sectionLabel}
            </a>
          {/each}
        </div>
      {/if}
    {/each}
  </nav>
</aside>
