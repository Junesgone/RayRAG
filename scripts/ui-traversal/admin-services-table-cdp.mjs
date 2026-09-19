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
const pager = () =>
  page.evaluate(() => {
    const host = document.getElementById("adminServicePager");
    return {
      text: host ? host.textContent.replace(/\s+/g, " ").trim() : null,
      size: document.getElementById("adminServicePager")
        ? document.querySelector(".admin-pager-size").value
        : null,
    };
  });

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
} catch (error) {
  results.errors.push(String(error && error.stack ? error.stack : error));
} finally {
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
}
if (results.errors.length || results.pageErrors.length) process.exit(1);
