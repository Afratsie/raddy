---
title: Access log
description: The structured JSON access log written by raddy with --access-log.
---

Pass `--access-log <file>` to append one structured JSON object per completed
request to the given file:

```bash
raddy run -c Raddyfile --access-log /var/log/raddy/access.jsonl
```

Each line is a single JSON object (JSON Lines). Appending (never truncating),
so you can rotate the file out from under raddy and it keeps writing to the new
path.

## Fields

| Field | Type | Meaning |
|---|---|---|
| `ts` | integer | Request start time as **epoch milliseconds** |
| `client` | string | The **real client IP** — the TCP peer, or the untrusted `X-Forwarded-For` entry when `trusted_proxies` is configured (see [Trusted proxies](../../config/trusted-proxies/)) |
| `method` | string | HTTP method (`GET`, `POST`, …) |
| `path` | string | The request path (query string excluded) |
| `status` | integer | HTTP response status code |
| `duration_ms` | integer | Request duration in milliseconds |

```json
{"ts":1760850000123,"client":"203.0.113.7","method":"GET","path":"/","status":200,"duration_ms":4}
```

The `client` field is the real client IP per the [trust model](../../config/trusted-proxies/):
without `trusted_proxies`, it is the TCP peer; with it configured, it is the
rightmost untrusted `X-Forwarded-For` entry.
