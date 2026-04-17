<script lang="ts">
  import Pre from '$lib/components/Pre.svelte';

  const ingestResponse = `{
  "id": "01J5XQKR4M2E3N8V7P6Y1WDCBA",
  "cost_nanodollars": 6250000,
  "model": "gpt-4o",
  "provider": "openai"
}`;

  const batchRequest = `{"events": [{"model": "gpt-4o", "provider": "openai", ...}, ...]}`;

  const batchResponse = `{
  "results": [
    {"id": "01J5X...", "cost_nanodollars": 100, ...},
    {"error": "validation: model must not be empty"}
  ],
  "accepted": 1,
  "rejected": 1
}`;

  const queryResponse = `{
  "events": [{
    "id": "01J5XQKR...",
    "timestamp": 1700000000000000000,
    "model": "gpt-4o",
    "provider": "openai",
    "usage": {"input_tokens": 500, "output_tokens": 200, ...},
    "cost_nanodollars": 6250000,
    "latency_total_ms": 1200,
    "streaming": false
  }],
  "cursor": 1700000000000000000,
  "has_more": false
}`;

  const quotaResponse = `{
  "cost_nanodollars": 48250000,
  "event_count": 137
}`;

  const rollupsResponse = `{
  "rollups": [{
    "day": 1700006400,
    "user_id": "user-123",
    "api_key_id": "key-abc",
    "provider": "openai",
    "model": "gpt-4o",
    "event_count": 42,
    "error_count": 1,
    "input_tokens": 21000,
    "output_tokens": 8400,
    "cache_read_input_tokens": 5000,
    "cache_creation_input_tokens": 0,
    "cost_nanodollars": 18750000
  }]
}`;

  const statsResponse = `{
  "stats": [{
    "provider": "openai",
    "model": "gpt-4o",
    "event_count": 137,
    "error_count": 3,
    "input_tokens": 68500,
    "output_tokens": 27400,
    "cache_read_input_tokens": 12000,
    "cache_creation_input_tokens": 0,
    "cost_nanodollars": 48250000
  }]
}`;

  const healthResponse = `{"status": "ok", "version": "0.1.0", "db": "connected"}`;

  const errorResponse = `{"error": "validation: model must not be empty"}`;

  const metricsExample = `# TYPE keplor_events_ingested_total counter
keplor_events_ingested_total{provider="openai"} 42`;
</script>

<svelte:head>
  <title>API Reference - Keplor</title>
</svelte:head>

<h1 class="text-3xl font-bold mb-2">API Reference</h1>
<p class="text-lg text-text-muted mb-8">Base URL: <code>http://localhost:8080</code></p>

<h2 id="auth">Authentication</h2>
<p>When API keys are configured, all <code>/v1/*</code> endpoints require a Bearer token:</p>
<pre><code>Authorization: Bearer your-api-key-here</code></pre>
<p>When no keys are configured (default), authentication is disabled. <code>/health</code> and <code>/metrics</code> are always public.</p>

<h2 id="ingest"><span class="method method-post">POST</span> /v1/events</h2>
<p>Ingest a single event. Waits for durable storage. Times out after 10 seconds.</p>

<h3>Request body</h3>
<table>
  <thead><tr><th>Field</th><th>Type</th><th>Required</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>model</code></td><td>string</td><td>yes</td><td>Model name (e.g. <code>"gpt-4o"</code>)</td></tr>
    <tr><td><code>provider</code></td><td>string</td><td>yes</td><td>Provider key (e.g. <code>"openai"</code>)</td></tr>
    <tr><td><code>usage.input_tokens</code></td><td>u32</td><td>no</td><td>Input tokens</td></tr>
    <tr><td><code>usage.output_tokens</code></td><td>u32</td><td>no</td><td>Output tokens</td></tr>
    <tr><td><code>usage.cache_read_input_tokens</code></td><td>u32</td><td>no</td><td>Cached prompt tokens</td></tr>
    <tr><td><code>usage.reasoning_tokens</code></td><td>u32</td><td>no</td><td>Reasoning/thinking tokens</td></tr>
    <tr><td><code>latency.ttft_ms</code></td><td>u32</td><td>no</td><td>Time to first token (ms)</td></tr>
    <tr><td><code>latency.total_ms</code></td><td>u32</td><td>no</td><td>End-to-end latency (ms)</td></tr>
    <tr><td><code>timestamp</code></td><td>string | i64</td><td>no</td><td>ISO 8601 or epoch nanos</td></tr>
    <tr><td><code>user_id</code></td><td>string</td><td>no</td><td>User identifier</td></tr>
    <tr><td><code>api_key_id</code></td><td>string</td><td>no</td><td>API key for attribution</td></tr>
    <tr><td><code>org_id</code></td><td>string</td><td>no</td><td>Organization for rollups</td></tr>
    <tr><td><code>source</code></td><td>string</td><td>no</td><td>Sending system</td></tr>
    <tr><td><code>http_status</code></td><td>u16</td><td>no</td><td>Upstream status code</td></tr>
    <tr><td><code>flags.streaming</code></td><td>bool</td><td>no</td><td>Streamed response</td></tr>
    <tr><td><code>flags.tool_calls</code></td><td>bool</td><td>no</td><td>Tool calls present</td></tr>
    <tr><td><code>request_body</code></td><td>object</td><td>no</td><td>Full request JSON (compressed)</td></tr>
    <tr><td><code>response_body</code></td><td>object</td><td>no</td><td>Full response JSON (compressed)</td></tr>
    <tr><td><code>error.kind</code></td><td>string</td><td>no</td><td><code>rate_limited</code>, <code>auth_failed</code>, etc.</td></tr>
  </tbody>
</table>

<h3>Response <code>201 Created</code></h3>
<Pre code={ingestResponse} />

<h2 id="batch"><span class="method method-post">POST</span> /v1/events/batch</h2>
<p>Ingest up to <strong>10,000 events</strong>. Fire-and-forget writes for throughput.</p>

<h3>Request</h3>
<Pre code={batchRequest} />

<h3>Response <code>201</code> or <code>207 Multi-Status</code></h3>
<Pre code={batchResponse} />

<h2 id="query"><span class="method method-get">GET</span> /v1/events</h2>
<p>Query events with filtering and cursor-based pagination.</p>

<h3>Query parameters</h3>
<table>
  <thead><tr><th>Param</th><th>Type</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>user_id</code></td><td>string</td><td>Filter by user</td></tr>
    <tr><td><code>model</code></td><td>string</td><td>Filter by model</td></tr>
    <tr><td><code>provider</code></td><td>string</td><td>Filter by provider</td></tr>
    <tr><td><code>source</code></td><td>string</td><td>Filter by source</td></tr>
    <tr><td><code>from</code></td><td>i64</td><td>After this epoch ns</td></tr>
    <tr><td><code>to</code></td><td>i64</td><td>Before this epoch ns</td></tr>
    <tr><td><code>limit</code></td><td>u32</td><td>Max results (default 50, max 1000)</td></tr>
    <tr><td><code>cursor</code></td><td>i64</td><td>Pagination cursor</td></tr>
  </tbody>
</table>

<h3>Response <code>200 OK</code></h3>
<Pre code={queryResponse} />

<h2 id="quota"><span class="method method-get">GET</span> /v1/quota</h2>
<p>Real-time cost and event count from the event table. At least one of <code>user_id</code> or <code>api_key_id</code> is required.</p>

<h3>Query parameters</h3>
<table>
  <thead><tr><th>Param</th><th>Type</th><th>Required</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>user_id</code></td><td>string</td><td>no*</td><td>Filter by user</td></tr>
    <tr><td><code>api_key_id</code></td><td>string</td><td>no*</td><td>Filter by API key</td></tr>
    <tr><td><code>from</code></td><td>i64</td><td>yes</td><td>Events on or after this epoch ns</td></tr>
  </tbody>
</table>
<p class="text-sm text-text-muted">* At least one of <code>user_id</code> or <code>api_key_id</code> must be provided.</p>

<h3>Response <code>200 OK</code></h3>
<Pre code={quotaResponse} />

<h2 id="rollups"><span class="method method-get">GET</span> /v1/rollups</h2>
<p>Pre-aggregated daily rollup rows broken down by provider and model. The background rollup task refreshes every 60 seconds.</p>

<h3>Query parameters</h3>
<table>
  <thead><tr><th>Param</th><th>Type</th><th>Required</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>user_id</code></td><td>string</td><td>no</td><td>Filter by user</td></tr>
    <tr><td><code>api_key_id</code></td><td>string</td><td>no</td><td>Filter by API key</td></tr>
    <tr><td><code>from</code></td><td>i64</td><td>yes</td><td>Start epoch ns (converted to day boundary)</td></tr>
    <tr><td><code>to</code></td><td>i64</td><td>yes</td><td>End epoch ns (converted to day boundary)</td></tr>
  </tbody>
</table>

<h3>Response <code>200 OK</code></h3>
<Pre code={rollupsResponse} />

<h2 id="stats"><span class="method method-get">GET</span> /v1/stats</h2>
<p>Aggregated period statistics summed from daily rollups. Optionally group by model for per-model cost breakdowns.</p>

<h3>Query parameters</h3>
<table>
  <thead><tr><th>Param</th><th>Type</th><th>Required</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>user_id</code></td><td>string</td><td>no</td><td>Filter by user</td></tr>
    <tr><td><code>api_key_id</code></td><td>string</td><td>no</td><td>Filter by API key</td></tr>
    <tr><td><code>from</code></td><td>i64</td><td>yes</td><td>Start epoch ns</td></tr>
    <tr><td><code>to</code></td><td>i64</td><td>yes</td><td>End epoch ns</td></tr>
    <tr><td><code>provider</code></td><td>string</td><td>no</td><td>Filter by provider</td></tr>
    <tr><td><code>group_by</code></td><td>string</td><td>no</td><td>Set to <code>"model"</code> to group by provider + model</td></tr>
  </tbody>
</table>

<h3>Response <code>200 OK</code></h3>
<Pre code={statsResponse} />

<h2 id="health"><span class="method method-get">GET</span> /health</h2>
<p>Liveness probe. Returns <code>200</code> when healthy, <code>503</code> when degraded.</p>
<Pre code={healthResponse} />

<h2 id="metrics"><span class="method method-get">GET</span> /metrics</h2>
<p>Prometheus text exposition format.</p>
<Pre code={metricsExample} />

<h2 id="errors">Error responses</h2>
<Pre code={errorResponse} />
<table>
  <thead><tr><th>Status</th><th>When</th></tr></thead>
  <tbody>
    <tr><td><code>400</code></td><td>Validation error, bad JSON, invalid timestamp</td></tr>
    <tr><td><code>401</code></td><td>Missing or invalid API key</td></tr>
    <tr><td><code>422</code></td><td>Unknown provider</td></tr>
    <tr><td><code>500</code></td><td>Storage failure, write timeout</td></tr>
  </tbody>
</table>
