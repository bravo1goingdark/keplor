<script lang="ts">
  import { base } from '$app/paths';
  import { onMount } from 'svelte';
  import CodeBlock from '$lib/components/CodeBlock.svelte';

  let activeTab = $state('curl');

  // ── Code samples ──────────────────────────────────────────
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

  // ── Scroll-triggered reveal observer ──────────────────────
  onMount(() => {
    const els = document.querySelectorAll('.reveal, .reveal-scale, .reveal-stagger');
    if (!els.length) return;

    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (e.isIntersecting) {
            e.target.classList.add('visible');
            io.unobserve(e.target);
          }
        }
      },
      { threshold: 0.15, rootMargin: '0px 0px -40px 0px' }
    );
    els.forEach((el) => io.observe(el));
    return () => io.disconnect();
  });
</script>

<svelte:head>
  <title>Keplor — LLM Observability & Cost Accounting</title>
  <meta name="description" content="Ingest every LLM event. Auto-compute cost across 2,263 models. Multi-tenant with tiered retention. Single binary, zero dependencies." />
  <meta name="keywords" content="LLM observability, LLM cost tracking, LLM log ingestion, OpenAI cost, Anthropic cost, AI cost accounting, LLM monitoring" />
  <link rel="canonical" href="https://keplor.dev" />

  <!-- Open Graph -->
  <meta property="og:title" content="Keplor — LLM Observability & Cost Accounting" />
  <meta property="og:description" content="Ingest every LLM event. Auto-compute cost across 2,263 models. Multi-tenant with tiered retention. Single binary, zero dependencies." />
  <meta property="og:url" content="https://keplor.dev" />

  <!-- Twitter -->
  <meta name="twitter:title" content="Keplor — LLM Observability & Cost Accounting" />
  <meta name="twitter:description" content="Ingest every LLM event. Auto-compute cost across 2,263 models. Multi-tenant with tiered retention. Single binary, zero dependencies." />

  <!-- Structured data -->
  {@html `<script type="application/ld+json">${JSON.stringify({
    "@context": "https://schema.org",
    "@type": "SoftwareApplication",
    "name": "Keplor",
    "description": "LLM observability and cost accounting server. Captures every request/response, auto-computes cost from a 2,263-model pricing catalog.",
    "applicationCategory": "DeveloperApplication",
    "operatingSystem": "Linux, macOS",
    "license": "https://opensource.org/licenses/Apache-2.0",
    "url": "https://keplor.dev",
    "offers": { "@type": "Offer", "price": "0", "priceCurrency": "USD" }
  })}</script>`}
</svelte:head>

<!-- ── Hero ─────────────────────────────────────────────── -->
<section class="relative overflow-hidden pt-36 md:pt-44 px-6 md:px-20" style="padding-bottom: clamp(80px, 10vw, 140px);">
  <!-- Decorative glow -->
  <div class="accent-glow -top-48 -right-48"></div>

  <div class="max-w-[1280px] mx-auto relative">
    <div class="max-w-[720px]">
      <p class="text-[12px] uppercase tracking-[0.14em] text-accent font-medium mb-6 animate-in">
        LLM Observability & Cost Accounting
      </p>

      <h1 class="font-serif text-[clamp(44px,6.5vw,88px)] leading-[0.93] tracking-[-0.03em] mb-8 animate-in">
        Every LLM call.<br />Captured. Priced.
      </h1>

      <p class="text-[18px] leading-[1.6] text-ink-muted max-w-[54ch] mb-10 animate-in-delay-1">
        A single binary ingests every prompt and completion, auto-computes cost
        from a <strong class="text-ink font-medium">2,263-model</strong> pricing catalog, and serves
        real-time aggregations. Multi-tenant with configurable retention tiers.
        Zero dependencies to start; S3/R2 when you scale.
      </p>

      <div class="flex flex-wrap items-center gap-4 animate-in-delay-2">
        <a href="{base}/docs/quickstart" class="px-6 py-3 bg-accent text-accent-ink text-[15px] font-medium rounded-[6px] hover:-translate-y-px hover:shadow-[0_4px_12px_rgba(255,91,46,0.25)] transition-all duration-200">
          Get started
        </a>
        <a href="{base}/docs/integration" class="px-6 py-3 border border-line text-[15px] font-medium rounded-[6px] text-ink hover:border-ink-muted/60 transition-colors">
          Integration guide
        </a>
        <a href="{base}/docs/api-reference" class="text-[15px] text-ink-muted hover:text-ink transition-colors group ml-2">
          API reference <span class="inline-block transition-transform group-hover:translate-x-0.5">&rarr;</span>
        </a>
      </div>
    </div>
  </div>
</section>

<!-- ── How it works (visual flow) ────────────────────────── -->
<section class="px-6 md:px-20" style="padding-top: clamp(64px, 8vw, 120px); padding-bottom: clamp(64px, 8vw, 120px);">
  <div class="max-w-[1280px] mx-auto">
    <div class="reveal">
      <p class="text-[12px] uppercase tracking-[0.14em] text-ink-muted mb-10">How it works</p>
    </div>

    <div class="grid md:grid-cols-3 gap-6 reveal-stagger">
      {#each [
        { step: '1', title: 'Your app calls an LLM', desc: 'OpenAI, Anthropic, Gemini, Bedrock, or any of 12 supported providers. Your code doesn\'t change.' },
        { step: '2', title: 'POST the event to Keplor', desc: 'Two required fields: model and provider. Add token counts and Keplor computes cost automatically. Each key gets its own retention tier.' },
        { step: '3', title: 'Query costs and usage', desc: 'Real-time quotas, daily rollups, per-model breakdowns. Filter by user, key, provider, or time range. Export as JSON Lines.' },
      ] as { step, title, desc }}
        <div class="relative p-8 rounded-[8px] border border-line bg-bg hover:border-ink-muted/30 transition-colors">
          <span class="inline-flex items-center justify-center w-8 h-8 rounded-full bg-accent-subtle text-accent text-[13px] font-medium mb-5">{step}</span>
          <h3 class="text-[17px] font-medium mb-2">{title}</h3>
          <p class="text-[15px] text-ink-muted leading-[1.65]">{desc}</p>
        </div>
      {/each}
    </div>
  </div>
</section>

<!-- ── Capabilities ──────────────────────────────────────── -->
<section class="bg-bg-alt px-6 md:px-20" style="padding-top: clamp(80px, 10vw, 140px); padding-bottom: clamp(80px, 10vw, 140px);">
  <div class="max-w-[1280px] mx-auto">
    <div class="reveal">
      <p class="text-[12px] uppercase tracking-[0.14em] text-ink-muted mb-3">Capabilities</p>
      <h2 class="font-serif text-[clamp(28px,3.5vw,44px)] leading-[1.05] tracking-[-0.02em] mb-14 max-w-[500px]">
        Everything you need.<br />Nothing you don't.
      </h2>
    </div>

    <div class="grid md:grid-cols-2 gap-x-16 gap-y-14 reveal-stagger">
      {#each [
        { title: 'Automatic cost accounting', desc: 'Bundled LiteLLM pricing catalog covers 2,263 models across all major providers. Handles cache discounts, reasoning tokens, batch pricing, and audio/image tokens. Cost stored as int64 nanodollars for precision.' },
        { title: 'Full request & response capture', desc: 'Every prompt and completion stored alongside event metadata. Optionally archive old events to Cloudflare R2, AWS S3, or MinIO as compressed JSONL files to keep SQLite lean.' },
        { title: 'Multi-tenant with tiered retention', desc: 'Assign API keys to named retention tiers: free (7 days), pro (90 days), team (180 days), or any custom tier. GC runs per-tier automatically. Tier names and durations are fully configurable.' },
        { title: 'Real-time aggregation API', desc: 'Quota checks, daily rollups, and period statistics via REST. Filter by user, API key, model, provider, or time range. Cursor-based pagination for large result sets.' },
        { title: 'Event archival to S3/R2', desc: 'Archive old events as compressed JSONL to Cloudflare R2, AWS S3, or MinIO. Age-based and size-based triggers. Daily rollups preserved in SQLite. Automatic 6-hour archive cycles with per-chunk error isolation.' },
        { title: 'Zero-dep single binary', desc: 'Static musl binary under 10 MB. SQLite with WAL mode and connection pooling. One-command Docker deploy. No JVM, no runtime, no cloud account required.' },
        { title: 'Server-side key attribution', desc: 'Authenticated keys are injected server-side, preventing clients from spoofing cost attribution. Each key carries a tier, so billing and retention are always tied to the actual caller.' },
        { title: '12 providers, one API', desc: 'OpenAI, Anthropic, Gemini, Bedrock, Azure, Mistral, Groq, xAI, DeepSeek, Cohere, Ollama, and any OpenAI-compatible endpoint. Provider-specific token handling built in.' },
      ] as { title, desc }}
        <div>
          <h3 class="text-[17px] font-medium mb-2">{title}</h3>
          <p class="text-[15px] text-ink-muted leading-[1.65] max-w-[52ch]">{desc}</p>
        </div>
      {/each}
    </div>
  </div>
</section>

<!-- ── Demo ───────────────────────────────────────────────── -->
<section class="px-6 md:px-20" style="padding-top: clamp(80px, 10vw, 140px); padding-bottom: clamp(80px, 10vw, 140px);">
  <div class="max-w-[860px] mx-auto">
    <div class="reveal">
      <p class="text-[12px] uppercase tracking-[0.14em] text-ink-muted mb-3">Integration</p>
      <h2 class="font-serif text-[clamp(28px,3.5vw,44px)] leading-[1.05] tracking-[-0.02em] mb-10">
        Three lines to start.
      </h2>
    </div>

    <div class="reveal-scale">
      <div class="border border-line rounded-[8px] bg-bg-alt overflow-hidden shadow-[0_1px_3px_rgba(0,0,0,0.04)]">
        <div class="flex border-b border-line bg-bg">
          {#each tabs as tab}
            <button
              onclick={() => (activeTab = tab.id)}
              class="px-5 py-3 text-[13px] font-mono border-b-2 -mb-px transition-colors cursor-pointer
                     {activeTab === tab.id
                       ? 'text-ink border-accent'
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

    <p class="text-[14px] text-ink-muted mt-6 reveal">
      Returns cost in nanodollars, event ID, and normalized model/provider. See the
      <a href="{base}/docs/integration" class="text-accent hover:underline">integration guide</a> for Python, Node.js, and LiteLLM examples.
    </p>
  </div>
</section>

<!-- ── Numbers ────────────────────────────────────────────── -->
<section class="px-6 md:px-20 border-t border-line" style="padding-top: clamp(80px, 10vw, 140px); padding-bottom: clamp(80px, 10vw, 140px);">
  <div class="max-w-[1280px] mx-auto">
    <div class="grid grid-cols-2 md:grid-cols-4 gap-y-12 reveal-stagger">
      {#each [
        { n: '<10 MB', label: 'Binary size', sub: 'Single static musl binary' },
        { n: '261K/s', label: 'Events throughput', sub: 'Per core, fire-and-forget' },
        { n: '2,263', label: 'Models priced', sub: 'LiteLLM catalog, auto-refreshed' },
        { n: '<1 ms', label: 'Ingestion overhead', sub: 'p99 ingestion latency' },
      ] as { n, label, sub }}
        <div>
          <div class="font-serif text-[clamp(28px,3.5vw,40px)] tracking-[-0.02em] text-ink">{n}</div>
          <div class="text-[14px] font-medium mt-2">{label}</div>
          <div class="text-[13px] text-ink-muted mt-0.5">{sub}</div>
        </div>
      {/each}
    </div>
  </div>
</section>

<!-- ── Providers ──────────────────────────────────────────── -->
<section class="bg-bg-alt px-6 md:px-20" style="padding-top: clamp(80px, 10vw, 140px); padding-bottom: clamp(80px, 10vw, 140px);">
  <div class="max-w-[1280px] mx-auto text-center">
    <div class="reveal">
      <p class="text-[12px] uppercase tracking-[0.14em] text-ink-muted mb-3">Supported providers</p>
      <h2 class="font-serif text-[clamp(28px,3.5vw,44px)] leading-[1.05] tracking-[-0.02em] mb-12">
        Every major LLM provider.
      </h2>
    </div>

    <div class="flex flex-wrap justify-center gap-3 max-w-[720px] mx-auto reveal-stagger">
      {#each ['OpenAI', 'Anthropic', 'Gemini', 'Vertex AI', 'AWS Bedrock', 'Azure OpenAI', 'Mistral', 'Groq', 'xAI Grok', 'DeepSeek', 'Cohere', 'Ollama'] as provider}
        <span class="px-4 py-2 text-[14px] border border-line rounded-full text-ink-muted hover:text-ink hover:border-ink-muted/40 transition-colors">
          {provider}
        </span>
      {/each}
      <span class="px-4 py-2 text-[14px] border border-dashed border-line rounded-full text-ink-muted">
        + any OpenAI-compatible
      </span>
    </div>
  </div>
</section>

<!-- ── CTA ────────────────────────────────────────────────── -->
<section class="px-6 md:px-20" style="padding-top: clamp(96px, 12vw, 160px); padding-bottom: clamp(96px, 12vw, 160px);">
  <div class="max-w-[1280px] mx-auto text-center reveal">
    <h2 class="font-serif text-[clamp(32px,4.5vw,56px)] leading-[1.0] tracking-[-0.02em] mb-5">
      Start observing.
    </h2>
    <p class="text-[17px] text-ink-muted mb-10 max-w-[44ch] mx-auto leading-[1.6]">
      <code>docker compose up</code> or build from source.
      No account, no API key, no credit card.
    </p>
    <div class="flex flex-wrap justify-center gap-4">
      <a href="{base}/docs/quickstart" class="px-6 py-3 bg-accent text-accent-ink text-[15px] font-medium rounded-[6px] hover:-translate-y-px hover:shadow-[0_4px_12px_rgba(255,91,46,0.25)] transition-all duration-200">
        Read the quickstart
      </a>
      <a href="https://github.com/bravo1goingdark/keplor" target="_blank" rel="noopener" class="px-6 py-3 border border-line text-[15px] font-medium rounded-[6px] text-ink hover:border-ink-muted/60 transition-colors">
        View on GitHub
      </a>
    </div>
  </div>
</section>
