# Key Rotation

Rotate bearer API keys without dropping in-flight requests, via SIGHUP-driven
config reload.

## Trigger

- Scheduled rotation (quarterly, or per compliance requirement).
- Suspected leak: `keplor_auth_failures_total{reason="invalid"}` spiking
  from a single source IP (credential-stuffing pattern).
- A team member with shared key access has left the org.
- Customer-reported leak via support channel.

## Verify

1. Confirm the running process has the SIGHUP handler installed — look
   in the journal for the line emitted by `sighup_reload_loop`:
   ```
   journalctl -u keplor -g "SIGHUP reload handler installed"
   ```
   If the line is absent, the server was started from in-memory config
   (tests only) and SIGHUP will be ignored. Restart with
   `keplor run --config /etc/keplor/keplor.toml` to install the handler.
2. Confirm the `[auth]` section currently in use:
   ```
   grep -A20 '^\[auth\]' /etc/keplor/keplor.toml
   ```
3. If you suspect a compromised key, identify it via the `key_id` label
   on `keplor_auth_successes_total` to scope blast radius before rotation.

## Fix

1. Generate a new secret (32 bytes, base64url):
   ```
   openssl rand -base64 32 | tr -d '=' | tr '/+' '_-'
   ```
2. Edit `/etc/keplor/keplor.toml` — append the new key alongside the old
   one so clients can migrate before the old key is removed:
   ```toml
   [auth]
   api_keys = [
     "prod-old:OLD_SECRET",     # remove in step 5
     "prod-new:NEW_SECRET",
   ]
   # or equivalently with explicit tier:
   [[auth.api_key_entries]]
   id = "prod-new"
   secret = "NEW_SECRET"
   tier = "pro"
   ```
3. Send SIGHUP to the running process:
   ```
   sudo systemctl kill -s HUP keplor
   # or:
   kill -HUP $(pidof keplor)
   ```
4. Verify the swap landed — both metric and log:
   ```
   curl -s localhost:9090/metrics | grep keplor_sighup_reloads_total
   journalctl -u keplor --since "1 minute ago" -g "api key set reloaded"
   ```
   On parse failure the running key set is unchanged and the log shows
   `SIGHUP: config parse failed — keys unchanged`. Fix the TOML and
   re-send SIGHUP.
5. Distribute the new secret to clients. Wait until the per-key success
   metric for the old `key_id` falls to zero (or your migration deadline
   passes), then remove the old entry from `keplor.toml` and SIGHUP again.

In-flight requests holding a snapshot of the old `ApiKeySet` finish
against the OLD set; new requests use the NEW set. No requests are
dropped. (See `auth.rs` — `ArcSwap<ApiKeySet>` semantics.)

## Post-mortem template

1. Timeline (UTC)
2. Detection: when, how (alert / customer / scheduled)
3. Customer impact: requests rejected, customer downtime
4. Root cause: leak vector, scheduled rotation, etc.
5. Resolution: when SIGHUP completed, when old key removed
6. Action items: rotation cadence, secret-storage hygiene, alerting
