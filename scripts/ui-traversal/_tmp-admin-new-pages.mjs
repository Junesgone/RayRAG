import { chromium } from "playwright-core";
const BASE = "http://127.0.0.1:9390";
const browser = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = browser.contexts()[0] || (await browser.newContext());
const page = await ctx.newPage();
const errors = [];
page.on("pageerror", (e) => errors.push(String(e).slice(0, 160)));
await page.goto(BASE + "/admin", { waitUntil: "domcontentloaded" });
if (page.url().includes("/admin") && (await page.locator("input[type=password]").count())) {
  await page.fill("input[type=email], input[name=email]", "admin@rayrag.local").catch(() => {});
  await page.fill("input[type=password]", "rayrag-smoke-admin").catch(() => {});
  await page.click("button[type=submit]").catch(() => {});
  await page.waitForTimeout(3000);
}
const out = {};
for (const path of ["/admin/whitelist", "/admin/roles", "/admin/monitoring"]) {
  await page.goto(BASE + path, { waitUntil: "networkidle", timeout: 60000 });
  await page.waitForTimeout(1500);
  out[path] = await page.evaluate(() => ({
    title: document.title,
    navLinks: [...document.querySelectorAll(".admin-console-nav-link")].map((a) => a.textContent.trim()),
    rows: document.querySelectorAll("tbody tr").length,
    status: (document.getElementById("wlStatus") || document.getElementById("roleStatus") || document.getElementById("monStatus") || {}).textContent || "",
    metrics: document.querySelectorAll(".admin-metric").length,
    body: (document.body.innerText || "").slice(0, 160).replace(/\n+/g, " | "),
  }));
}
// The dataset tabs must not carry the chunk-method explainer any more.
await page.goto(BASE + "/datasets", { waitUntil: "networkidle", timeout: 60000 });
const firstDataset = await page.locator("a[href^='/datasets/']").first().getAttribute("href").catch(() => null);
if (firstDataset) {
  await page.goto(BASE + firstDataset, { waitUntil: "networkidle", timeout: 60000 });
  await page.waitForTimeout(1200);
  out.dataset = await page.evaluate(() => {
    const tabs = [...document.querySelectorAll("[data-dest], .ds-tab, [role=tab]")].map((t) => t.textContent.trim());
    const text = document.body.innerText || "";
    const intro = text.match(/Built-in pipeline[^\n]*/g) || [];
    return { tabs: tabs.slice(0, 8), introOccurrences: intro.length, introSample: intro.slice(0, 2) };
  });
}
console.log(JSON.stringify({ out, errors }, null, 1));
await page.close();
await browser.close();
