# Security and epistemic boundaries

| Boundary | Enforcement (file) | Test evidence |
|---|---|---|
| Model output is untrusted data | `actions.rs` parse → `apply_in_tx` (schema, actor ACTIVE, membership, capability, state) | T23, T36, T56 |
| Only delivered observations reach a prompt | `comms.rs` delivery → `observations`; `context.rs` joins reader's observations only | T04, T14, T28 |
| Knowledge ACL deny-by-default, filtered inside SQL | `knowledge.rs` `AUTH_SQL` | T28, T29, T38 |
| Unauthorized == nonexistent | `knowledge::read_resource` | T38 |
| World truth / sealed keys never to agents | `KnowledgeKind::WorldTruth`, `HiddenTestKey` + `Sealed/OperatorOnly` excluded from `AUTH_SQL` | T29, T42, T49 |
| Narrator cannot add facts | `narrative::validate_grounding`; deterministic renderer receives observable facts only | T43 |
| Spawn is a request; depth 1; caps; explicit handoff only | `spawn.rs` | T30–T35, T60 |
| Org rank ≠ permissions | capabilities from profile `allowed_actions`; org edges only gate ASSIGN_TASK routing | T35 |
| No shell/code/network/file tools (S1 only) | `tools.rs` allowlist; unknown ⇒ UNSAFE_TOOL_DISABLED | T56 |
| Path traversal, symlinks, junctions | `knowledge::safe_join` (component walk, reparse-point check, canonical prefix) | T37 |
| Transactions roll back fully on rejection | `Engine::tx` + `submit_action` two-phase journaling | T60 |
| Backend-enforced operator vs participant view | `Engine::view_role`, `require_operator()` on truth commands | GUI smoke, T55 |
| No cloud call without acknowledgement | `providers::authorize_endpoint` before any request | T24 |
| Secrets never in prompts/exports/settings | `ProviderConfig.api_key` `serde(skip)`, memory store, `redact.rs` | T12, T58 |
| Managed sidecar loopback-only + ephemeral key; kill only own child | `managed_llama.rs` | T57, T62 |
| Rich text cannot execute or load remote content | `src/lib/text.tsx` (no innerHTML, no links); CSP blocks remote images/frames | T07 |
| Budgets and loop caps | `scheduler.rs` chain depth, turn/token budgets checked at enqueue and claim | T17, T25 |
| Resource limits | message 4000 chars, image 8 MB / 8192 px with magic sniffing, doc 200 KB, tool rate limit, one queued job per agent | T59 |

## Not provided in this beta
- S2 OS sandbox and S3 VM workers (no executable tools exist to sandbox).
- OS credential storage for cloud keys (keys are session-memory only).
- Signed binaries / installer; clean-machine validation (see KNOWN_LIMITATIONS).
- Formal red-team of prompt-injection against real models: the tests prove injected text cannot change
  permissions or reach trusted segments; they do not measure how often a model *follows* injected prose.
