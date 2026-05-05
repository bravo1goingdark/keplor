<script lang="ts">
  import { base } from '$app/paths';
  import Pre from '$lib/components/Pre.svelte';

  const curlIngest = `$ curl -X POST http://localhost:8080/v1/events \\
  -H "Authorization: Bearer sk-your-key" \\
  -H "Content-Type: application/json" \\
  -d '{
    "model": "gpt-4o",
    "provider": "openai",
    "usage": {"input_tokens": 1000, "output_tokens": 500},
    "latency": {"ttft_ms": 30, "total_ms": 450},
    "user_id": "alice",
    "source": "my-app"
  }'`;

  const curlResponse = `{
  "id": "01JA2B3C4D5E6F7G8H9J0KMNPQ",
  "cost_nanodollars": 6250000,
  "model": "gpt-4o",
  "provider": "openai"
}`;

  const pythonExample = 'import requests, time\n\n' +
'KEPLOR = "http://localhost:8080"\n' +
'HEADERS = {\n' +
'    "Authorization": "Bearer sk-your-key",\n' +
'    "Content-Type": "application/json",\n' +
'}\n\n' +
'# After each LLM call, log to Keplor\n' +
'def log_llm_event(model, provider, usage, latency_ms, user_id=None):\n' +
'    return requests.post(f"{KEPLOR}/v1/events", headers=HEADERS, json={\n' +
'        "model": model,\n' +
'        "provider": provider,\n' +
'        "usage": usage,\n' +
'        "latency": {"total_ms": latency_ms},\n' +
'        "http_status": 200,\n' +
'        "user_id": user_id,\n' +
'        "source": "my-app",\n' +
'    }).json()\n\n' +
'result = log_llm_event("gpt-4o", "openai", {\n' +
'    "input_tokens": 1500,\n' +
'    "output_tokens": 800,\n' +
'}, 450, user_id="alice")\n\n' +
'print(f"Event {result[\'id\']} cost: ${result[\'cost_nanodollars\'] / 1e9:.6f}")';

  const nodeExample = 'const KEPLOR = "http://localhost:8080";\n' +
'const headers = {\n' +
'  "Authorization": "Bearer sk-your-key",\n' +
'  "Content-Type": "application/json",\n' +
'};\n\n' +
'async function logLlmCall(model, provider, usage, latencyMs, userId) {\n' +
'  const resp = await fetch(`${KEPLOR}/v1/events`, {\n' +
'    method: "POST",\n' +
'    headers,\n' +
'    body: JSON.stringify({\n' +
'      model, provider, usage,\n' +
'      latency: { total_ms: latencyMs },\n' +
'      http_status: 200,\n' +
'      user_id: userId,\n' +
'      source: "my-node-app",\n' +
'    }),\n' +
'  });\n' +
'  return resp.json();\n' +
'}\n\n' +
'const result = await logLlmCall("gpt-4o", "openai",\n' +
'  { input_tokens: 1200, output_tokens: 600 }, 350, "alice");\n' +
'console.log(`Cost: $${(result.cost_nanodollars / 1e9).toFixed(6)}`);';

  const batchExample = `$ curl -X POST http://localhost:8080/v1/events/batch \\
  -H "Authorization: Bearer sk-your-key" \\
  -H "Content-Type: application/json" \\
  -d '{
    "events": [
      {"model": "gpt-4o", "provider": "openai", "usage": {"input_tokens": 500}},
      {"model": "claude-sonnet-4-20250514", "provider": "anthropic",
       "usage": {"input_tokens": 800, "output_tokens": 200}}
    ]
  }'`;

  const batchResponse = `{
  "results": [
    {"id": "01JA...", "cost_nanodollars": 1250000, "model": "gpt-4o", "provider": "openai"},
    {"id": "01JB...", "cost_nanodollars": 4200000, "model": "claude-sonnet-4-20250514", "provider": "anthropic"}
  ],
  "accepted": 2,
  "rejected": 0
}`;

  const litellmCallback = 'import litellm, requests\n\n' +
'KEPLOR = "http://localhost:8080"\n\n' +
'def keplor_callback(kwargs, completion_response, start_time, end_time):\n' +
'    latency_ms = int((end_time - start_time).total_seconds() * 1000)\n' +
'    usage = completion_response.get("usage", {})\n' +
'    requests.post(f"{KEPLOR}/v1/events", json={\n' +
'        "model": kwargs.get("model", ""),\n' +
'        "provider": kwargs.get("custom_llm_provider", "openai"),\n' +
'        "usage": {\n' +
'            "input_tokens": usage.get("prompt_tokens", 0),\n' +
'            "output_tokens": usage.get("completion_tokens", 0),\n' +
'        },\n' +
'        "latency": {"total_ms": latency_ms},\n' +
'        "http_status": 200,\n' +
'        "user_id": kwargs.get("user"),\n' +
'        "source": "litellm",\n' +
'    })\n\n' +
'litellm.success_callback = [keplor_callback]';

  const quotaExample = `$ curl "http://localhost:8080/v1/quota?user_id=alice&from=1700000000000000000" \\
  -H "Authorization: Bearer sk-your-key"`;

  const quotaResponse = `{"cost_nanodollars": 150000000, "event_count": 85}`;

  const fullEventExample = `{
  "model": "claude-sonnet-4-20250514",
  "provider": "anthropic",
  "cost_nanodollars": null,
  "usage": {
    "input_tokens": 2000,
    "output_tokens": 1000,
    "cache_read_input_tokens": 500,
    "cache_creation_input_tokens": 0,
    "reasoning_tokens": 0
  },
  "latency": {
    "ttft_ms": 45,
    "total_ms": 800,
    "time_to_close_ms": 20
  },
  "timestamp": "2026-04-15T10:30:00Z",
  "method": "POST",
  "endpoint": "/v1/messages",
  "http_status": 200,
  "source": "litellm",
  "user_id": "alice",
  "api_key_id": "my-service",
  "org_id": "acme-corp",
  "project_id": "chatbot-v2",
  "route_id": "chat",
  "flags": {
    "streaming": true,
    "tool_calls": false,
    "reasoning": true,
    "stream_incomplete": false,
    "cache_used": true
  },
  "error": null,
  "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
  "request_id": "req_abc123",
  "client_ip": "10.0.1.42",
  "user_agent": "my-app/1.0",
  "metadata": {"session_id": "sess_xyz", "user_tag": "premium"}
}`;
</script>

<svelte:head>
  <title>Integration Guide - Keplor</title>
</svelte:head>

<h1 class="text-3xl font-bold mb-2">Integration Guide</h1>
<p class="text-lg text-text-muted mb-8">Everything a service needs to log LLM traffic through Keplor.</p>

<h2 id="overview">How it works</h2>
<p>Your application or gateway makes LLM calls as usual, then POSTs event data to Keplor. Keplor computes cost from its bundled pricing catalog, compresses and stores the event, and makes it queryable via the API.</p>

<Pre code={"Your App / Gateway                Keplor\n      |                              |\n      |-- LLM call --> Provider      |\n      |<-- response --               |\n      |                              |\n      |-- POST /v1/events ---------> | validate, compute cost,\n      |<-- 201 {id, cost} ---------- | compress, store"} />

<p>Keplor is <strong>observational only</strong> &mdash; it never touches your LLM traffic. It just records what happened.</p>

<h2 id="quickstart">Minimal example</h2>
<p>Send the two required fields (<code>model</code> and <code>provider</code>) plus token counts:</p>
<Pre code={curlIngest} />
<p>Response:</p>
<Pre code={curlResponse} />
<p>Cost is auto-computed: <code>6250000</code> nanodollars = <strong>$0.00625</strong>.</p>

<h2 id="auth">Authentication</h2>
<p>When API keys are configured, include a Bearer token:</p>
<pre><code>Authorization: Bearer sk-your-key</code></pre>

<h3>Key formats</h3>
<table>
  <thead><tr><th>Config format</th><th>Key ID</th><th>Tier</th></tr></thead>
  <tbody>
    <tr><td><code>"prod-svc:sk-abc123"</code></td><td><code>prod-svc</code></td><td><code>default_tier</code></td></tr>
    <tr><td><code>"sk-abc123"</code></td><td><code>key_&lt;sha256-prefix&gt;</code></td><td><code>default_tier</code></td></tr>
    <tr><td><code>{"{ id, secret, tier }"}</code></td><td><code>id</code> value</td><td>explicit <code>tier</code></td></tr>
  </tbody>
</table>

<h3>Retention tiers</h3>
<p>Each API key is assigned a <strong>retention tier</strong> that controls how long its events are kept. Configure tiers in <code>[retention]</code>:</p>
<pre><code>[retention]
default_tier = "free"

[[retention.tiers]]
name = "free"
days = 7

[[retention.tiers]]
name = "pro"
days = 90

[[retention.tiers]]
name = "team"
days = 180</code></pre>
<p>Assign tiers to keys using the extended format:</p>
<pre><code>[[auth.api_key_entries]]
id = "pro-user"
secret = "sk-pro-key"
tier = "pro"</code></pre>
<p>Tier names are fully configurable &mdash; add <code>"enterprise"</code>, <code>"trial"</code>, or any custom name. GC runs one pass per tier automatically.</p>

<h3>Server-side key attribution</h3>
<p>When auth is enabled, Keplor <strong>overrides</strong> the client-provided <code>api_key_id</code> with the authenticated key's ID and assigns the key's retention tier. This prevents clients from spoofing attribution.</p>

<h3>Open mode</h3>
<p>When no keys are configured (the default), auth is disabled. All requests are accepted without a Bearer token and assigned to <code>default_tier</code>.</p>

<h2 id="schema">What to send</h2>
<p>Only <code>model</code> and <code>provider</code> are required. Everything else is optional with sensible defaults.</p>

<h3>Required fields</h3>
<table>
  <thead><tr><th>Field</th><th>Type</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>model</code></td><td>string</td><td>Model name (<code>"gpt-4o"</code>, <code>"claude-sonnet-4-20250514"</code>)</td></tr>
    <tr><td><code>provider</code></td><td>string</td><td>Provider key (see <a href="#providers">supported providers</a>)</td></tr>
  </tbody>
</table>

<h3>Token usage</h3>
<table>
  <thead><tr><th>Field</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>usage.input_tokens</code></td><td>u32</td><td>0</td><td>Input/prompt tokens</td></tr>
    <tr><td><code>usage.output_tokens</code></td><td>u32</td><td>0</td><td>Output/completion tokens</td></tr>
    <tr><td><code>usage.cache_read_input_tokens</code></td><td>u32</td><td>0</td><td>Tokens served from cache</td></tr>
    <tr><td><code>usage.cache_creation_input_tokens</code></td><td>u32</td><td>0</td><td>Tokens written to cache</td></tr>
    <tr><td><code>usage.reasoning_tokens</code></td><td>u32</td><td>0</td><td>Chain-of-thought / thinking tokens</td></tr>
    <tr><td><code>usage.audio_input_tokens</code></td><td>u32</td><td>0</td><td>Audio input tokens</td></tr>
    <tr><td><code>usage.audio_output_tokens</code></td><td>u32</td><td>0</td><td>Audio output tokens</td></tr>
    <tr><td><code>usage.image_tokens</code></td><td>u32</td><td>0</td><td>Image/vision tokens</td></tr>
    <tr><td><code>usage.tool_use_tokens</code></td><td>u32</td><td>0</td><td>Tool/function call tokens</td></tr>
  </tbody>
</table>

<h3>Latency</h3>
<table>
  <thead><tr><th>Field</th><th>Type</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>latency.ttft_ms</code></td><td>u32</td><td>Time to first byte (ms)</td></tr>
    <tr><td><code>latency.total_ms</code></td><td>u32</td><td>End-to-end latency (ms)</td></tr>
    <tr><td><code>latency.time_to_close_ms</code></td><td>u32</td><td>Time from last token to stream close</td></tr>
  </tbody>
</table>

<h3>Attribution</h3>
<table>
  <thead><tr><th>Field</th><th>Type</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>user_id</code></td><td>string</td><td>User identity for cost attribution</td></tr>
    <tr><td><code>api_key_id</code></td><td>string</td><td>API key (overridden by server when auth enabled)</td></tr>
    <tr><td><code>org_id</code></td><td>string</td><td>Organization ID</td></tr>
    <tr><td><code>project_id</code></td><td>string</td><td>Project ID</td></tr>
    <tr><td><code>route_id</code></td><td>string</td><td>Logical route name (<code>"chat"</code>, <code>"embeddings"</code>)</td></tr>
    <tr><td><code>source</code></td><td>string</td><td>Name of the sending system</td></tr>
  </tbody>
</table>

<h3>Flags</h3>
<table>
  <thead><tr><th>Field</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>flags.streaming</code></td><td>bool</td><td>false</td><td>Response was streamed</td></tr>
    <tr><td><code>flags.tool_calls</code></td><td>bool</td><td>false</td><td>Included tool/function calls</td></tr>
    <tr><td><code>flags.reasoning</code></td><td>bool</td><td>false</td><td>Used extended thinking</td></tr>
    <tr><td><code>flags.stream_incomplete</code></td><td>bool</td><td>false</td><td>Stream ended prematurely</td></tr>
    <tr><td><code>flags.cache_used</code></td><td>bool</td><td>false</td><td>Response served from cache</td></tr>
  </tbody>
</table>

<h3>Other optional fields</h3>
<table>
  <thead><tr><th>Field</th><th>Type</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>cost_nanodollars</code></td><td>i64</td><td>Override auto-computed cost (nanodollars)</td></tr>
    <tr><td><code>timestamp</code></td><td>i64 or string</td><td>Epoch nanos or ISO 8601 (default: server time)</td></tr>
    <tr><td><code>method</code></td><td>string</td><td>HTTP method (default: <code>"POST"</code>)</td></tr>
    <tr><td><code>endpoint</code></td><td>string</td><td>API path (<code>"/v1/chat/completions"</code>)</td></tr>
    <tr><td><code>http_status</code></td><td>u16</td><td>Upstream HTTP status code</td></tr>
    <tr><td><code>error.kind</code></td><td>string</td><td>Error category (<code>"rate_limited"</code>, etc.)</td></tr>
    <tr><td><code>error.message</code></td><td>string</td><td>Error message text</td></tr>
    <tr><td><code>error.status</code></td><td>u16</td><td>Error HTTP status</td></tr>
    <tr><td><code>trace_id</code></td><td>string</td><td>W3C trace ID</td></tr>
    <tr><td><code>request_id</code></td><td>string</td><td>Provider request ID</td></tr>
    <tr><td><code>client_ip</code></td><td>string</td><td>Client source IP</td></tr>
    <tr><td><code>user_agent</code></td><td>string</td><td>Client user-agent</td></tr>
    <tr><td><code>metadata</code></td><td>any JSON</td><td>Arbitrary metadata (queryable via <code>user_tag</code>/<code>session_tag</code>; capped at 64 KB)</td></tr>
  </tbody>
</table>
<p class="text-sm text-text-muted"><strong>Schema is strict.</strong> <code>IngestEvent</code> rejects unknown fields with HTTP 422 &mdash; no silent drops. If you&rsquo;re migrating from a system that captured request/response bodies, store those in your own object store keyed by the returned event <code>id</code>; Keplor only handles the metadata.</p>

<h3>Full event example</h3>
<Pre code={fullEventExample} />

<h2 id="what-you-get">What you get back</h2>
<p>Every successful ingest returns:</p>
<table>
  <thead><tr><th>Field</th><th>Type</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>id</code></td><td>string</td><td>ULID (time-sortable unique ID)</td></tr>
    <tr><td><code>cost_nanodollars</code></td><td>i64</td><td>Computed or overridden cost</td></tr>
    <tr><td><code>model</code></td><td>string</td><td>Normalized model name</td></tr>
    <tr><td><code>provider</code></td><td>string</td><td>Normalized provider key</td></tr>
  </tbody>
</table>

<h2 id="providers">Supported providers</h2>
<table>
  <thead><tr><th>Provider key</th><th>Service</th></tr></thead>
  <tbody>
    <tr><td><code>openai</code></td><td>OpenAI (api.openai.com)</td></tr>
    <tr><td><code>anthropic</code></td><td>Anthropic (api.anthropic.com)</td></tr>
    <tr><td><code>gemini</code></td><td>Google AI Studio</td></tr>
    <tr><td><code>vertex_ai</code></td><td>Google Vertex AI</td></tr>
    <tr><td><code>bedrock</code></td><td>AWS Bedrock</td></tr>
    <tr><td><code>azure</code></td><td>Azure OpenAI</td></tr>
    <tr><td><code>mistral</code></td><td>Mistral AI</td></tr>
    <tr><td><code>groq</code></td><td>Groq</td></tr>
    <tr><td><code>xai</code></td><td>xAI Grok</td></tr>
    <tr><td><code>deepseek</code></td><td>DeepSeek</td></tr>
    <tr><td><code>cohere</code></td><td>Cohere v2</td></tr>
    <tr><td><code>ollama</code></td><td>Ollama (local)</td></tr>
  </tbody>
</table>
<p>Any unrecognized provider string is treated as <strong>OpenAI-compatible</strong>. Matching is case-insensitive.</p>

<h2 id="cost">Cost accounting</h2>
<p>Costs are stored as <strong>int64 nanodollars</strong> (10<sup>-9</sup> USD) to avoid floating-point precision issues.</p>
<table>
  <thead><tr><th>Nanodollars</th><th>USD</th></tr></thead>
  <tbody>
    <tr><td><code>1,000,000,000</code></td><td>$1.00</td></tr>
    <tr><td><code>1,000,000</code></td><td>$0.001</td></tr>
    <tr><td><code>1,000</code></td><td>$0.000001</td></tr>
  </tbody>
</table>
<p>When you omit <code>cost_nanodollars</code>, Keplor computes it from the model pricing catalog and your <code>usage</code> token counts. This handles prompt caching discounts, reasoning token pricing, and audio/image tokens automatically.</p>
<p>To override: set <code>cost_nanodollars</code> to your own value. Unknown models get cost <code>0</code>.</p>

<h2 id="batch">Batch + fire-and-forget</h2>
<p>For high-throughput scenarios there are two options:</p>
<ul>
  <li><strong>Single-event fire-and-forget</strong>: <code>POST /v1/events?durable=false</code>. Server enqueues the event and returns <code>202 Accepted</code> immediately. Sub-2 ms p50 in production builds; events may be lost if the process crashes before the next batch flush.</li>
  <li><strong>Batch endpoint</strong>: <code>POST /v1/events/batch</code> with up to <strong>10,000 events</strong> per request:</li>
</ul>
<Pre code={batchExample} />
<p>Response (<code>201</code> all accepted, <code>207</code> partial):</p>
<Pre code={batchResponse} />
<p>Batch writes are fire-and-forget by default: events are validated synchronously but flushed to disk asynchronously (~50 ms). Set the <code>X-Keplor-Durable: true</code> header to await each write&rsquo;s flush confirmation.</p>

<h2 id="querying">Querying your data</h2>
<p>Check cost for a user:</p>
<Pre code={quotaExample} />
<Pre code={quotaResponse} />
<p>See the <a href="{base}/docs/api-reference">API Reference</a> for all query endpoints: events, rollups, stats, and quota.</p>

<h2 id="errors">Error handling</h2>
<table>
  <thead><tr><th>Status</th><th>Meaning</th><th>Retry?</th></tr></thead>
  <tbody>
    <tr><td><code>201</code></td><td>Created</td><td>No</td></tr>
    <tr><td><code>207</code></td><td>Partial success (batch)</td><td>Retry failed items</td></tr>
    <tr><td><code>400</code></td><td>Validation error or bad JSON</td><td>Fix request</td></tr>
    <tr><td><code>401</code></td><td>Missing or invalid API key</td><td>Fix auth</td></tr>
    <tr><td><code>408</code></td><td>Request timeout</td><td>Yes</td></tr>
    <tr><td><code>422</code></td><td>Unprocessable entity</td><td>Fix payload</td></tr>
    <tr><td><code>429</code></td><td>Rate limit exceeded</td><td>Yes, after <code>Retry-After</code> seconds</td></tr>
    <tr><td><code>500</code></td><td>Server error</td><td>Yes, with backoff</td></tr>
    <tr><td><code>503</code></td><td>Server overloaded</td><td>Yes, with backoff</td></tr>
  </tbody>
</table>
<p>All errors return <code>{'{"error": "message"}'}</code>. All responses include an <code>X-Request-Id</code> header for log correlation.</p>

<h3>Idempotency</h3>
<p>To safely retry failed requests without creating duplicates, include an <code>Idempotency-Key</code> header:</p>
<pre><code>curl -X POST http://localhost:8080/v1/events \
  -H "Idempotency-Key: my-unique-key-123" \
  -H "Content-Type: application/json" \
  -d '&#123;"model": "gpt-4o", "provider": "openai"&#125;'</code></pre>
<p>If the same key is sent again within the TTL (default 5 minutes), Keplor returns the cached response without creating a new event.</p>

<h2 id="examples">Integration examples</h2>

<h3>Python</h3>
<Pre code={pythonExample} />

<h3>Node.js</h3>
<Pre code={nodeExample} />

<h3>LiteLLM callback</h3>
<Pre code={litellmCallback} />

<h2 id="operations">Production operations</h2>

<h3>Configuration</h3>
<Pre code={'[server]\nlisten_addr = "0.0.0.0:8080"\nshutdown_timeout_secs = 25       # drain batch writer + WAL checkpoint\nrequest_timeout_secs = 30        # per-request timeout (408 on exceed)\nmax_connections = 10000          # concurrent connection limit (65000 for 50K+ users)\n\n[storage]\ndata_dir = "/var/lib/keplor"     # KeplorDB data directory\nretention_days = 90              # legacy global GC (prefer [retention] tiers)\ngc_interval_secs = 3600          # GC run frequency (0 = disabled)\nwal_checkpoint_secs = 300        # rotate active WAL into a sealed segment\nmax_db_size_mb = 0               # cap data dir size (0 = unlimited; 507 on exceed)\nflush_interval_ms = 50           # BatchWriter cadence\nrollup_loop_secs = 60            # rollup-refresh cadence\n\n[auth]\napi_keys = ["prod-svc:sk-abc"]   # simple format (empty = open mode)\n\n# Extended format with tier:\n# [[auth.api_key_entries]]\n# id = "pro-user"\n# secret = "sk-pro-key"\n# tier = "pro"\n\n[retention]\ndefault_tier = "free"\n\n[[retention.tiers]]\nname = "free"\ndays = 7\n\n[[retention.tiers]]\nname = "pro"\ndays = 90\n\n[pipeline]\nbatch_size = 64                  # use 256 for high throughput\nmax_body_bytes = 10485760        # 10 MB\nchannel_capacity = 32768         # batch writer queue depth\nflush_interval_ms = 50           # 1-10000; raise for sustained-write workloads\nflush_shards = 4                 # 1-64; parallel BatchWriter shards (pair with wal_shard_count)\nwrite_timeout_secs = 10          # 1-300; bounds worst-case durable latency\n\n[idempotency]\nenabled = true                   # dedup retries via Idempotency-Key header\nttl_secs = 300                   # 5 minute cache TTL\nmax_entries = 100000\n\n[rate_limit]\nenabled = false                  # per-key rate limiting (429 on exceed)\nrequests_per_second = 100.0\nburst = 200\n\n[pricing]\nrefresh_interval_secs = 86400    # auto-refresh LiteLLM catalog daily (0 = disabled)\n\n# [tls]                          # optional HTTPS\n# cert_path = "/etc/keplor/cert.pem"\n# key_path = "/etc/keplor/key.pem"'} />
<p>Override any field with <code>KEPLOR_&lt;SECTION&gt;_&lt;FIELD&gt;</code> environment variables. See <a href="{base}/docs/configuration">Configuration</a> for the full reference.</p>

<h3>Event archival (S3 / R2 / MinIO)</h3>
<p>For long-term retention beyond what the local data dir should hold, archive old events to any S3-compatible object store. Build with the <code>s3</code> feature and add an <code>[archive]</code> section.</p>

<p><strong>What moves:</strong> Entire events &mdash; serialized to JSONL, compressed with zstd, uploaded as files partitioned by user and day. Daily rollups stay in the local store for fast aggregation. Archived events can be merged back into <code>GET /v1/events</code> on demand via <code>?include_archived=true</code>.</p>

<h4>Cloudflare R2</h4>
<p>R2 has 10 GB free storage and zero egress fees.</p>
<Pre code={'[archive]\nbucket = "keplor-archive"\nendpoint = "https://<account-id>.r2.cloudflarestorage.com"\nregion = "auto"\naccess_key_id = "your-r2-access-key"\nsecret_access_key = "your-r2-secret-key"\nprefix = "events"\narchive_after_days = 30'} />

<h4>AWS S3</h4>
<Pre code={'[archive]\nbucket = "keplor-archive"\nendpoint = "https://s3.us-east-1.amazonaws.com"\nregion = "us-east-1"\naccess_key_id = "AKIA..."\nsecret_access_key = "..."\nprefix = "events"\narchive_after_days = 30'} />

<h4>MinIO (self-hosted)</h4>
<Pre code={'[archive]\nbucket = "keplor-archive"\nendpoint = "http://localhost:9000"\nregion = "us-east-1"\naccess_key_id = "minioadmin"\nsecret_access_key = "minioadmin"\npath_style = true    # required for MinIO\narchive_after_days = 30'} />

<p>Archival runs every <code>archive_interval_secs</code> (default 1 hour). Events are grouped by <code>(user_id, day)</code>, compressed with zstd, and uploaded. Archived events are tombstoned in KeplorDB; segment GC reclaims their disk space on the next sweep. S3 connectivity is verified at startup with a HEAD probe.</p>

<p><strong>Important:</strong> Set <code>archive_after_days</code> lower than your shortest retention tier, or GC will delete events before archival. See <a href="{base}/docs/blob-storage">Event Archival</a> for the full lifecycle.</p>

<p>Build command: <code>cargo build --release --features keplor-cli/mimalloc,keplor-cli/s3</code></p>

<h3>JSON structured logging</h3>
<Pre code="$ keplor run --json-logs" />
<p>Emits newline-delimited JSON for log aggregation (Loki, Datadog, CloudWatch).</p>

<h3>Graceful shutdown</h3>
<p>On SIGINT/SIGTERM, Keplor stops accepting connections, drains the batch writer (flushes all pending events), runs a WAL checkpoint, and exits. Drain waits up to <code>shutdown_timeout_secs</code>.</p>

<h3>Automated GC</h3>
<p>Keplor runs tiered garbage collection every <code>gc_interval_secs</code> (default: 1 hour). Each configured retention tier gets its own pass &mdash; free-tier events older than 7 days are deleted independently of pro-tier events at 90 days.</p>
<p>Set <code>gc_interval_secs = 0</code> to disable. You can still run <code>keplor gc --older-than-days N</code> manually.</p>

<h2 id="metrics">Prometheus metrics</h2>
<p>Scrape <code>GET /metrics</code> (no auth required).</p>
<table>
  <thead><tr><th>Metric</th><th>Type</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>{`keplor_events_ingested_total{provider, model, tier}`}</code></td><td>counter</td><td>Events ingested</td></tr>
    <tr><td><code>{`keplor_events_errors_total{stage, error_type}`}</code></td><td>counter</td><td>Errors by stage (validation, store, queue_full) and type</td></tr>
    <tr><td><code>{`keplor_ingest_latency_seconds{tier, provider}`}</code></td><td>histogram</td><td>Per-tier ingest latency (p50/p95/p99)</td></tr>
    <tr><td><code>keplor_ingest_duration_seconds</code></td><td>histogram</td><td>Legacy unlabelled latency histogram retained for dashboard continuity</td></tr>
    <tr><td><code>keplor_batch_flushes_total</code></td><td>counter</td><td>Batch flush operations</td></tr>
    <tr><td><code>keplor_batch_events_flushed_total</code></td><td>counter</td><td>Events flushed to KeplorDB</td></tr>
    <tr><td><code>keplor_batch_flush_errors_total</code></td><td>counter</td><td>Batch flush failures</td></tr>
    <tr><td><code>keplor_batch_queue_depth</code></td><td>gauge</td><td>Bounded mpsc channel depth (back-pressure indicator)</td></tr>
    <tr><td><code>{`keplor_storage_bytes{tier}`}</code></td><td>gauge</td><td>Bytes on disk per tier (sampled every 10 s)</td></tr>
    <tr><td><code>{`keplor_segments_total{tier}`}</code></td><td>gauge</td><td>Closed segment-file count per tier</td></tr>
    <tr><td><code>{`keplor_wal_events{tier}`}</code></td><td>gauge</td><td>Events buffered in the active WAL, not yet rotated</td></tr>
    <tr><td><code>{`keplor_storage_events{tier}`}</code></td><td>gauge</td><td>Total events across segments + WAL per tier</td></tr>
    <tr><td><code>{`keplor_gc_segments_deleted_total{tier}`}</code></td><td>counter</td><td>Segments dropped by GC per tier</td></tr>
    <tr><td><code>{`keplor_gc_bytes_freed_total{tier}`}</code></td><td>counter</td><td>Bytes reclaimed by GC per tier</td></tr>
    <tr><td><code>{`keplor_archive_chunks_total{status}`}</code></td><td>counter</td><td>Archive cycles by chunk outcome (success / fail)</td></tr>
    <tr><td><code>{`keplor_pricing_catalog_refresh_total{result}`}</code></td><td>counter</td><td>Pricing catalog refresh cycles (ok / error)</td></tr>
    <tr><td><code>keplor_pricing_catalog_age_seconds</code></td><td>gauge</td><td>Seconds since last successful catalog refresh. Alert when &gt; 2× <code>refresh_interval_secs</code>.</td></tr>
    <tr><td><code>keplor_auth_successes_total</code></td><td>counter</td><td>Successful auth attempts</td></tr>
    <tr><td><code>{`keplor_auth_failures_total{reason}`}</code></td><td>counter</td><td>Auth failures (missing or invalid)</td></tr>
  </tbody>
</table>

<h2 id="next">Next steps</h2>
<p>
  <a href="{base}/docs/api-reference">API Reference</a> &mdash; all endpoints, parameters, and response shapes.<br />
  <a href="{base}/docs/configuration">Configuration</a> &mdash; TOML config, env vars, auth keys.<br />
  <a href="{base}/docs/quickstart">Quickstart</a> &mdash; install and run Keplor in 2 minutes.
</p>
