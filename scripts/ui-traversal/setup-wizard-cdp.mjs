// CDP probe for the first-login setup page (`api::setup` + `web::setup_page`):
// a real user opens /setup, reads the host detection, edits a live setting and a
// restart-only setting, saves, and the probe checks the receipt *and* the file the
// process writes for the next boot.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/setup-wizard-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

// The wizard writes the deployment's `.env`, which is the project root's file (mounted
// into the container). The probe edits two values and saves, so it snapshots that file
// first and puts it back afterwards: a probe must not leave the operator's configuration
// changed. `RAYRAG_PROJECT_ENV` overrides the path for a deployment that moved it.
const PROJECT_ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const ENV_PATH = process.env.RAYRAG_PROJECT_ENV || join(PROJECT_ROOT, ".env");
let envBackup = null;
try {
  envBackup = readFileSync(ENV_PATH, "utf8");
} catch {
  envBackup = null;
}

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
  for (const group of ["storage", "resources", "search", "access"]) {
    if (!results.render.groups.includes(group)) fail(`the ${group} group must be present`);
  }
  if (results.render.fields < 25) fail("expected at least 25 configurable fields");
  if (!results.render.recommended) fail("the host recommendation must be shown");
  step("page render", results.render);
  await page.screenshot({ path: `${shots}/setup-wizard-${STAMP}.png` });

  // 2a. Values the deployment never set are prefilled with the built-in default and
  //     labelled as such, so a first-login user confirms instead of researching.
  results.prefill = await page.evaluate(() => {
    const read = (key) => {
      const control = document.querySelector(`[data-testid='setup-${key}']`);
      return {
        value: control ? control.value : null,
        isDefault: !!document.querySelector(`[data-testid='setup-default-${key}']`),
      };
    };
    return {
      cmdTimeout: read("RAYRAG_CMD_TIMEOUT"),
      bodyLimit: read("RAYRAG_HTTP_BODY_LIMIT_BYTES"),
      uploadLimit: read("RAYRAG_MAX_UPLOAD_BYTES"),
      dimension: read("RAYRAG_EMBEDDING_DIMENSION"),
      backend: read("RAYRAG_VECTOR_BACKEND"),
      register: read("REGISTER_ENABLED"),
      embedBase: read("EMBED_API_BASE"),
    };
  });
  step("prefilled defaults", results.prefill);
  // A deployment that already sets a variable shows its own value (source `env`/`file`);
  // one that does not shows the code default with the `default` badge. Both are "prefilled
  // and only needs confirming", which is the contract under test.
  if (results.prefill.cmdTimeout.value !== "7200") {
    fail("the command timeout must show 7200: " + JSON.stringify(results.prefill.cmdTimeout));
  }
  if (results.prefill.bodyLimit.value !== "33554432") {
    fail("the body limit must show 32 MiB: " + JSON.stringify(results.prefill.bodyLimit));
  }
  if (results.prefill.uploadLimit.value !== "67108864") {
    fail("the upload limit must show 64 MiB: " + JSON.stringify(results.prefill.uploadLimit));
  }
  if (results.prefill.dimension.value !== "384" || results.prefill.backend.value !== "zvec") {
    fail("the vector settings must be prefilled: " + JSON.stringify(results.prefill));
  }
  if (!results.prefill.register.value) {
    fail("the sign-up switch must be prefilled: " + JSON.stringify(results.prefill.register));
  }
  if (results.prefill.embedBase.isDefault || results.prefill.embedBase.isDefault === undefined) {
    fail("an endpoint without a default must not be labelled one");
  }
  const labelled = Object.entries(results.prefill).filter(([, entry]) => entry.isDefault);
  if (!labelled.length) {
    fail("at least the unset fields must carry the default badge: " + JSON.stringify(results.prefill));
  }

  // 2b. The container memory limit is shown as a copyable line, not an editable
  //     setting: its value lives in docker-compose.yml, and an input here would
  //     store a line nothing reads.
  results.readonly = await page.evaluate(() => {
    const line = document.querySelector("[data-testid='setup-readonly-RAYRAG_MEMORY_LIMIT']");
    const button = document.querySelector("[data-testid='setup-copy-RAYRAG_MEMORY_LIMIT']");
    const field = document.querySelector(".setup-field[data-key='RAYRAG_MEMORY_LIMIT']");
    return {
      lineText: line ? line.textContent : null,
      hasInput: field ? !!field.querySelector("input,select") : null,
      isCode: line ? line.tagName : null,
      copyLabel: button ? button.textContent : null,
      help: field ? field.querySelector(".setup-help")?.textContent : null,
    };
  });
  step("memory limit row", results.readonly);
  if (!results.readonly.lineText?.startsWith("RAYRAG_MEMORY_LIMIT=")) {
    fail("the memory limit line must be copy-ready: " + results.readonly.lineText);
  }
  if (results.readonly.hasInput) fail("the memory limit must not be editable here");
  if (results.readonly.isCode !== "CODE") fail("the line must render as a code block");
  if (!results.readonly.copyLabel) fail("the row must offer a copy button");
  if (!results.readonly.help?.includes("docker-compose.yml")) {
    fail("the help must say where the value belongs");
  }
  await page.click("[data-testid='setup-copy-RAYRAG_MEMORY_LIMIT']");
  await page.waitForTimeout(400);
  results.copyFeedback = await page.evaluate(
    () => document.querySelector("[data-testid='setup-copy-RAYRAG_MEMORY_LIMIT']").textContent,
  );
  step("copy feedback", results.copyFeedback);

  // 2c. Writing it is refused by the API — the page cannot promise what it cannot do.
  results.readonlyWrite = await page.evaluate(async () => {
    const response = await fetch("/api/v1/setup/complete", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        updates: [{ key: "RAYRAG_MEMORY_LIMIT", value: "8g" }],
        mark_completed: false,
      }),
    });
    return { status: response.status, body: await response.json() };
  });
  step("readonly write refused", results.readonlyWrite);
  if (results.readonlyWrite.body.code === 0) {
    fail("the container memory limit must not be writable from the page");
  }

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

  // 3b. The receipt continues into the model-provider page, because endpoints in a
  //     file are only half of a deployment: the models are added (and verified) there.
  results.nextStep = await page.evaluate(() => {
    const host = document.getElementById("setupNext");
    const link = document.querySelector("[data-testid='setup-next']");
    return {
      hidden: host ? host.hidden : null,
      href: link ? link.getAttribute("href") : null,
      label: link ? link.textContent : null,
      hint: document.querySelector("[data-testid='setup-next-hint']")?.textContent || null,
    };
  });
  step("next step", results.nextStep);
  if (results.nextStep.hidden !== false) fail("the hand-off must be visible after saving");
  if (results.nextStep.href !== "/user-setting/model") {
    fail("the hand-off must point at the model-provider page");
  }

  // 3c. The wizard follows the environment file: an edit made outside the page (here a
  //     direct append, as an operator would do with vi) reloads into the form.
  writeFileSync(
    ENV_PATH,
    readFileSync(ENV_PATH, "utf8").replace(/^RAYRAG_MAX_UPLOAD_BYTES=.*$/m, "") +
      "RAYRAG_MAX_UPLOAD_BYTES=12345678\n",
  );
  await page.waitForFunction(
    () => {
      const control = document.querySelector("[data-testid='setup-RAYRAG_MAX_UPLOAD_BYTES']");
      return control && control.value === "12345678";
    },
    null,
    { timeout: 30000 },
  );
  // `waitForSelector` waits for *visibility*, so a hidden banner needs the predicate form.
  await page.waitForFunction(() => document.getElementById("setupSync").hidden, null, {
    timeout: 30000,
  });
  results.externalEdit = await page.evaluate(() => ({
    value: document.querySelector("[data-testid='setup-RAYRAG_MAX_UPLOAD_BYTES']").value,
    isDefault: !!document.querySelector("[data-testid='setup-default-RAYRAG_MAX_UPLOAD_BYTES']"),
    banner: document.getElementById("setupSync").hidden,
  }));
  step("external edit picked up", results.externalEdit);
  if (results.externalEdit.isDefault) {
    fail("a value read from the file must not be labelled a default");
  }

  // 3d. With unsaved edits the page warns instead of silently discarding them.
  await page.fill("[data-testid='setup-RAYRAG_CORS_ORIGIN']", "http://typed.example");
  writeFileSync(
    ENV_PATH,
    readFileSync(ENV_PATH, "utf8").replace(/^RAYRAG_OCR_PROVIDER=.*$/m, "") +
      "RAYRAG_OCR_PROVIDER=none\n",
  );
  // Wait for the *warning*: an informational banner from the earlier reload must not
  // satisfy this check.
  await page.waitForSelector("[data-testid='setup-sync'].warn", { timeout: 30000 });
  results.dirtyWarning = await page.evaluate(() => ({
    text: document.getElementById("setupSyncText").textContent,
    warn: document.getElementById("setupSync").classList.contains("warn"),
    typed: document.querySelector("[data-testid='setup-RAYRAG_CORS_ORIGIN']").value,
    reloadLabel: document.getElementById("setupSyncReload").textContent,
  }));
  step("unsaved-edit warning", results.dirtyWarning);
  if (!results.dirtyWarning.warn) fail("the warning must be marked as a warning");
  if (results.dirtyWarning.typed !== "http://typed.example") {
    fail("the page must not discard unsaved edits");
  }
  await page.click("[data-testid='setup-sync-reload']");
  await page.waitForFunction(
    () => {
      const control = document.querySelector("[data-testid='setup-RAYRAG_CORS_ORIGIN']");
      return control && control.value === "";
    },
    null,
    { timeout: 30000 },
  );
  step("reloaded after confirming", true);

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
// Put the operator's configuration back exactly as it was: the wizard wrote the
// project's `.env`, and a probe must not leave a deployment reconfigured.
if (envBackup !== null) {
  try {
    writeFileSync(ENV_PATH, envBackup);
    console.log(`restored ${ENV_PATH}`);
  } catch (error) {
    console.error(`could not restore ${ENV_PATH}: ${error.message}`);
  }
}
console.log(JSON.stringify(results, null, 2));
process.exit(results.errors.length ? 1 : 0);
