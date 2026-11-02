# RoomLab measured benchmark

Endpoint: http://127.0.0.1:1234/v1
Model: gemma4-26b-a4b-uncensored-hauhaucs-balanced
Repeats per condition: 3
Reasoning allowance: 1200 tokens
Date (UTC): 2026-11-02T22:56:31.642Z
Scenario: Commons, one human message mentioning every agent; each agent makes one real call (visible output cap 200 tokens + reasoning allowance, temperature 0.7). Concurrency 1 (default scheduler).

| agents | repeat | calls ok | calls failed | wall for all turns (ms) |
|---|---|---|---|---|
| 1 | 1 | 1 | 0 | 50394 |
| 1 | 2 | 1 | 0 | 17948 |
| 1 | 3 | 1 | 0 | 25900 |
| 3 | 1 | 3 | 0 | 106603 |
| 3 | 2 | 3 | 0 | 102666 |
| 3 | 3 | 3 | 0 | 103035 |
| 5 | 1 | 5 | 0 | 168785 |
| 5 | 2 | 5 | 0 | 147424 |
| 5 | 3 | 5 | 0 | 215338 |

## Per-condition latency (actual samples)

| agents | n calls | first-token p50 | first-token p95 | total p50 | total p95 | queue-wait p50 | queue-wait p95 | completion tokens p50 |
|---|---|---|---|---|---|---|---|---|
| 1 | 3 | 24666 | 48468 | 25898 | 50392 | 2 | 2 | 512 |
| 3 | 9 | 33276 | 45645 | 35124 | 47482 | 30895 | 67672 | 654 |
| 5 | 15 | 32560 | 41224 | 33657 | 56548 | 72629 | 126236 | 716 |

All times in milliseconds, measured by RoomLab around real HTTP calls. Percentiles use nearest-rank on the samples listed; with fewer than ~20 samples p95 is effectively the max and should be read as indicative only. Queue wait = time from job enqueue to call start (includes earlier agents' turns because concurrency is 1). RAM/VRAM are not measured by this tool.
