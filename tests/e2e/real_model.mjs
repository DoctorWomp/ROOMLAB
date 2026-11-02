// GUI test with a REAL local model (T20/T21 M-path): first-run "Connect local inference",
// create a model-backed agent in the Agent Foundry, mention it, observe the streamed draft
// and the committed message labelled with the model id. Requires a running LM Studio/Ollama.
// Usage: node tests/e2e/real_model.mjs --exe <RoomLab.exe> --model <id> [--allowance 1200] [--base http://127.0.0.1:1234/v1]
import { spawn, execSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { chromium } from "playwright-core";

const arg = (k, d) => { const i = process.argv.indexOf(k); return i > 0 ? process.argv[i + 1] : d; };
const exe = arg("--exe"), model = arg("--model"), allowance = arg("--allowance", "1200"), base = arg("--base", "http://127.0.0.1:1234/v1");
if (!exe || !model) { console.error("BLOCKED: --exe and --model required"); process.exit(2); }
const shots = arg("--shots", path.join(process.cwd(), "docs", "screenshots"));
const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "roomlab-real-"));
const port = 9334;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const record = (id, ok, note = "") => { results.push({ id, ok, note }); console.log(`${ok ? "PASS" : "FAIL"} ${id}${note ? " — " + note : ""}`); };

const child = spawn(exe, [], { env: { ...process.env, ROOMLAB_DATA_DIR: dataDir, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` }, stdio: "ignore" });
try {
  for (let i = 0; i < 120; i++) { try { if ((await fetch(`http://127.0.0.1:${port}/json/version`)).ok) break; } catch {} await sleep(250); }
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  let page;
  for (let i = 0; i < 80 && !page; i++) { page = browser.contexts().flatMap((c) => c.pages())[0]; if (!page) await sleep(250); }

  await page.getByText("Welcome to RoomLab").waitFor({ timeout: 30000 });
  await page.getByRole("radio", { name: /Connect local inference/ }).click();
  await page.getByRole("textbox", { name: "Endpoint URL" }).fill(base);
  await page.getByRole("button", { name: "Test /models" }).click();
  await page.getByText(/model\(s\) reported/).waitFor({ timeout: 20000 });
  const opts = await page.locator("select").first().locator("option").allTextContents();
  record("T19 GUI lists actual model ids from /models", opts.some((o) => o.includes(model)), opts.join(" | "));
  await page.locator("select").first().selectOption(model);
  await page.locator('input[type="number"]').first().fill(String(allowance));
  await page.screenshot({ path: path.join(shots, "10_first_run_connect_lmstudio.png") });
  await page.getByRole("button", { name: "Connect and start" }).click();
  await page.getByText("Members").waitFor({ timeout: 20000 });

  // Create a model-backed agent in the Foundry.
  await page.getByRole("button", { name: "Create agent" }).click();
  await page.getByText("Agent Foundry").waitFor();
  await page.locator("label.field", { hasText: "Display name" }).locator("input").fill("Nova");
  await page.locator("label.field", { hasText: "Icon" }).locator("input").fill("✨");
  await page.locator("label.field", { hasText: "Persona" }).locator("textarea").fill("Warm, brief, curious. Answers in one or two sentences.");
  await page.locator("label.check", { hasText: "commons" }).locator("input").check();
  await page.getByRole("button", { name: "Create agent" }).last().click();
  await page.locator(".nav-item", { hasText: "Nova" }).waitFor({ timeout: 10000 });
  record("T13 agent created in Foundry (L1, local/default)", true);

  const box = page.getByRole("textbox", { name: "Message", exact: true });
  await box.fill("@Nova what's a good name for a houseplant? One sentence please.");
  await box.press("Enter");
  let sawDraft = false;
  try {
    await page.locator(".msg.draft").first().waitFor({ timeout: 240000 });
    sawDraft = true;
    await sleep(400);
    await page.screenshot({ path: path.join(shots, "11_real_model_streaming_draft.png") });
  } catch {}
  record("T20 streamed draft visible while generating", sawDraft);
  const novaMsg = page.locator("article.msg").filter({ hasText: "Nova" }).filter({ hasText: model }).first();
  await novaMsg.waitFor({ timeout: 300000 });
  await sleep(800);
  const txt = await novaMsg.innerText();
  await page.screenshot({ path: path.join(shots, "12_real_model_reply.png") });
  record("T21 real model reply committed with model id label (not DEMO)", !txt.includes("DEMO") && txt.includes(model), txt.replace(/\s+/g, " ").slice(0, 200));
  const drafts = await page.locator(".msg.draft").count();
  record("T20 draft cleared after final commit", drafts === 0);
  // Metrics tab shows measured latency.
  await page.getByRole("button", { name: /Observatory/ }).click();
  await page.getByRole("tab", { name: "Metrics" }).click();
  await sleep(1500);
  await page.screenshot({ path: path.join(shots, "13_observatory_measured_metrics.png") });
  const metrics = await page.locator("table.t").innerText();
  record("T64 measured latency shown from real calls", /first visible token/.test(metrics), metrics.replace(/\s+/g, " ").slice(0, 160));
  await browser.close().catch(() => {});
} catch (e) {
  record("real-model GUI flow", false, String(e));
} finally {
  try { execSync(`taskkill /PID ${child.pid} /T /F`); } catch {}
  fs.writeFileSync(path.join(shots, "real_model_results.json"), JSON.stringify({ when: new Date().toISOString(), model, results }, null, 2));
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} real-model GUI checks passed`);
  process.exit(failed ? 1 : 0);
}
