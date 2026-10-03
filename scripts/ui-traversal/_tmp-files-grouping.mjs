import { chromium } from "playwright-core";
const BASE = "http://127.0.0.1:9390";
const browser = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = browser.contexts()[0] || (await browser.newContext());
const page = await ctx.newPage();
const errors = [];
page.on("pageerror", (e) => errors.push(String(e).slice(0, 160)));
await page.goto(BASE + "/login", { waitUntil: "domcontentloaded" });
if (page.url().includes("/login")) {
  await page.fill("input[type=email], input[name=email]", "admin@rayrag.local").catch(() => {});
  await page.fill("input[type=password]", "rayrag-smoke-admin").catch(() => {});
  await page.click("button[type=submit]").catch(() => {});
  await page.waitForTimeout(2500);
}
await page.goto(BASE + "/files", { waitUntil: "networkidle", timeout: 60000 });
await page.waitForTimeout(1800);
const top = await page.evaluate(() => ({
  folders: [...document.querySelectorAll("[data-testid=files-dataset-folder]")].map((c) => c.textContent.trim()),
  gridColumns: getComputedStyle(document.getElementById("fileDsGrid") || document.body).gridTemplateColumns,
  tableRows: document.querySelectorAll("#fl tr").length,
}));
// Open the dataset folder and confirm the table now lists that dataset's files.
let opened = null;
const card = page.locator("[data-testid=files-dataset-folder]").filter({ hasText: "folders-ui-check" }).first();
if (await card.count()) {
  await card.click();
  await page.waitForTimeout(1800);
  opened = await page.evaluate(() => ({
    breadcrumb: (document.getElementById("fileBreadcrumb") || {}).textContent || "",
    rows: [...document.querySelectorAll("#fl tr")].map((r) => r.textContent.trim().replace(/\s+/g, " ").slice(0, 80)),
    backVisible: !(document.getElementById("fileDsBack") || {}).hidden,
  }));
  await page.click("[data-testid=files-dataset-back]");
  await page.waitForTimeout(1200);
}
// Narrow viewport: the grid must reflow rather than overflow.
await page.setViewportSize({ width: 480, height: 900 });
await page.waitForTimeout(600);
const narrow = await page.evaluate(() => ({
  overflowX: document.documentElement.scrollWidth - document.documentElement.clientWidth,
  columns: getComputedStyle(document.getElementById("fileDsGrid") || document.body).gridTemplateColumns,
}));
console.log(JSON.stringify({ top, opened, narrow, errors }, null, 1));
await page.close();
await browser.close();
