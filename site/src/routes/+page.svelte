<script lang="ts">
  import { base } from '$app/paths';
  import CodeBlock from '$lib/components/CodeBlock.svelte';

  let activeTab = $state('curl');

  const curlCode = `<span class="text-ink/40">$</span> curl -X POST http://localhost:8080/v1/events \\
  -H <span class="text-[#9a6e3a]">"Content-Type: application/json"</span> \\
  -d <span class="text-[#9a6e3a]">'{"model":"gpt-4o","provider":"openai",
      "usage":{"input_tokens":500,"output_tokens":200}}'</span>`;
  const curlRaw = `curl -X POST http://localhost:8080/v1/events -H "Content-Type: application/json" -d '{"model":"gpt-4o","provider":"openai","usage":{"input_tokens":500,"output_tokens":200}}'`;

  const pythonCode = `<span class="text-ink/40">import</span> requests

resp = requests.post(
    <span class="text-[#9a6e3a]">"http://localhost:8080/v1/events"</span>,
    json={
        <span class="text-[#9a6e3a]">"model"</span>: <span class="text-[#9a6e3a]">"gpt-4o"</span>,
        <span class="text-[#9a6e3a]">"provider"</span>: <span class="text-[#9a6e3a]">"openai"</span>,
        <span class="text-[#9a6e3a]">"usage"</span>: {<span class="text-[#9a6e3a]">"input_tokens"</span>: 500},
    },
)`;
  const pythonRaw = `import requests\nresp = requests.post("http://localhost:8080/v1/events", json={"model":"gpt-4o","provider":"openai"})`;

  const tsCode = `<span class="text-ink/40">const</span> resp = <span class="text-ink/40">await</span> fetch(<span class="text-[#9a6e3a]">"http://localhost:8080/v1/events"</span>, {
  method: <span class="text-[#9a6e3a]">"POST"</span>,
  headers: { <span class="text-[#9a6e3a]">"Content-Type"</span>: <span class="text-[#9a6e3a]">"application/json"</span> },
  body: JSON.stringify({
    model: <span class="text-[#9a6e3a]">"gpt-4o"</span>,
    provider: <span class="text-[#9a6e3a]">"openai"</span>,
    usage: { input_tokens: 500, output_tokens: 200 },
  }),
});`;
  const tsRaw = `const resp = await fetch("http://localhost:8080/v1/events", { method: "POST", body: JSON.stringify({ model: "gpt-4o" }) });`;

  const tabs = [
    { id: 'curl', label: 'curl', code: curlCode, raw: curlRaw },
    { id: 'python', label: 'python', code: pythonCode, raw: pythonRaw },
    { id: 'ts', label: 'typescript', code: tsCode, raw: tsRaw },
  ];
</script>

<svelte:head>
  <title>Keplor - LLM Log Aggregation</title>
  <meta name="description" content="Capture every LLM request and response across all providers. Compressed, deduplicated, queryable. Single binary." />
</svelte:head>

<!-- Hero -->
<section class="pt-40 pb-0 px-6 md:px-20" style="padding-bottom: clamp(96px, 12vw, 160px);">
  <div class="max-w-[1280px] mx-auto">
    <div class="max-w-[66%]">
      <p class="text-[12px] uppercase tracking-[0.12em] text-ink-muted mb-6 animate-in">
        LLM Log Aggregation
      </p>

      <h1 class="font-serif text-[clamp(48px,7vw,96px)] leading-[0.95] tracking-[-0.03em] mb-8 animate-in">
        Every LLM call.<br />Captured.
      </h1>

      <p class="text-[18px] leading-[1.55] text-ink-muted max-w-[52ch] mb-10 animate-in-delay-1">
        One binary captures every LLM call, computes cost from a 2,263-model
        pricing catalog, and serves real-time aggregations. Nothing else needed.
      </p>

      <div class="flex items-center gap-6 animate-in-delay-2">
        <a href="{base}/docs/quickstart" class="px-[22px] py-[12px] bg-accent text-accent-ink text-[15px] font-medium rounded-[6px] hover:-translate-y-px transition-transform">
          Get started
        </a>
        <a href="{base}/docs/api-reference" class="text-[15px] text-ink-muted hover:text-ink transition-colors group">
          See the API <span class="inline-block transition-transform group-hover:translate-x-0.5">&rarr;</span>
        </a>
      </div>
    </div>
  </div>
</section>

<!-- Value -->
<section class="px-6 md:px-20" style="padding-top: clamp(96px, 12vw, 160px); padding-bottom: clamp(96px, 12vw, 160px);">
  <div class="max-w-[1280px] mx-auto">
    <div class="grid md:grid-cols-3 gap-x-8 gap-y-12">
      {#each [
        { n: '01', title: 'Full request/response capture', desc: 'Every prompt and completion stored with zstd compression. System prompts and tool schemas deduplicated by content hash. Trained dictionaries for 5x ratios.' },
        { n: '02', title: 'Cost accounting & aggregation', desc: 'Automatic cost from a 2,263-model pricing catalog. Daily rollups, real-time quota checks, and period statistics — all via REST. Filter by user, model, provider, or time range.' },
        { n: '03', title: 'Zero dependencies', desc: 'A single static binary under 10 MB. SQLite for storage. No containers, no runtime, no cloud account. Just run it.' },
      ] as { n, title, desc }}
        <div>
          <span class="font-serif text-[15px] text-ink-muted">{n}</span>
          <h3 class="text-[17px] font-medium mt-2 mb-2">{title}</h3>
          <p class="text-[15px] text-ink-muted leading-[1.65] max-w-[58ch]">{desc}</p>
        </div>
      {/each}
    </div>
  </div>
</section>

<!-- Demo -->
<section class="bg-bg-alt px-6 md:px-20" style="padding-top: clamp(96px, 12vw, 160px); padding-bottom: clamp(96px, 12vw, 160px);">
  <div class="max-w-[800px] mx-auto">
    <p class="text-[12px] uppercase tracking-[0.12em] text-ink-muted mb-6">Integration</p>

    <div class="border border-line rounded-[4px] bg-bg overflow-hidden">
      <div class="flex border-b border-line">
        {#each tabs as tab}
          <button
            onclick={() => (activeTab = tab.id)}
            class="px-5 py-3 text-[13px] font-mono border-b-2 -mb-px transition-colors cursor-pointer
                   {activeTab === tab.id
                     ? 'text-ink border-ink'
                     : 'text-ink-muted border-transparent hover:text-ink'}"
          >
            {tab.label}
          </button>
        {/each}
      </div>
      {#each tabs as tab}
        {#if activeTab === tab.id}
          <pre class="p-6 overflow-x-auto text-[14px] leading-[1.75] font-mono text-ink-muted"><code>{@html tab.code}</code></pre>
        {/if}
      {/each}
    </div>
  </div>
</section>

<!-- Aggregation -->
<section class="px-6 md:px-20" style="padding-top: clamp(96px, 12vw, 160px); padding-bottom: clamp(96px, 12vw, 160px);">
  <div class="max-w-[1280px] mx-auto">
    <p class="text-[12px] uppercase tracking-[0.12em] text-ink-muted mb-6">Aggregation</p>

    <div class="grid md:grid-cols-3 gap-x-8 gap-y-12">
      {#each [
        { n: '01', title: 'Real-time quota', desc: 'GET /v1/quota returns cost and event count for any user or API key since a given timestamp. Sub-millisecond, hits the event table directly.' },
        { n: '02', title: 'Daily rollups', desc: 'GET /v1/rollups returns pre-aggregated daily breakdowns by provider and model. Background task refreshes every 60 seconds.' },
        { n: '03', title: 'Period statistics', desc: 'GET /v1/stats sums rollups across a date range. Optional group-by model for per-model cost breakdowns.' },
      ] as { n, title, desc }}
        <div>
          <span class="font-serif text-[15px] text-ink-muted">{n}</span>
          <h3 class="text-[17px] font-medium mt-2 mb-2">{title}</h3>
          <p class="text-[15px] text-ink-muted leading-[1.65] max-w-[58ch]">{desc}</p>
        </div>
      {/each}
    </div>
  </div>
</section>

<!-- Numbers -->
<section class="px-6 md:px-20" style="padding-top: clamp(96px, 12vw, 160px); padding-bottom: clamp(96px, 12vw, 160px);">
  <div class="max-w-[1280px] mx-auto grid grid-cols-2 md:grid-cols-4 gap-y-10">
    {#each [
      { n: '<10 MB', l: 'Binary size' },
      { n: '368K/s', l: 'Events throughput' },
      { n: '2,263', l: 'Models priced' },
      { n: '<1 ms', l: 'Ingestion overhead' },
    ] as { n, l }}
      <div>
        <div class="font-serif text-[32px] tracking-[-0.02em]">{n}</div>
        <div class="text-[13px] text-ink-muted mt-1">{l}</div>
      </div>
    {/each}
  </div>
</section>

<!-- CTA -->
<section class="px-6 md:px-20 border-t border-line" style="padding-top: clamp(96px, 12vw, 160px); padding-bottom: clamp(96px, 12vw, 160px);">
  <div class="max-w-[1280px] mx-auto">
    <h2 class="font-serif text-[clamp(32px,4vw,48px)] leading-[1.05] tracking-[-0.02em] mb-5">
      Start logging.
    </h2>
    <p class="text-[17px] text-ink-muted mb-8 max-w-[44ch] leading-[1.6]">
      Five commands from clone to your first captured event.
      No account, no API key, no credit card.
    </p>
    <a href="{base}/docs/quickstart" class="px-[22px] py-[12px] bg-accent text-accent-ink text-[15px] font-medium rounded-[6px] hover:-translate-y-px transition-transform inline-block">
      Read the quickstart
    </a>
  </div>
</section>
