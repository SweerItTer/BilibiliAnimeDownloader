# Offline API response replay

## 1. Scope / trigger

Use explicit local response data to inspect the actual production GUI while external requests and real accounts/tasks remain isolated. Replay replaces API JSON GET data only; Core continues owning parsing, preview/access classification, part identity, catalogs, strict quality selection and planning. This is synthetic-data acceptance, not live-service or entitlement qualification.

## 2. Signatures

`OfflineApiReplay::load(&Path) -> Result<OfflineApiReplay>` is exported from `bili_core`; the cloneable opaque value has redacted Debug. `CoreConfig.api_replay: Option<OfflineApiReplay>` defaults to None in `CoreConfig::new`. Public struct-literal consumers must provide the new field and matched SDK consumers must rebuild. `Api::with_credentials` retains replay through profile-scoped operations.

## 3. Contracts

The file is a regular local JSON file, bounded to eight MiB, schema version 1 and 1..512 distinct entries:

```json
{"schema_version":1,"responses":[{"path":"/pgc/view/web/season","query":{"season_id":"9001"},"body":{"code":0,"result":{}},"delay_ms":0}]}
```

The example shows the transport envelope only; actual body schemas still need all fields required by the owning Core parser. Query values are strings. Match HTTPS `api.bilibili.com`, default port, no userinfo/fragment, path and all decoded stable query pairs. Reject duplicate query names and duplicate response identities; ignore only `wts`/`w_rid` on `/x/player/wbi/playurl` and `/x/web-interface/wbi/search/type`. Search fixtures also supply synthetic device initialization via `/x/frontend/finger/spi`. A response delay is optional and bounded to 0..5000 ms; its asynchronous sleep cancels when the request future is dropped. Never log file contents, query values, URLs or remote message text.

`Core::new` requires MemoryOnly credential policy with replay and rejects the combination with Remember before opening state. Replay forces the existing API and anonymous media reqwest builders through an explicit closed-loopback proxy, regardless of inherited proxy bypasses. Auth, covers, verification and sidecars reuse those clients; a shell's independently constructed updater must be disabled separately. An unmatched JSON request returns a safe error and never reaches the HTTP branch. No media completion, file result, account success or GUI Outcome is synthesized.

## 4. Validation / error matrix

| Input | Observable result |
| --- | --- |
| Missing, malformed, oversized or unsupported-schema file | Safe InvalidInput load error; never disable replay silently |
| Duplicate entry/query or invalid host/path/delay | Safe InvalidInput; no response contents in errors |
| Missing complete request key | Safe error; no HTTP fallback |
| Profile-scoped resolve/catalog/plan | Same immutable replay and normal Core business rules |
| Persistent auth requested with replay | Rejected before opening state |
| Unreplayed cover/auth/media operation | Existing HTTP client cannot connect to an external destination |
| Cancel during a delayed response | Future drops; no detached response task or late side effect |

## 5. Good / base / bad

Good: a synthetic 24-episode season is parsed through Core and an explicit unsupported quality remains blocked. Base: None keeps the existing live-capable client construction. Bad: a missing fixture falls through to a service, the GUI fabricates PreparedItem/Outcome objects, or a blocked transfer is reported completed.

## 6. Tests required

Owning nonvisual tests cover load limits/schema/duplicates, complete canonical matching, WBI volatile fields only, redacted errors/Debug and cancellation. Real Core integration resolves original parts, loads catalogs and strictly plans an obtainable quality while rejecting an unavailable tier and missing response. A reachable loopback control proves API/media HTTP and HTTPS clients cannot bypass their explicit replay proxy. GUI inspection must still use the visible production executable; no separate fixture GUI or offscreen renderer qualifies it.

## 7. Wrong versus correct

Wrong: inject a fake successful GUI Prepared response or completed job. Correct: explicitly configure immutable API response data, keep App/Bridge/Core unchanged, observe the real visible UI and report the synthetic-data boundary separately from live-service coverage.
