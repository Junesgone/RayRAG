// CDP probe for the admin user table's last two cells
// (`pages/admin/users.tsx`): the `admin.userType` column is a normal/superuser
// `Select` that grants through `PUT /api/v1/admin/users/{u}/admin` and revokes
// through `DELETE` on the same endpoint, and collapses to a secondary
// `admin.superuser` badge on the signed-in administrator's own row; the actions
// cell holds the hover-revealed lucide trio (`ClipboardList` → account page,
// `UserLock` → change password, danger `Trash2` → delete), the last two only on
// rows that are not your own. The probe creates one throwaway account so a
// non-self row exists, exercises grant/revoke on it and deletes it again.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/admin-users-cells-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}
const THROWAWAY = "cells-probe@example.com";

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

const calls = [];
page.on("request", (request) => {
  const url = new URL(request.url());
  if (url.pathname.startsWith("/api/v1/admin/users")) {
    calls.push(`${request.method()} ${url.pathname}`);
  }
});

let created = false;
try {
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(ORIGIN + "/admin", { waitUntil: "domcontentloaded", timeout: 40000 });
  if (await page.locator("#adminEmail").count()) {
    await page.fill("#adminEmail", "admin@rayrag.local");
    await page.fill("#adminPassword", PASSWORD);
    await page.click("[data-testid='admin-login-submit']");
    await page.waitForURL("**/admin/services", { timeout: 20000 });
  }

  // A throwaway account so the table has a row that is not the signed-in admin.
  const setup = await page.evaluate(async (email) => {
    const token = localStorage.getItem("rayrag_token") || "";
    const res = await fetch("/api/v1/admin/users", {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: "Bearer " + token },
      body: JSON.stringify({ username: email, password: "probe-passphrase", role: "normal" }),
    });
    const data = await res.json().catch(() => ({}));
    return { status: res.status, code: data.code, message: data.message };
  }, THROWAWAY);
  created = setup.code === 0 || setup.status === 409;
  check(created, "throwaway_created", JSON.stringify(setup));

  await page.goto(ORIGIN + "/admin/users", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='admin-user-row']", { timeout: 20000 });
  await page.waitForTimeout(600);

  results.headers = await page.locator(".admin-table thead th").allInnerTexts();
  results.header_style = await page.locator(".admin-table thead th").first().evaluate((node) => {
    const style = getComputedStyle(node);
    return `${style.textTransform} ${style.fontWeight} ${style.height} ${style.paddingLeft} ${style.paddingRight}`;
  });
  check(
    results.headers.join("|") === "Email|Nickname|Role|Status|User type|Actions",
    "column_headers",
    results.headers.join("|"),
  );
  check(
    results.header_style === "none 400 56px 24px 16px",
    "header_geometry",
    results.header_style,
  );

  // The signed-in administrator's own row: badge, and no destructive actions.
  const selfRow = page.locator("[data-testid='admin-user-row']", {
    has: page.locator("[data-testid='admin-user-type-badge']"),
  });
  const selfCount = await selfRow.count();
  results.self_row = {
    count: selfCount,
    badge: selfCount ? (await selfRow.first().locator("[data-testid='admin-user-type-badge']").innerText()).trim() : null,
    detailIcons: selfCount ? await selfRow.first().locator("[data-testid='admin-user-action-detail']").count() : 0,
    passwordIcons: selfCount ? await selfRow.first().locator("[data-testid='admin-user-action-password']").count() : 0,
    deleteIcons: selfCount ? await selfRow.first().locator("[data-testid='admin-user-action-delete']").count() : 0,
    select: selfCount ? await selfRow.first().locator("[data-testid='admin-user-type-select']").count() : -1,
  };
  check(
    selfCount === 1 &&
      results.self_row.badge === "Superuser" &&
      results.self_row.select === 0 &&
      results.self_row.detailIcons === 1 &&
      results.self_row.passwordIcons === 0 &&
      results.self_row.deleteIcons === 0,
    "self_row_own_controls",
    JSON.stringify(results.self_row),
  );

  // The throwaway row: select with the three icons, revealed on hover only.
  const row = page.locator("[data-testid='admin-user-row']", {
    has: page.locator("[data-testid='admin-user-type-select']"),
  });
  const target = page.locator("[data-testid='admin-user-row']").filter({ hasText: THROWAWAY });
  check((await target.count()) === 1, "throwaway_row_present");
  const actions = target.locator(".admin-row-actions");
  const opacityBefore = await actions.evaluate((node) => getComputedStyle(node).opacity);
  await target.hover();
  await page.waitForTimeout(300);
  const opacityAfter = await actions.evaluate((node) => getComputedStyle(node).opacity);
  results.hover = { before: opacityBefore, after: opacityAfter };
  check(
    Number(opacityBefore) === 0 && Number(opacityAfter) === 1,
    "actions_revealed_on_hover",
    JSON.stringify(results.hover),
  );
  results.row_icons = {
    detail: await target.locator("[data-testid='admin-user-action-detail']").count(),
    password: await target.locator("[data-testid='admin-user-action-password']").count(),
    delete: await target.locator("[data-testid='admin-user-action-delete']").count(),
    order: await target.locator(".admin-row-actions > button").evaluateAll((nodes) =>
      nodes.map((node) => node.dataset.testid),
    ),
  };
  check(
    results.row_icons.detail === 1 &&
      results.row_icons.password === 1 &&
      results.row_icons.delete === 1 &&
      results.row_icons.order.join(",") ===
        "admin-user-action-detail,admin-user-action-password,admin-user-action-delete",
    "row_icon_trio",
    JSON.stringify(results.row_icons),
  );

  const typeSelect = target.locator("[data-testid='admin-user-type-select']");
  results.type_select = {
    value: await typeSelect.inputValue(),
    options: await typeSelect.locator("option").allInnerTexts(),
    ariaLabel: await typeSelect.getAttribute("aria-label"),
  };
  check(
    results.type_select.value === "normal" &&
      results.type_select.options.join("|") === "Normal|Superuser" &&
      (results.type_select.ariaLabel || "").includes(THROWAWAY),
    "type_select_shape",
    JSON.stringify(results.type_select),
  );

  // Grant → `PUT …/admin`, then revoke → `DELETE …/admin`.
  calls.length = 0;
  await typeSelect.selectOption("superuser");
  await page.waitForTimeout(900);
  const afterGrant = await target.locator("[data-testid='admin-user-type-select']").inputValue();
  const encodedTarget = encodeURIComponent(THROWAWAY);
  const grantCalls = calls.filter((call) => call.endsWith(`/admin/users/${encodedTarget}/admin`));
  check(
    grantCalls.length === 1 && grantCalls[0].startsWith("PUT ") && afterGrant === "superuser",
    "grant_superuser_put",
    `${grantCalls.join(",")} → ${afterGrant}`,
  );

  calls.length = 0;
  await target.locator("[data-testid='admin-user-type-select']").selectOption("normal");
  await page.waitForTimeout(900);
  const afterRevoke = await target.locator("[data-testid='admin-user-type-select']").inputValue();
  const revokeCalls = calls.filter((call) => call.endsWith(`/admin/users/${encodedTarget}/admin`));
  check(
    revokeCalls.length === 1 && revokeCalls[0].startsWith("DELETE ") && afterRevoke === "normal",
    "revoke_superuser_delete",
    `${revokeCalls.join(",")} → ${afterRevoke}`,
  );

  // `UserLock` opens the change-password dialog targeting that account.
  await target.hover();
  await target.locator("[data-testid='admin-user-action-password']").click();
  await page.waitForSelector("#adminPasswordDialog.show", { timeout: 10000 });
  results.password_dialog = {
    email: await page.locator("[data-testid='admin-password-email']").inputValue(),
  };
  check(results.password_dialog.email === THROWAWAY, "password_icon_targets_row", results.password_dialog.email);
  await page.click("#adminPasswordDialog .admin-dialog-actions .btn-o");
  await page.waitForTimeout(300);

  // The danger `Trash2` opens the delete confirmation for the same account.
  await target.hover();
  await target.locator("[data-testid='admin-user-action-delete']").click();
  await page.waitForSelector("#adminDeleteUserDialog.show", { timeout: 10000 });
  results.delete_dialog = {
    target: await text("#adminDeleteTarget"),
    confirmation: await text("#adminDeleteUserDialog .admin-dialog-sub"),
    box: await page.locator("#adminDeleteTarget").evaluate((node) => {
      const style = getComputedStyle(node);
      return `${style.borderTopWidth} ${style.borderTopStyle} ${style.borderRadius} ${style.marginTop} ${style.paddingTop}`;
    }),
  };
  check(
    results.delete_dialog.target === THROWAWAY &&
      results.delete_dialog.confirmation === "Are you sure you want to delete this user?" &&
      /^(0\.5|1)px solid 8px 24px 16px$/.test(results.delete_dialog.box),
    "delete_icon_targets_row",
    JSON.stringify(results.delete_dialog),
  );
  await page.click("#adminDeleteUserDialog .admin-dialog-actions .btn-o");
  await page.waitForTimeout(300);

  // `ClipboardList` navigates to the account page.
  await target.hover();
  await target.locator("[data-testid='admin-user-action-detail']").click();
  await page.waitForURL(`**/admin/users/${encodeURIComponent(THROWAWAY)}`, { timeout: 15000 });
  results.detail_navigation = page.url().replace(ORIGIN, "");
  check(
    results.detail_navigation === `/admin/users/${encodeURIComponent(THROWAWAY)}`,
    "detail_icon_navigates",
    results.detail_navigation,
  );

  check(pageErrors.length === 0, "no_page_errors", pageErrors.join(" | "));
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/admin-users-cells-failure.png" }).catch(() => {});
} finally {
  if (created) {
    const cleaned = await page
      .evaluate(async (email) => {
        const token = localStorage.getItem("rayrag_token") || "";
        const res = await fetch("/api/v1/admin/users/" + encodeURIComponent(email), {
          method: "DELETE",
          headers: { Authorization: "Bearer " + token },
        });
        const data = await res.json().catch(() => ({}));
        return { status: res.status, code: data.code };
      }, THROWAWAY)
      .catch(() => ({ status: 0 }));
    results.cleanup_deleted = cleaned.code === 0 ? "ok" : "FAIL";
    if (cleaned.code !== 0) results.errors.push(`cleanup_deleted (${JSON.stringify(cleaned)})`);
  }
  results.calls = calls;
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
