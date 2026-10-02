// CDP probe for `/admin/sandbox-settings`
// (`pages/admin/sandbox-settings.tsx`): the provider-selection radio grid comes
// from `/api/v1/admin/sandbox/providers`, the configuration card renders the
// selected provider's schema (SSH switches between Password and Private Key
// authentication), "Test connection" posts to `/api/v1/admin/sandbox/test` and
// renders the result modal, and "Save configuration" persists the active
// provider. Nothing is mutated: the save re-submits the already active
// `self_managed` provider, and the SSH probe only points at a closed port.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/admin-sandbox-providers-cdp.mjs
import { chromium } from "playwright-core";
import { readFileSync } from "node:fs";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}

const PROVIDERS = ["local", "self_managed", "ssh", "aliyun_codeinterpreter", "e2b", "tenki", "ucloud_agent_sandbox"];
const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
await page.setViewportSize({ width: 1500, height: 1000 });
const results = { errors: [], pageErrors };
mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });
const check = (ok, label, detail) => {
  results[label] = ok ? "ok" : "FAIL";
  if (!ok) results.errors.push(label + (detail ? ` (${detail})` : ""));
};
const text = async (selector) => (await page.locator(selector).innerText()).trim();

try {
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const login = await page.evaluate(async (password) => {
    const res = await fetch("/api/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "admin@rayrag.local", password }),
    });
    const data = await res.json();
    if (data.code !== 0) return { ok: false, body: data };
    localStorage.setItem("rayrag_token", data.data.access_token);
    document.cookie = "rayrag_token=" + data.data.access_token + ";path=/";
    return { ok: true };
  }, PASSWORD);
  if (!login.ok) throw new Error("login failed: " + JSON.stringify(login.body));

  await page.goto(`${ORIGIN}/admin/sandbox-settings`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='admin-sandbox-provider-self_managed']", {
    timeout: 20000,
  });
  await page.waitForFunction(
    (ids) => ids.every((id) => document.querySelector(`[data-testid='admin-sandbox-provider-${id}']`)),
    PROVIDERS,
    { timeout: 20000 },
  );
  for (const id of PROVIDERS) {
    const card = page.locator(`[data-testid='admin-sandbox-provider-${id}']`);
    check((await card.count()) === 1, `provider_card_${id}`);
  }
  const cards = await page.locator("[data-testid='admin-sandbox-providers'] .sb-provider").allInnerTexts();
  check(cards.length === PROVIDERS.length, "all_provider_cards", String(cards.length));
  check(cards.some((text) => text.includes("Self-Managed")), "self_managed_card_copy", cards.map((text) => text.replace(/\n/g, "/")).join(" | ").slice(0, 120));
  check(cards.some((text) => text.includes("self-hosted")), "provider_tags_render");
  check(cards.some((text) => text.toUpperCase().includes("E2B")), "e2b_card_copy");

  // The active provider (seeded `self_managed`) opens with its schema fields.
  await page.waitForSelector("#sbConfigCard:not([hidden])", { timeout: 20000 });
  check(
    (await text("#sbConfigTitle")) === "Self-Managed configuration",
    "config_title_names_the_provider",
    await text("#sbConfigTitle"),
  );
  check(
    (await page.locator("[data-field='endpoint']").count()) === 1,
    "endpoint_field_rendered",
  );
  check(
    (await page.locator("details[data-testid='admin-sandbox-deployment']").count()) === 1,
    "deployment_defaults_disclosure",
  );
  const readonly = await page.locator("[data-field='executor_manager_image']").isDisabled();
  check(readonly, "deployment_fields_are_readonly");

  // Selecting SSH swaps the schema, groups the fields and shows the auth tabs.
  await page.locator("[data-testid='admin-sandbox-provider-ssh']").click();
  await page.waitForFunction(
    () => (document.getElementById("sbConfigTitle") || {}).textContent === "SSH configuration",
    null,
    { timeout: 20000 },
  );
  await page.waitForSelector("[data-field='host']", { timeout: 20000 });
  for (const field of ["host", "username", "port", "work_dir", "python_bin", "node_bin"]) {
    check(
      (await page.locator(`[data-field='${field}']`).count()) === 1,
      `ssh_field_${field}`,
    );
  }
  check(
    (await page.locator("details[data-testid='admin-sandbox-deployment']").count()) === 0,
    "ssh_has_no_deployment_defaults",
  );
  check(
    (await page.locator("[data-field='password']").count()) === 1 &&
      (await page.locator("[data-field='private_key']").count()) === 0,
    "password_tab_is_the_default",
  );
  const authText = await text(".sb-auth");
  check(
    authText.includes("Password") && authText.includes("Private Key"),
    "auth_tabs_present",
  );
  check(
    (await text("#sbFields .sb-group:first-child")).includes("Runtime Settings"),
    "runtime_group",
  );
  await page.locator("[data-testid='admin-sandbox-auth-private_key']").click();
  await page.waitForSelector("[data-field='private_key']", { timeout: 10000 });
  check(
    (await page.locator("[data-field='password']").count()) === 0,
    "private_key_tab_hides_the_password",
  );
  check(
    (await page.locator("[data-field='passphrase']").count()) === 1,
    "passphrase_belongs_to_the_key_tab",
  );

  // SSH probe against a closed port: the modal reports the real failure.
  await page.fill("[data-field='host']", "127.0.0.1");
  await page.fill("[data-field='port']", "1");
  await page.fill("[data-field='username']", "ragflow");
  await page.fill("[data-field='private_key']", readFileSync(process.env.RAYRAG_SSH_PROBE_KEY || "/tmp/rr-sshd/client", "utf8"));
  await page.locator("[data-testid='admin-sandbox-test']").click();
  await page.waitForSelector("[data-testid='admin-sandbox-test-modal'].show", { timeout: 10000 });
  check(true, "test_modal_opens");
  await page.waitForSelector("[data-testid='admin-sandbox-test-message']", { timeout: 30000 });
  const sshMessage = await text("[data-testid='admin-sandbox-test-message']");
  check(sshMessage.includes("Test FAILED"), "ssh_probe_reports_failure", sshMessage.slice(0, 80));
  check(
    (await text("#sbTestSummary")) === "Failed to connect to sandbox provider",
    "failure_summary_copy",
  );
  await page.locator("#sbTestModal .ds-dialog-footer button").click();
  await page.waitForFunction(
    () => !document.getElementById("sbTestModal").classList.contains("show"),
    null,
    { timeout: 10000 },
  );
  check(true, "test_modal_closes");

  // The local provider really runs the upstream probe and passes.
  await page.locator("[data-testid='admin-sandbox-provider-local']").click();
  await page.waitForFunction(
    () => (document.getElementById("sbConfigTitle") || {}).textContent === "Local configuration",
    null,
    { timeout: 20000 },
  );
  await page.locator("[data-testid='admin-sandbox-test']").click();
  await page.waitForSelector("[data-testid='admin-sandbox-test-message']", { timeout: 60000 });
  const localMessage = await text("[data-testid='admin-sandbox-test-message']");
  check(localMessage.includes("Test PASSED"), "local_probe_passes", localMessage.slice(0, 90));
  check(
    (await text("[data-testid='admin-sandbox-test-stdout']")).includes("TEST_PASSED"),
    "local_probe_stdout_rendered",
  );
  check(
    (await text("#sbTestSummary")) === "Successfully connected to sandbox provider",
    "success_summary_copy",
  );
  check(
    localMessage.includes("Exit code: 0") && /Execution time: \d+\.\d\ds/.test(localMessage),
    "exit_code_and_execution_time_rendered",
  );
  await page.locator("#sbTestModal .ds-dialog-footer button").click();
  await page.waitForFunction(
    () => !document.getElementById("sbTestModal").classList.contains("show"),
    null,
    { timeout: 10000 },
  );

  // Saving re-submits the already active provider, so nothing changes on disk.
  await page.locator("[data-testid='admin-sandbox-provider-self_managed']").click();
  await page.waitForFunction(
    () => (document.getElementById("sbConfigTitle") || {}).textContent === "Self-Managed configuration",
    null,
    { timeout: 20000 },
  );
  await page.locator("[data-testid='admin-sandbox-save']").click();
  await page.waitForFunction(
    () => /updated successfully/.test((document.getElementById("sbStatus") || {}).textContent || ""),
    null,
    { timeout: 20000 },
  );
  check(true, "save_reports_success");
  const active = await page.evaluate(async () => {
    const res = await fetch("/api/v1/admin/sandbox/config");
    const data = await res.json();
    return data.data ? data.data.provider_type : null;
  });
  check(active === "self_managed", "active_provider_unchanged", String(active));
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/admin-sandbox-providers.png" });
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
} finally {
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
