<script lang="ts">
  import Pre from '$lib/components/Pre.svelte';

  const ingestResponse = `{
  "id": "01J5XQKR4M2E3N8V7P6Y1WDCBA",
  "cost_nanodollars": 6250000,
  "model": "gpt-4o",
  "provider": "openai"
}`;

  const fireForgetResponse = `// 202 Accepted
{
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
  }],
  "has_more": false
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
  }],
  "has_more": false
}`;

  const deleteResponse = `{
  "events_deleted": 1234,
  "segments_dropped": 3
}`;

  const healthResponse = `{
  "status": "ok",
  "version": "0.1.0",
  "db": "connected",
  "queue_depth": 0,
  "queue_capacity": 32768,
  "queue_utilization_pct": 0
}`;

  const errorResponse = `{"error": "validation: model must not be empty"}`;

  const metricsExample = `# TYPE keplor_events_ingested_total counter
keplor_events_ingested_total{provider="openai",model="gpt-4o",tier="pro"} 42

# TYPE keplor_segments_total gauge
keplor_segments_total{tier="free"} 9084

# TYPE keplor_storage_bytes gauge
keplor_storage_bytes{tier="free"} 41508725

# TYPE keplor_pricing_catalog_age_seconds gauge
keplor_pricing_catalog_age_seconds 14392`;
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
<p>Ingest a single event. Two modes: durable (default, awaits flush) and fire-and-forget (returns immediately). Supports <code>Idempotency-Key</code> header to prevent duplicate creation on retries.</p>

<h3>Query parameters</h3>
<table>
  <thead><tr><th>Param</th><th>Type</th><th>Default</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>durable</code></td><td>bool</td><td><code>true</code></td><td>When <code>false</code> the server enqueues the event and returns <code>202 Accepted</code> immediately (no await-flush). p50 drops ~10×; events may be lost if the process crashes before the next batch flush.</td></tr>
  </tbody>
</table>

<h3>Request body</h3>
<p>Schema is strict (<code>deny_unknown_fields</code>) &mdash; sending an unknown key returns <code>422 Unprocessable Entity</code>.</p>
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
    <tr><td><code>error.kind</code></td><td>string</td><td>no</td><td><code>rate_limited</code>, <code>auth_failed</code>, etc.</td></tr>
    <tr><td><code>metadata</code></td><td>object</td><td>no</td><td>Arbitrary JSON (queryable via <code>user_tag</code> / <code>session_tag</code>; capped at 64 KB).</td></tr>
  </tbody>
</table>

<h3>Response <code>201 Created</code> (durable)</h3>
<Pre code={ingestResponse} />

<h3>Response <code>202 Accepted</code> (fire-and-forget, <code>?durable=false</code>)</h3>
<Pre code={fireForgetResponse} />

<h2 id="batch"><span class="method method-post">POST</span> /v1/events/batch</h2>
<p>Ingest up to <strong>10,000 events</strong>. Fire-and-forget by default. Set <code>X-Keplor-Durable: true</code> header to await flush confirmation for each event.</p>

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
    <tr><td><code>include_archived</code></td><td>bool</td><td>When <code>true</code> the server fetches every archive manifest overlapping <code>[from, to]</code> for the requested user, decompresses + parses the JSONL chunks, and merges with live events sorted by <code>(ts_ns desc, id desc)</code>. Currently uncached &mdash; each request pays the per-manifest S3 round-trip. Suitable for backfill / audit, not hot dashboards.</td></tr>
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
    <tr><td><code>limit</code></td><td>u32</td><td>no</td><td>Max rows (default 100, max 1000)</td></tr>
    <tr><td><code>offset</code></td><td>u32</td><td>no</td><td>Offset for pagination (default 0)</td></tr>
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
    <tr><td><code>limit</code></td><td>u32</td><td>no</td><td>Max rows (default 100, max 1000)</td></tr>
    <tr><td><code>offset</code></td><td>u32</td><td>no</td><td>Offset for pagination (default 0)</td></tr>
  </tbody>
</table>

<h3>Response <code>200 OK</code></h3>
<Pre code={statsResponse} />

<h2 id="delete-single"><span class="method method-delete">DELETE</span> /v1/events/:id</h2>
<p>Delete a single event by ID.</p>
<p>Returns <code>204 No Content</code> if deleted, <code>404 Not Found</code> if the event does not exist.</p>

<h2 id="delete-bulk"><span class="method method-delete">DELETE</span> /v1/events?older_than_days=N</h2>
<p>Bulk delete events older than N days. Equivalent to <code>keplor gc</code> via HTTP. <code>older_than_days</code> must be greater than 0.</p>
<h3>Response <code>200 OK</code></h3>
<Pre code={deleteResponse} />

<h2 id="export"><span class="method method-get">GET</span> /v1/events/export</h2>
<p>Stream all matching events as JSON Lines (<code>application/x-ndjson</code>). Accepts the same filter parameters as <code>GET /v1/events</code> but with no result-set limit. Each line is one JSON event object.</p>

<h2 id="health"><span class="method method-get">GET</span> /health</h2>
<p>Liveness probe. Returns <code>200</code> when healthy, <code>503</code> when degraded.</p>
<Pre code={healthResponse} />

<h2 id="metrics"><span class="method method-get">GET</span> /metrics</h2>
<p>Prometheus text exposition format.</p>
<Pre code={metricsExample} />

<h2 id="headers">Request/Response headers</h2>
<table>
  <thead><tr><th>Header</th><th>Direction</th><th>Description</th></tr></thead>
  <tbody>
    <tr><td><code>Authorization</code></td><td>Request</td><td><code>Bearer &lt;secret&gt;</code> (required when keys configured)</td></tr>
    <tr><td><code>Idempotency-Key</code></td><td>Request</td><td>Optional. Prevents duplicate event creation on retries. Cached for 5 min (configurable).</td></tr>
    <tr><td><code>X-Keplor-Durable</code></td><td>Request</td><td>Set to <code>true</code> on batch endpoint to await flush confirmation. Default: fire-and-forget.</td></tr>
    <tr><td><code>X-Request-Id</code></td><td>Both</td><td>Echoed if sent; otherwise Keplor generates a ULID and returns it.</td></tr>
    <tr><td><code>Retry-After</code></td><td>Response</td><td>Seconds until rate limit resets (returned with <code>429</code>).</td></tr>
  </tbody>
</table>

<h2 id="errors">Error responses</h2>
<Pre code={errorResponse} />
<table>
  <thead><tr><th>Status</th><th>When</th><th>Retry?</th></tr></thead>
  <tbody>
    <tr><td><code>204</code></td><td>Event deleted successfully</td><td>No</td></tr>
    <tr><td><code>400</code></td><td>Validation error, bad JSON, invalid timestamp</td><td>No</td></tr>
    <tr><td><code>401</code></td><td>Missing or invalid API key</td><td>No</td></tr>
    <tr><td><code>404</code></td><td>Event not found (DELETE)</td><td>No</td></tr>
    <tr><td><code>408</code></td><td>Request exceeded <code>request_timeout_secs</code></td><td>Yes</td></tr>
    <tr><td><code>415</code></td><td><code>Content-Type</code> not <code>application/json</code></td><td>No (fix client)</td></tr>
    <tr><td><code>422</code></td><td>Unprocessable: malformed JSON, unknown field (schema is strict <code>deny_unknown_fields</code>), or unknown provider</td><td>No</td></tr>
    <tr><td><code>429</code></td><td>Per-key rate limit exceeded</td><td>Yes (after <code>Retry-After</code>)</td></tr>
    <tr><td><code>500</code></td><td>Storage failure, write timeout</td><td>Yes (with backoff)</td></tr>
    <tr><td><code>503</code></td><td>Batch writer overloaded (back-pressure)</td><td>Yes (with backoff)</td></tr>
    <tr><td><code>507</code></td><td>Data dir size limit exceeded</td><td>Yes (run GC or increase limit)</td></tr>
  </tbody>
</table>
