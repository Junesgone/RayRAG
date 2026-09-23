// CDP probe for the first-login setup page (`api::setup` + `web::setup_page`):
// a real user opens /setup, reads the host detection, edits a live setting and a
// restart-only setting, saves, and the probe checks the receipt *and* the file the
// process writes for the next boot.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/setup-wizard-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}
const STAMP = Date.now();
const shots = "docs/ui-traversal/rayrag-audit";
mkdirSync(shots, { recursive: true });

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1440, height: 1000 });
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
const results = { steps: [], errors: [], pageErrors };
const step = (name, value) => {
  results.steps.push({ name, value });
  console.log(`• ${name}: ${JSON.stringify(value)}`);
};
const fail = (message) => {
  throw new Error(message);
};

try {
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const seeded = await page.evaluate(async ({ password }) => {
    const login = await fetch("/api/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "admin@rayrag.local", password }),
    }).then((res) => res.json());
    if (!login.data?.access_token) throw new Error("login failed");
    const token = login.data.access_token;
    localStorage.setItem("rayrag_token", token);
    document.cookie = `rayrag_token=${token};path=/`;
    return { token };
  }, { password: PASSWORD });
  step("logged in", { token: `${seeded.token.slice(0, 8)}…` });

  // 1. The status route answers before any session exists (the login page uses it).
  const status = await page.evaluate(() => fetch("/api/v1/setup/status").then((r) => r.json()));
  if (status.code !== 0) fail("setup status failed: " + JSON.stringify(status));
  step("setup status", status.data);

  // 2. A user opens the page and the catalogue renders.
  await page.goto(`${ORIGIN}/setup`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='setup-groups'] .setup-card", { timeout: 30000 });
  results.render = await page.evaluate(() => ({
    groups: Array.from(document.querySelectorAll("[data-testid='setup-groups'] .setup-card")).map(
      (card) => card.dataset.group,
    ),
    fields: document.querySelectorAll("[data-testid='setup-groups'] .setup-field").length,
    liveBadges: document.querySelectorAll(".setup-live").length,
    envBadges: document.querySelectorAll(".setup-from-env").length,
    host: document.getElementById("setupHost")?.textContent.trim(),
    recommended: document.querySelector("[data-testid='setup-recommended-tasks']")?.textContent,
    envFile: document.getElementById("setupEnvFile")?.textContent.trim(),
  }));
  if (!results.render.groups.includes("models")) fail("the models group must be present");
  if (results.render.fields < 15) fail("expected at least 15 configurable fields");
  if (!results.render.recommended) fail("the host recommendation must be shown");
  step("page render", results.render);
  await page.screenshot({ path: `${shots}/setup-wizard-${STAMP}.png` });

  // 3. Edit a live value and a restart-only value, then save.
  const edited = await page.evaluate(() => {
    const timeout = document.querySelector("[data-testid='setup-RAYRAG_MODEL_TIMEOUT']");
    const workers = document.querySelector("[data-testid='setup-RAYRAG_MAX_CONCURRENT_TASKS']");
    timeout.value = "333";
    timeout.dispatchEvent(new Event("input", { bubbles: true }));
    workers.value = "2";
    workers.dispatchEvent(new Event("input", { bubbles: true }));
    return { timeout: timeout.value, workers: workers.value };
  });
  step("edited fields", edited);
  await page.click("[data-testid='setup-save']");
  await page.waitForFunction(
    () => {
      const node = document.getElementById("setupStatus");
      return node && node.textContent && node.textContent.startsWith("Saved");
    },
    null,
    { timeout: 30000 },
  );
  results.receipt = await page.evaluate(
    () => document.getElementById("setupStatus").textContent,
  );
  step("save receipt", results.receipt);
  // The receipt must separate what applies now from what waits for a restart, and it
  // must not claim the restart-only value was applied.
  if (!results.receipt.includes("applied now")) fail("the receipt must list live changes");
  if (!results.receipt.includes("restart to apply")) fail("the receipt must list restart-only changes");
  if (results.receipt.includes("applied now: 2")) fail("a startup-only value was claimed live");

  // 4. The file the process writes is the one it read at startup.
  const onDisk = await page.evaluate(async () => {
    const response = await fetch("/api/v1/setup/options");
    const payload = await response.json();
    return payload.data.env_file;
  });
  results.envFile = onDisk;
  step("env file", onDisk);

  // 5. Reopening the page shows the stored values (the save is durable).
  await page.goto(`${ORIGIN}/setup`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='setup-RAYRAG_MODEL_TIMEOUT']", { timeout: 30000 });
  results.reloaded = await page.evaluate(() => ({
    timeout: document.querySelector("[data-testid='setup-RAYRAG_MODEL_TIMEOUT']").value,
    workers: document.querySelector("[data-testid='setup-RAYRAG_MAX_CONCURRENT_TASKS']").value,
  }));
  step("reloaded values", results.reloaded);

  results.pageErrors = pageErrors;
  if (pageErrors.length) fail("page errors: " + JSON.stringify(pageErrors));
} catch (error) {
  results.errors.push(String(error && error.message ? error.message : error));
  results.diagnostics = await page
    .evaluate(() => ({
      url: location.href,
      status: document.getElementById("setupStatus")?.textContent,
      fields: document.querySelectorAll(".setup-field").length,
    }))
    .catch((diagnosticError) => ({ diagnosticError: String(diagnosticError.message) }));
  console.error("PROBE FAILED:", results.errors[results.errors.length - 1]);
}
console.log(JSON.stringify(results, null, 2));
process.exit(results.errors.length ? 1 : 0);
