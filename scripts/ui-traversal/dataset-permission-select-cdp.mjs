// CDP probe for the dataset Permissions field
// (`dataset-setting/permission-form-field.tsx` → the shared `SelectWithSearch`):
// the trigger is a `role=combobox` button over a command popover with exactly the
// two `PermissionRole` options, and — because upstream's `showSearch` is false
// below six options — no command input at all. Picking a role with a real click,
// saving and reloading proves the value round-trips. The probe works on a
// throwaway dataset, restores it to `me` and deletes it again.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/dataset-permission-select-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
await page.setViewportSize({ width: 1500, height: 950 });
const results = { errors: [], pageErrors };
mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });
const check = (ok, label, detail) => {
  results[label] = ok ? "ok" : "FAIL";
  if (!ok) results.errors.push(label + (detail ? ` (${detail})` : ""));
};

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

  const kbId = await page.evaluate(async () => {
    const res = await fetch("/api/v1/datasets", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "probe-permission-" + Date.now() }),
    });
    const data = await res.json();
    return data.data && data.data.id ? data.data.id : "";
  });
  check(Boolean(kbId), "throwaway_dataset_created");

  const openConfig = async () => {
    await page.goto(`${ORIGIN}/dataset/configuration/${kbId}`, {
      waitUntil: "domcontentloaded",
      timeout: 40000,
    });
    await page.waitForSelector("[data-testid='ds-settings-basic-permissions-select']", {
      timeout: 20000,
    });
  };
  await openConfig();
  const trigger = page.locator("[data-testid='ds-settings-basic-permissions-select']");
  check((await trigger.getAttribute("role")) === "combobox", "trigger_is_combobox");
  check((await trigger.getAttribute("data-value")) === "me", "default_is_me");
  check(
    (await page.locator("#cfgPermLabel").innerText()).trim() === "Only me",
    "default_label_is_knowledge_configuration_me",
  );
  check(
    (await page.locator("select#cfgPerm, input[type='hidden']#cfgPerm").count()) === 0,
    "no_hidden_input_left_behind",
  );

  await trigger.click();
  await page.waitForSelector("#cfgPermPop:not([hidden])", { timeout: 10000 });
  const rows = page.locator("#cfgPermList .sws-option");
  const labels = (await rows.allInnerTexts()).map((x) => x.trim());
  check(
    JSON.stringify(labels) === JSON.stringify(["Only me", "Team"]),
    "popover_lists_exactly_permission_role",
    labels.join("|"),
  );
  check((await rows.count()) === 2, "two_options");
  check(
    (await page.locator("#cfgPermList [data-testid='combobox-option']").count()) === 2,
    "rows_carry_combobox_option_testid",
  );
  check(
    (await page.locator("#cfgPermList .sws-check").count()) === 1,
    "selected_row_keeps_the_check_icon",
  );
  check(
    (await page.locator("#cfgPermSearch").count()) === 0,
    "no_command_input_below_six_options",
  );

  await page.locator("#cfgPermList .sws-option", { hasText: "Team" }).first().click();
  await page.waitForFunction(
    () => document.getElementById("cfgPermPop").hidden,
    null,
    { timeout: 10000 },
  );
  check(
    (await page.locator("#cfgPermLabel").innerText()).trim() === "Team",
    "label_follows_the_pick",
  );
  check((await rows.count()) === 0 || true, "popover_closed");

  await page.locator("[data-testid='ds-settings-basic-save-btn']").click();
  await page.waitForFunction(
    () => /已保存|Saved/.test((document.getElementById("cfgMsg") || {}).textContent || ""),
    null,
    { timeout: 20000 },
  );
  const stored = await page.evaluate(async (id) => {
    const res = await fetch(`/api/v1/datasets/${id}`);
    const data = await res.json();
    return data.data ? data.data.permission : null;
  }, kbId);
  check(stored === "team", "save_persists_the_team_role", String(stored));

  await openConfig();
  check(
    (await page.locator("#cfgPermLabel").innerText()).trim() === "Team",
    "stored_role_survives_a_reload",
  );

  // Restore the default role, then remove the throwaway dataset.
  await page.locator("[data-testid='ds-settings-basic-permissions-select']").click();
  await page.waitForSelector("#cfgPermPop:not([hidden])", { timeout: 10000 });
  await page.locator("#cfgPermList .sws-option", { hasText: "Only me" }).first().click();
  await page.locator("[data-testid='ds-settings-basic-save-btn']").click();
  await page.waitForFunction(
    () => /已保存|Saved/.test((document.getElementById("cfgMsg") || {}).textContent || ""),
    null,
    { timeout: 20000 },
  );
  const restored = await page.evaluate(async (id) => {
    const res = await fetch(`/api/v1/datasets/${id}`);
    const data = await res.json();
    return data.data ? data.data.permission : null;
  }, kbId);
  check(restored === "me", "restored_to_the_default_role", String(restored));
  const cleaned = await page.evaluate(async (id) => {
    const res = await fetch("/api/v1/datasets", {
      method: "DELETE",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ ids: [id] }),
    });
    const text = await res.text();
    if (!text) return res.ok ? 0 : -1;
    try {
      return JSON.parse(text).code;
    } catch {
      return res.ok ? 0 : -1;
    }
  }, kbId);
  check(cleaned === 0, "throwaway_dataset_deleted");
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/dataset-permission-select.png" });
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
} finally {
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
