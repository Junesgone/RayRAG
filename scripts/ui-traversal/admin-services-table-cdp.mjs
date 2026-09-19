// CDP probe for the admin services table
// (`web/src/pages/admin/service-status.tsx`): the global search box filters rows
// across id/name/type/host/port, the card footer paginates (Total N + page size
// 10/20/50/100) and `Reset` clears both the type filter and the search term.
// Real clicks; nothing is mutated.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/admin-services-table-cdp.mjs
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

const rows = () =>
  page.evaluate(() =>
    Array.from(document.querySelectorAll("[data-testid='admin-service-row']")).map((row) =>
      Array.from(row.querySelectorAll("td")).map((cell) => cell.textContent.trim()),
    ),
  );
const pager = (id = "adminServicePager") =>
  page.evaluate((pagerId) => {
    const host = document.getElementById(pagerId);
    return {
      text: host ? host.textContent.replace(/\s+/g, " ").trim() : null,
      size: host ? host.querySelector(".admin-pager-size").value : null,
    };
  }, id);

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
    // the console pages are guarded server-side, which reads the cookie
    document.cookie = "rayrag_token=" + data.data.access_token + ";path=/";
    return { ok: true };
  }, PASSWORD);
  if (!login.ok) throw new Error("login failed: " + JSON.stringify(login.body));
  await page.goto(`${ORIGIN}/admin/services`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='admin-service-row']", { timeout: 20000 });
  results.initial = { rows: (await rows()).length, pager: await pager() };
  // Global search narrows by service name.
  await page.fill("#adminServiceSearch", "postgres");
  await page.waitForTimeout(250);
  results.searched = { rows: await rows(), pager: await pager() };
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/admin-services-search-20260821.png" });
  // Reset clears the search box and restores every row.
  await page.click("button:has-text('Reset')");
  await page.waitForTimeout(250);
  results.reset = { rows: (await rows()).length, search: await page.inputValue("#adminServiceSearch") };
  // Page size switch keeps the table populated and the footer in sync.
  await page.selectOption("[data-testid='admin-services-page-size']", "20");
  await page.waitForTimeout(250);
  results.pageSize = { rows: (await rows()).length, pager: await pager() };
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/admin-services-20260821.png" });
  // Extra-info dialog: the payload renders as a JsonView-style typed tree.
  await page.click("button:has-text('Show details')");
  await page.waitForSelector(".json-tree-list", { timeout: 10000 });
  results.details = await page.evaluate(() => {
    const dialog = document.getElementById("adminServiceDialog");
    const toggles = Array.from(dialog.querySelectorAll(".json-tree-toggle"));
    return {
      title: document.getElementById("adminServiceName").textContent,
      nodeCount: dialog.querySelectorAll(".json-tree-node").length,
      firstToggle: toggles.length ? toggles[0].textContent : null,
      keys: Array.from(dialog.querySelectorAll(".json-tree-key"))
        .slice(0, 4)
        .map((node) => node.textContent),
      valueClasses: Array.from(
        new Set(
          Array.from(dialog.querySelectorAll(".json-tree-value")).map((node) =>
            Array.from(node.classList).find((name) => name.startsWith("json-tree-") && name !== "json-tree-value"),
          ),
        ),
      ),
      rawJsonLeaked: dialog.textContent.includes('{"'),
    };
  });
  // A toggle collapses its child list for real.
  await page.click(".json-tree-toggle");
  await page.waitForTimeout(150);
  results.collapsed = await page.evaluate(() => {
    const toggle = document.querySelector(".json-tree-toggle");
    return {
      expanded: toggle.getAttribute("aria-expanded"),
      listHidden: toggle.parentElement.querySelector(".json-tree-list").hidden,
    };
  });
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/admin-service-details-20260821.png" });

  // `/admin/users`: global search + sortable headers + footer pagination.
  await page.goto(`${ORIGIN}/admin/users`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='admin-user-row']", { timeout: 20000 });
  const userRows = () =>
    page.evaluate(() =>
      Array.from(document.querySelectorAll("[data-testid='admin-user-row']")).map((row) =>
        row.querySelector("td").textContent.trim(),
      ),
    );
  results.usersInitial = { rows: (await userRows()).length, pager: await pager("adminUserPager") };
  await page.click("[data-testid='admin-users-sort-email']");
  await page.waitForTimeout(250);
  results.usersSorted = {
    rows: await userRows(),
    mark: await page.textContent("#adminUserSortEmail"),
  };
  await page.click("[data-testid='admin-users-sort-email']");
  await page.waitForTimeout(250);
  results.usersSortedDesc = { rows: await userRows(), mark: await page.textContent("#adminUserSortEmail") };
  await page.fill("#adminUserSearch", "zzz-no-such-user");
  await page.waitForTimeout(250);
  results.usersSearched = { rows: (await userRows()).length, pager: await pager("adminUserPager") };
  await page.click("button:has-text('Reset')");
  await page.waitForTimeout(250);
  results.usersReset = {
    rows: (await userRows()).length,
    search: await page.inputValue("#adminUserSearch"),
    mark: await page.textContent("#adminUserSortEmail"),
  };
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/admin-users-20260821.png" });
} catch (error) {
  results.errors.push(String(error && error.stack ? error.stack : error));
} finally {
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
}
if (results.errors.length || results.pageErrors.length) process.exit(1);
