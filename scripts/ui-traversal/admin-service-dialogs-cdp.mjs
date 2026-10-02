// CDP probe for the admin service dialogs
// (`pages/admin/service-status.tsx` + `service-detail.tsx` +
// `task-executor-detail.tsx`): the row actions are the hover-revealed
// ClipboardList (service details) and Settings2 (extra information) icons, the
// details dialog titles itself per service type and renders the payload by shape
// — the task executor gets one card per executor with `Lag:`/`Pending:`, an SVG
// bar chart with a legend, and a click tooltip. The probe only opens and closes
// dialogs.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/admin-service-dialogs-cdp.mjs
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

  await page.goto(`${ORIGIN}/admin/services`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='admin-service-row']", { timeout: 20000 });

  // The two action icons are hidden until the row is hovered (upstream
  // `opacity-0 group-hover/row:opacity-100`).
  const firstRow = page.locator("[data-testid='admin-service-row']").first();
  const actions = firstRow.locator(".admin-row-actions");
  check(
    (await actions.evaluate((node) => getComputedStyle(node).opacity)) === "0",
    "row_actions_hidden_by_default",
  );
  await firstRow.hover();
  await page.waitForTimeout(200);
  check(
    (await actions.evaluate((node) => getComputedStyle(node).opacity)) === "1",
    "row_actions_revealed_on_hover",
  );
  check(
    (await firstRow.locator("[data-testid='admin-service-action-clipboard']").count()) === 1 &&
      (await firstRow.locator("[data-testid='admin-service-action-extra']").count()) === 1,
    "both_upstream_icons_present",
  );

  // Extra information dialog renders the row's `extra` through the JSON tree.
  await firstRow.locator("[data-testid='admin-service-action-extra']").click();
  await page.waitForSelector("[data-testid='admin-service-extra-dialog'].show", { timeout: 10000 });
  check((await text("#adminExtraDialog h2")) === "Extra information", "extra_dialog_title");
  check((await page.locator("#adminServiceExtra .json-tree-node").count()) >= 1, "extra_json_tree");
  await page.locator("[data-testid='admin-extra-close']").click();
  await page.waitForFunction(
    () => !document.getElementById("adminExtraDialog").classList.contains("show"),
    null,
    { timeout: 10000 },
  );
  check(true, "extra_dialog_closes");

  // Service details dialog: title per type and a shaped body.
  const types = await page.locator("[data-testid='admin-service-row'] td:nth-child(3)").allInnerTexts();
  const executorRow = page
    .locator("[data-testid='admin-service-row']")
    .filter({ has: page.locator("td:nth-child(3)", { hasText: "task_executor" }) })
    .first();
  const targetRow = (await executorRow.count()) ? executorRow : firstRow;
  await targetRow.hover();
  await targetRow.locator("[data-testid='admin-service-action-clipboard']").click();
  await page.waitForSelector("[data-testid='admin-service-detail-dialog'].show", { timeout: 20000 });
  const title = await text("#adminServiceDialogTitle");
  const isExecutor = (await executorRow.count()) > 0;
  check(
    isExecutor ? title === "Task executor detail" : /^Service .+ detail$/.test(title),
    "details_dialog_title",
    title,
  );
  check(types.length >= 5, "service_types_rendered", String(types.length));
  const body = page.locator("#adminServiceDetail");
  const shaped =
    (await body.locator("[data-testid='admin-task-executor-detail']").count()) > 0 ||
    (await body.locator("[data-testid='admin-service-detail-object']").count()) > 0 ||
    (await body.locator("[data-testid='admin-service-detail-table']").count()) > 0 ||
    (await body.locator("[data-testid='admin-service-detail-string']").count()) > 0;
  check(shaped, "details_body_uses_a_shaped_renderer");
  if (isExecutor && (await body.locator("[data-testid='admin-task-executor-card']").count())) {
    check(
      (await body.locator("[data-testid='admin-task-executor-chart']").count()) >= 1,
      "executor_bar_chart_rendered",
    );
    const chart = body.locator("[data-testid='admin-task-executor-chart']").first();
    check((await chart.locator(".admin-chart-done").count()) > 0, "done_bars_rendered");
    check((await chart.locator(".admin-chart-failed").count()) > 0, "failed_bars_rendered");
    check(
      (await chart.locator(".admin-chart-tick").count()) > 0,
      "rotated_time_ticks_rendered",
    );
    const rotation = await chart
      .locator(".admin-chart-tick")
      .first()
      .getAttribute("transform");
    check(String(rotation).includes("rotate(60)"), "ticks_rotate_60_degrees", String(rotation));
    check(
      (await body.locator(".admin-chart-legend-item").count()) === 2,
      "legend_has_two_entries",
    );
    check(
      (await text(".admin-executor-meta")).includes("Lag:") &&
        (await text(".admin-executor-meta")).includes("Pending:"),
      "executor_meta_line",
      await text(".admin-executor-meta"),
    );
    await chart.locator(".admin-chart-done[data-index]").first().dispatchEvent("click");
    await page.waitForSelector("[data-testid='admin-task-executor-tooltip']", { timeout: 10000 });
    check(true, "click_tooltip_opens");
    check(
      (await page.locator("[data-testid='admin-task-executor-tooltip'] .json-tree-node").count()) >= 1,
      "tooltip_uses_the_json_tree",
    );
  }
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/admin-service-dialog.png" });
  await page.locator("[data-testid='admin-service-detail-close']").click();
  await page.waitForFunction(
    () => !document.getElementById("adminServiceDialog").classList.contains("show"),
    null,
    { timeout: 10000 },
  );
  check(true, "details_dialog_closes");
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
} finally {
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
