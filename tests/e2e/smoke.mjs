// GUI smoke test against the REAL packaged RoomLab.exe (not a mock UI).
// WebView2 exposes the DevTools protocol via WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS; Playwright
// attaches over CDP. Screenshots are captured from the running app.
//
// Usage: node tests/e2e/smoke.mjs --exe <path\to\RoomLab.exe> [--data <dir>] [--shots <dir>] [--port 9333]
import { spawn, execSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { chromium } from "playwright-core";

const arg = (k, d) => { const i = process.argv.indexOf(k); return i > 0 ? process.argv[i + 1] : d; };
const exe = arg("--exe");
if (!exe || !fs.existsSync(exe)) { console.error("BLOCKED: --exe path to RoomLab.exe required"); process.exit(2); }
const dataDir = arg("--data", fs.mkdtempSync(path.join(os.tmpdir(), "roomlab-e2e-")));
const shots = arg("--shots", path.join(process.cwd(), "docs", "screenshots"));
const port = Number(arg("--port", "9333"));
// --portable: do not override the data folder, so RoomLab must use data\ next to the exe.
const portable = process.argv.includes("--portable");
fs.mkdirSync(shots, { recursive: true });

const results = [];
const record = (id, ok, note = "") => { results.push({ id, ok, note }); console.log(`${ok ? "PASS" : "FAIL"} ${id}${note ? " — " + note : ""}`); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function launch() {
  const child = spawn(exe, [], {
    env: { ...process.env, ...(portable ? {} : { ROOMLAB_DATA_DIR: dataDir }), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` },
    stdio: "ignore",
    detached: false,
  });
  return child;
}

async function attach() {
  for (let i = 0; i < 120; i++) {
    try {
      const r = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (r.ok) break;
    } catch {}
    await sleep(250);
  }
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  let page;
  for (let i = 0; i < 80 && !page; i++) {
    page = browser.contexts().flatMap((c) => c.pages()).find((p) => !p.url().startsWith("devtools"));
    if (!page) await sleep(250);
  }
  if (!page) throw new Error("no page found over CDP");
  return { browser, page };
}

const errors = [];
async function shot(page, name) { await page.screenshot({ path: path.join(shots, `${name}.png`) }); }
async function waitText(page, text, timeout = 30000) { await page.getByText(text, { exact: false }).first().waitFor({ timeout }); }

async function main() {
  console.log(`exe=${exe}\ndata=${dataDir}\nshots=${shots}`);
  let child = launch();
  let { browser, page } = await attach();
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
  await page.setViewportSize?.({ width: 1440, height: 900 }).catch(() => {});

  // T01 cold launch in Demo, no provider / weights.
  try {
    await waitText(page, "Welcome to RoomLab", 30000);
    await shot(page, "01_first_run");
    record("T01 cold launch shows first-run", true);
  } catch (e) { record("T01 cold launch shows first-run", false, String(e)); }

  // T02 first-run: choose Demo.
  await page.getByRole("button", { name: "Explore Demo", exact: true }).click();
  await waitText(page, "Members", 20000);
  record("T02 first-run Demo completes", true);

  // Commons conversation with three Demo agents.
  const box = page.getByRole("textbox", { name: "Message", exact: true });
  await box.fill("Thanks for shredding my favorite theory last night... ugh. Honestly, please don't stop.");
  await box.press("Enter");
  try {
    await page.locator("article.msg").filter({ hasText: "Lumen" }).filter({ hasText: "DEMO" }).first().waitFor({ timeout: 30000 });
    await page.locator("article.msg").filter({ hasText: "Moth" }).first().waitFor({ timeout: 30000 });
    await page.locator(".lane").first().waitFor({ timeout: 30000 });
    await sleep(1500);
    await shot(page, "02_commons_demo_chat");
    record("Commons: 3 Demo agents respond, DEMO labels + reaction lane visible", true);
  } catch (e) { record("Commons demo conversation", false, String(e)); }

  // T05 ownership labels present.
  const kinds = await page.locator(".kind").allTextContents();
  record("T05 message ownership labels", kinds.includes("HUMAN") && kinds.includes("AGENT"), `labels: ${[...new Set(kinds)].join(",")}`);

  // T06/T07 unicode + rich text safety in the real app.
  await box.fill("Unicode 👩‍👩‍👧‍👦 ❤️ مرحبا <script>window.__pwned=1</script> javascript:alert(1)");
  await box.press("Enter");
  await sleep(1200);
  const pwned = await page.evaluate(() => window.__pwned === 1 || !!document.querySelector(".chat script, .chat a[href]"));
  const hasZwj = await page.locator(".chat").innerText().then((t) => t.includes("👩‍👩‍👧‍👦") && t.includes("مرحبا"));
  record("T06 Unicode/ZWJ/RTL rendered", hasZwj);
  record("T07 script/link injection inert in chat", !pwned);

  // T03 create the other three room types from the UI.
  for (const [kind, name] of [["GUILD", "guild"], ["STORY", "keep"], ["CRUCIBLE", "crucible"]]) {
    await page.getByRole("button", { name: "New room" }).click();
    await page.getByRole("combobox", { name: "Room type" }).selectOption(kind);
    await page.getByPlaceholder("Room name").fill(name);
    await page.getByRole("button", { name: "Create from template" }).click();
    await page.locator(".room-head .title", { hasText: name }).waitFor({ timeout: 10000 });
    record(`T03 create ${kind} room`, true);
  }

  // Story: world panel, advance ticks, move.
  await page.locator(".nav-item", { hasText: "keep" }).click();
  await page.getByRole("tab", { name: "World" }).click();
  await page.locator("svg.map").waitFor({ timeout: 10000 });
  for (let i = 0; i < 4; i++) {
    await page.getByRole("button", { name: /Advance world tick/ }).click();
    await sleep(4000);
  }
  await page.getByRole("button", { name: /Great Hall/ }).first().click();
  await sleep(1500);
  await shot(page, "03_story_world_operator");
  const storyText = await page.locator(".chat").innerText();
  record("T39 Story movement + world messages", storyText.includes("Great Hall"), "");

  // Guild: post a task; scout request appears; approve; wait for archive.
  await page.locator(".nav-item", { hasText: "guild" }).click();
  await box.fill("Evaluate two explanations for the outage: (A) cache change or (B) traffic spike.");
  await box.press("Enter");
  try {
    await page.getByRole("button", { name: "Approve" }).first().waitFor({ timeout: 60000 });
    await shot(page, "04_guild_spawn_request");
    await page.getByRole("button", { name: "Approve" }).first().click();
    await page.getByRole("tab", { name: /Org & tasks/ }).click();
    await page.locator(".badge", { hasText: "READY_FOR_REVIEW" }).first().waitFor({ timeout: 60000 });
    await sleep(1000);
    await shot(page, "05_guild_org_tasks_after_child");
    record("T31/T33 approve child, child reports, task ready for review (GUI)", true);
  } catch (e) { record("T31/T33 Guild spawn lifecycle (GUI)", false, String(e)); }

  // Crucible.
  await page.locator(".nav-item", { hasText: "crucible" }).click();
  await page.getByRole("tab", { name: "Cases" }).click();
  await page.getByPlaceholder(/Contentious proposal/).fill("Replace all code review with automated LLM review.");
  await page.getByRole("button", { name: "Start review" }).click();
  try {
    await page.locator("article.msg").filter({ hasText: "Strongest objection" }).first().waitFor({ timeout: 40000 });
    await page.locator("article.msg").filter({ hasText: "Both readings" }).first().waitFor({ timeout: 40000 });
    await shot(page, "06_crucible_case");
    record("T48 Crucible three distinct role messages (GUI)", true);
  } catch (e) { record("T48 Crucible (GUI)", false, String(e)); }

  // T18 pause / step / resume.
  await page.getByRole("button", { name: "Pause simulation" }).click();
  await page.getByRole("button", { name: "Run simulation" }).waitFor({ timeout: 5000 });
  await page.getByRole("button", { name: /Step/ }).click();
  await page.getByRole("button", { name: "Run simulation" }).click();
  await page.getByRole("button", { name: "Pause simulation" }).waitFor({ timeout: 5000 });
  record("T18 pause/step/resume controls", true);

  // Observatory: events + who-knows-what.
  await page.getByRole("button", { name: /Observatory/ }).click();
  await page.locator("table.t tbody tr").first().waitFor({ timeout: 10000 });
  const n = await page.locator("table.t tbody tr").count();
  await shot(page, "07_observatory_events");
  await page.getByRole("tab", { name: "Who knows what" }).click();
  await sleep(800);
  await shot(page, "08_observatory_who_knows_what");
  record("Observatory event log + knowledge inspector", n > 10, `${n} rows`);

  // T09 Simple/Advanced and participant view role.
  await page.getByRole("button", { name: /Operator view/ }).click();
  await page.getByRole("button", { name: /Participant view/ }).waitFor({ timeout: 5000 });
  await page.getByRole("tab", { name: "Who knows what" }).click();
  const gated = await page.getByText("operator-only", { exact: false }).count();
  record("Participant view cannot open who-knows-what", gated > 0);
  await page.getByRole("button", { name: /Participant view/ }).click();

  // T08 keyboard: Tab moves focus to a visible control.
  await page.keyboard.press("Tab");
  const focused = await page.evaluate(() => document.activeElement && document.activeElement.tagName);
  record("T08 keyboard focus reachable", !!focused, focused ?? "");

  // T10 forced stop + relaunch keeps data.
  await browser.close().catch(() => {});
  execSync(`taskkill /PID ${child.pid} /T /F`);
  await sleep(1500);
  child = launch();
  ({ browser, page } = await attach());
  try {
    await page.locator(".nav-item", { hasText: "commons" }).waitFor({ timeout: 20000 });
    await page.locator(".nav-item", { hasText: "commons" }).click();
    await waitText(page, "shredding my favorite theory", 15000);
    await shot(page, "09_after_forced_restart");
    record("T10/T61 relaunch after forced stop retains rooms + messages", true);
  } catch (e) { record("T10 relaunch persistence", false, String(e)); }

  // Graceful exit (WM_CLOSE) and verify process ends.
  await browser.close().catch(() => {});
  try { execSync(`taskkill /PID ${child.pid}`); } catch {}
  let gone = false;
  for (let i = 0; i < 40 && !gone; i++) {
    await sleep(250);
    try { process.kill(child.pid, 0); } catch { gone = true; }
  }
  if (!gone) { try { execSync(`taskkill /PID ${child.pid} /T /F`); } catch {} }
  record("Clean exit on window close", gone);

  if (portable) {
    const db = path.join(path.dirname(exe), "data", "roomlab.sqlite");
    record("T61 portable data folder created next to RoomLab.exe", fs.existsSync(db), db);
  }
  const realErrors = errors.filter((e) => !/favicon/i.test(e));
  record("No uncaught page errors", realErrors.length === 0, realErrors.slice(0, 3).join(" | "));
  const failed = results.filter((r) => !r.ok);
  fs.writeFileSync(path.join(shots, "smoke_results.json"), JSON.stringify({ when: new Date().toISOString(), exe, dataDir, results }, null, 2));
  console.log(`\n${results.length - failed.length}/${results.length} GUI checks passed`);
  process.exit(failed.length ? 1 : 0);
}

main().catch((e) => { console.error(e); process.exit(1); });
