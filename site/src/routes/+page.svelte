<script lang="ts">
  import CodeBlock from '$lib/components/CodeBlock.svelte';

  let activeTab = $state('curl');

  const curlCode = `<span class="text-text-muted">$</span> <span class="text-green-300">curl</span> -X POST http://localhost:8080/v1/events \\
  -H <span class="text-amber-300">"Content-Type: application/json"</span> \\
  -d <span class="text-amber-300">'{"model":"gpt-4o","provider":"openai",
      "usage":{"input_tokens":500,"output_tokens":200}}'</span>

<span class="text-text-muted">{"id":"01J5X...","cost_nanodollars":6250000,"provider":"openai"}</span>`;

  const curlRaw = `curl -X POST http://localhost:8080/v1/events \\
  -H "Content-Type: application/json" \\
  -d '{"model":"gpt-4o","provider":"openai","usage":{"input_tokens":500,"output_tokens":200}}'`;

  const pythonCode = `<span class="text-purple-300">import</span> requests

resp = requests.post(
    <span class="text-amber-300">"http://localhost:8080/v1/events"</span>,
    headers={<span class="text-amber-300">"Authorization"</span>: <span class="text-amber-300">f"Bearer {api_key}"</span>},
    json={
        <span class="text-amber-300">"model"</span>: <span class="text-amber-300">"gpt-4o"</span>,
        <span class="text-amber-300">"provider"</span>: <span class="text-amber-300">"openai"</span>,
        <span class="text-amber-300">"usage"</span>: {<span class="text-amber-300">"input_tokens"</span>: 1200, <span class="text-amber-300">"output_tokens"</span>: 350},
        <span class="text-amber-300">"user_id"</span>: <span class="text-amber-300">"alice"</span>,
    },
)`;

  const pythonRaw = `import requests\n\nresp = requests.post(\n    "http://localhost:8080/v1/events",\n    json={"model": "gpt-4o", "provider": "openai"},\n)`;

  const tsCode = `<span class="text-purple-300">const</span> resp = <span class="text-purple-300">await</span> fetch(<span class="text-amber-300">"http://localhost:8080/v1/events"</span>, {
  method: <span class="text-amber-300">"POST"</span>,
  headers: {
    <span class="text-amber-300">"Authorization"</span>: <span class="text-amber-300">\`Bearer \${apiKey}\`</span>,
    <span class="text-amber-300">"Content-Type"</span>: <span class="text-amber-300">"application/json"</span>,
  },
  body: JSON.stringify({
    model: <span class="text-amber-300">"gpt-4o"</span>,
    provider: <span class="text-amber-300">"openai"</span>,
    usage: { input_tokens: 1200, output_tokens: 350 },
  }),
});`;

  const tsRaw = `const resp = await fetch("http://localhost:8080/v1/events", {\n  method: "POST",\n  body: JSON.stringify({ model: "gpt-4o", provider: "openai" }),\n});`;

  const tabs = [
    { id: 'curl', label: 'cURL', code: curlCode, raw: curlRaw },
    { id: 'python', label: 'Python', code: pythonCode, raw: pythonRaw },
    { id: 'ts', label: 'TypeScript', code: tsCode, raw: tsRaw },
  ];

  const features = [
    { tag: 'cost_nanodollars', title: 'Nanodollar precision', desc: 'Auto-computed from LiteLLM pricing catalog. Per-user, per-key, per-model attribution.' },
    { tag: 'request + response', title: 'Full-body capture', desc: 'Request and response stored with zstd compression and content-addressed deduplication.' },
    { tag: 'sha256 + refcount', title: 'Content-addressed dedup', desc: 'Identical system prompts stored once. Trained zstd dictionaries for 5x compression.' },
    { tag: '13 providers', title: 'Universal coverage', desc: 'OpenAI, Anthropic, Gemini, Bedrock, Azure, Mistral, Groq, xAI, DeepSeek, Cohere, Ollama.' },
    { tag: 'GET /v1/events', title: 'Query API', desc: 'Filter by user, model, provider, time range. Cursor-based pagination. Sub-100us latency.' },
    { tag: 'GET /metrics', title: 'Prometheus native', desc: 'Built-in metrics endpoint. Structured JSON logging. Plug into Grafana or Datadog.' },
  ];

  const providers = ['OpenAI', 'Anthropic', 'Gemini', 'Vertex AI', 'Bedrock', 'Azure OpenAI', 'Mistral', 'Groq', 'xAI', 'DeepSeek', 'Cohere', 'Ollama'];

  const stats = [
    { value: '<10 MB', label: 'static binary' },
    { value: '368K', label: 'events/sec' },
    { value: '13', label: 'providers' },
    { value: '<1 ms', label: 'p99 overhead' },
  ];
</script>

<svelte:head>
  <title>Keplor - LLM Observability Infrastructure</title>
  <meta name="description" content="Open-source LLM observability. Track every token, every cost, every request. Single static binary under 10MB." />
</svelte:head>

<!-- Hero -->
<section class="relative pt-20 pb-16 px-6 overflow-hidden">
  <div class="absolute top-[-200px] left-1/2 -translate-x-1/2 w-[600px] h-[600px] rounded-full bg-[radial-gradient(circle,rgba(59,130,246,0.06)_0%,transparent_70%)] pointer-events-none"></div>

  <div class="max-w-3xl mx-auto text-center relative">
    <div class="inline-flex items-center gap-2 px-3 py-1 rounded-full text-[11px] font-mono bg-bg-raised border border-border text-text-muted mb-8">
      <span class="text-accent">v0.1.0</span>
      <span>Apache-2.0</span>
    </div>

    <h1 class="text-4xl sm:text-5xl lg:text-[3.5rem] font-bold tracking-tight leading-[1.1] mb-6">
      LLM observability<br />
      <span class="text-accent">without the overhead</span>
    </h1>

    <p class="text-lg sm:text-xl text-text-muted/80 leading-relaxed mb-10 max-w-xl mx-auto">
      Track every token, every cost, every request across all providers. Single static binary. Sub-millisecond overhead.
    </p>

    <div class="max-w-xl mx-auto mb-10">
      <CodeBlock code={curlCode} raw={curlRaw} />
    </div>

    <div class="flex flex-col sm:flex-row items-center justify-center gap-3">
      <a href="/docs/quickstart" class="px-6 py-2.5 bg-accent hover:bg-accent-dim text-white text-sm font-medium rounded-md transition-colors">
        Read the Quickstart
      </a>
      <a href="https://github.com/bravo1goingdark/keplor" target="_blank" rel="noopener" class="px-6 py-2.5 border border-border hover:border-border-hover text-sm font-medium rounded-md transition-colors">
        View on GitHub
      </a>
    </div>
  </div>
</section>

<!-- Stats -->
<section class="pb-20 px-6">
  <div class="max-w-3xl mx-auto flex flex-wrap justify-center gap-3">
    {#each stats as { value, label }}
      <div class="bg-bg-raised border border-border rounded-full px-5 py-2 text-[13px]">
        <span class="text-accent font-mono font-semibold">{value}</span>
        <span class="text-text-muted ml-1">{label}</span>
      </div>
    {/each}
  </div>
</section>

<!-- Features -->
<section class="pb-24 px-6">
  <div class="max-w-5xl mx-auto">
    <h2 class="text-2xl font-semibold text-center mb-3">What you get</h2>
    <p class="text-center text-text-muted mb-12 max-w-lg mx-auto text-[15px]">
      Everything you need to understand your LLM spend and behavior.
    </p>
    <div class="grid sm:grid-cols-2 lg:grid-cols-3 gap-3">
      {#each features as { tag, title, desc }}
        <div class="bg-bg-raised border border-border rounded-xl p-6 hover:border-border-hover transition-colors">
          <div class="text-[12px] font-mono text-accent mb-3">{tag}</div>
          <h3 class="font-semibold mb-2 text-[15px]">{title}</h3>
          <p class="text-[13px] text-text-muted leading-relaxed">{desc}</p>
        </div>
      {/each}
    </div>
  </div>
</section>

<!-- How it works -->
<section class="pb-24 px-6">
  <div class="max-w-2xl mx-auto">
    <h2 class="text-2xl font-semibold text-center mb-12">How it works</h2>
    <div class="space-y-8">
      {#each [
        { n: '1', title: 'Instrument your gateway', desc: 'Point your LiteLLM proxy or application at Keplor\'s ingestion endpoint. One HTTP POST per LLM call.' },
        { n: '2', title: 'Keplor processes and stores', desc: 'Events are validated, cost-computed, compressed, and deduplicated into SQLite. Batched writes for throughput.' },
        { n: '3', title: 'Query and analyze', desc: 'Use the REST API or CLI to query events and costs. Prometheus metrics for real-time dashboards.' },
      ] as { n, title, desc }}
        <div class="flex gap-5">
          <div class="w-8 h-8 rounded-full border-[1.5px] border-accent flex items-center justify-center text-accent text-[13px] font-mono shrink-0 mt-0.5">{n}</div>
          <div>
            <h3 class="font-semibold mb-1 text-[15px]">{title}</h3>
            <p class="text-[13px] text-text-muted leading-relaxed">{desc}</p>
          </div>
        </div>
      {/each}
    </div>
  </div>
</section>

<!-- Code examples -->
<section class="pb-24 px-6">
  <div class="max-w-3xl mx-auto">
    <h2 class="text-2xl font-semibold text-center mb-3">Integrate in minutes</h2>
    <p class="text-center text-text-muted mb-8 text-[15px]">One HTTP call. Any language.</p>

    <div class="border border-border rounded-xl overflow-hidden">
      <div class="flex border-b border-border bg-bg-raised">
        {#each tabs as tab}
          <button
            onclick={() => (activeTab = tab.id)}
            class="px-4 py-2.5 text-[12px] font-mono border-b-2 transition-colors cursor-pointer
                   {activeTab === tab.id
                     ? 'text-accent border-accent'
                     : 'text-text-muted border-transparent hover:text-text'}"
          >
            {tab.label}
          </button>
        {/each}
      </div>
      {#each tabs as tab}
        {#if activeTab === tab.id}
          <div>
            <pre class="p-5 overflow-x-auto text-[13px] leading-7 font-mono"><code>{@html tab.code}</code></pre>
          </div>
        {/if}
      {/each}
    </div>
  </div>
</section>

<!-- Providers -->
<section class="pb-24 px-6">
  <div class="max-w-4xl mx-auto text-center">
    <h2 class="text-2xl font-semibold mb-8">Supported providers</h2>
    <div class="flex flex-wrap justify-center gap-2">
      {#each providers as p}
        <span class="bg-bg-raised border border-border rounded-md px-3 py-1.5 text-[12px] text-text-muted font-mono">{p}</span>
      {/each}
    </div>
  </div>
</section>

<!-- CTA -->
<section class="pb-24 px-6">
  <div class="max-w-2xl mx-auto text-center py-16 px-8 bg-bg-raised border border-border rounded-2xl">
    <h2 class="text-2xl font-semibold mb-3">Start observing</h2>
    <p class="text-text-muted mb-8 text-[15px]">Download the binary. Start the server. Send your first event.</p>
    <a href="/docs/quickstart" class="inline-block px-6 py-2.5 bg-accent hover:bg-accent-dim text-white text-sm font-medium rounded-md transition-colors">
      Quickstart Guide
    </a>
  </div>
</section>
