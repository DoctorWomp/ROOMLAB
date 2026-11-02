"""Optional analysis helper (not a runtime dependency).

Usage: python tools/export_run.py <run>.roomlab.json
Prints message counts per author, rejected actions by reason code, and measured inference latency
(operator exports only include provider_requests). Uses only the standard library.
"""
import collections
import json
import statistics
import sys


def pct(v, p):
    s = sorted(v)
    return s[min(len(s) - 1, round(p * (len(s) - 1)))] if s else None


def main(path):
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    b = json.load(open(path, encoding="utf-8"))
    if b.get("format") != "roomlab-run-export-v1":
        sys.exit("not a RoomLab export")
    m = b["manifest"]
    print(f"run {m['run_id']} · {m['name']} · {m['session_type']} · seed {m['seed']} · export {b['export_kind']}")
    by_author = collections.Counter(x["author_id"] for x in b["messages"])
    print("messages by author:", dict(by_author))
    rej = collections.Counter()
    for e in b["events"]:
        if e["event_type"] == "ACTION_REJECTED":
            rej[json.loads(e["payload_json"]).get("reason_code")] += 1
    print("rejected actions:", dict(rej) or "none visible in this export")
    reqs = b.get("provider_requests", [])
    tot = [r["wall_end_ms"] - r["wall_start_ms"] for r in reqs if r.get("wall_end_ms")]
    ft = [r["first_token_ms"] for r in reqs if r.get("first_token_ms") is not None]
    if tot:
        print(f"inference calls n={len(tot)} total p50={pct(tot, .5)} p95={pct(tot, .95)} mean={statistics.mean(tot):.0f} ms; first-token p50={pct(ft, .5)} ms")
    else:
        print("no provider calls in this export (Demo run or public export)")
    if b.get("turn_log"):
        out = collections.Counter((t["agent_id"], t["outcome"]) for t in b["turn_log"])
        for (a, o), n in sorted(out.items()):
            print(f"  {a:<24} {o:<22} {n}")


if __name__ == "__main__":
    main(sys.argv[1])
