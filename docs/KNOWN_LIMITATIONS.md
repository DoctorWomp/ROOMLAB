# Known limitations (beta 0.1.0-beta.1)

## Not validated
- **Clean-machine test (T61/T63 manual):** the portable build was launched and GUI-tested on the
  build machine with fresh data folders, but **not** on a clean Windows 11 user account or a second PC.
- **Missing-WebView2 path:** implemented (native error box with download link, exit code 2) but not
  exercised, because WebView2 is installed here.
- **Human-factors / hover / reduced-motion (T08 manual):** CSS states and `prefers-reduced-motion`
  support exist; keyboard focus was checked automatically; no human accessibility review was done.

## Partial
- **Managed llama-server:** supervisor implemented and tested with a real `llama-server.exe` + GGUF
  (start, loopback bind, one-time key, real reply, owned-process cleanup). The release does **not**
  bundle a llama-server binary (Release A "Connected Portable"); users select their own. The GUI start
  button was not exercised by the automated GUI tests.
- **Cloud endpoints:** opt-in with explicit acknowledgement, keys in memory only (re-enter each
  session). No cost estimation (shows raw tokens, "cost unknown"). Not tested against a real cloud API.
- **Thinking models:** require the Reasoning allowance setting. Latency is dominated by hidden
  reasoning (≈30–46 s first visible token for Gemma 4 26B on this machine).
- **Story:** one bundled world (Forgotten Keep). Combat/trade/probabilistic encounters are not
  implemented. Familiars are scripted (L0). Narration is deterministic; no LLM narrator.
- **Guild org modes:** MANUAL editing works; ASSISTED/SELF_ORGANIZING are stored but agents cannot
  propose org-graph diffs yet.
- **Agent knowledge import from folders:** the safe path resolver and `import_mount_file` exist and are
  tested; there is no GUI picker for mounting a folder yet (operator can add notes in-app).
- **Playback** shows the recorded event/messages list up to a sequence number; it does not re-render
  the full chat view per frame.
- **Only the active run advances.** Queued turns in other runs wait until you switch to that run.
- **One generation at a time** (concurrency 1, by design). Several model-backed agents answer in
  sequence; with a slow thinking model, three mentioned agents can take minutes.

## Blocked
- **T51 vision relay:** no verified vision-capable route was configured/tested. Images are shown to
  the human; text-only agents get only `image_attachment_present` (T50 passes). No vision claims are made.

## Future (by design not in this beta)
- Recursive / agent-spawned room participants; S2 OS sandbox and S3 VM workers; image generation;
  multi-user/remote rooms; signed installer; macOS/Linux.

## Measurement caveats
- Benchmark numbers are from one machine (RTX 4070 Ti SUPER, 94 GB RAM, LM Studio) and small sample
  sizes; p95 with n<20 is indicative only. RAM/VRAM of external providers is not measured by RoomLab.
- Token counts shown before a call are estimates (chars/4) and labelled as such; after a call the
  provider-reported usage is used when available.
