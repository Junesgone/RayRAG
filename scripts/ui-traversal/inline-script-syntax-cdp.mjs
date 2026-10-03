// Every page's inline JavaScript must parse.
//
// Page scripts live inside Rust string literals. A stray quote closes the literal early, an escaped
// quote inside a single-quoted HTML attribute is not an escape at all, and a missing `;` silently
// glues two statements together. The page still returns HTTP 200 and the HTML still balances its
// tags, so only the browser notices - as a console error, or as raw source text printed on screen.
// This check runs the same parser the browser uses over every inline <script> on every page.
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const BASE = process.env.RAYRAG_BASE_URL || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}
const OUT = "docs/ui-traversal";
mkdirSync(OUT, { recursive: true });

const PAGES = [
  "/", "/chats", "/chat", "/search", "/agents", "/memories", "/files", "/datasets",
  "/datasets/new", "/knowledge", "/user-setting", "/admin", "/admin/services", "/admin/users",
  "/admin/whitelist", "/admin/roles", "/admin/monitoring", "/admin/sandbox-settings",
];

const browser = await chromium.connectOverCDP(
  process.env.RAYRAG_CDP_URL || "http://127.0.0.1:16002",
);
const ctx = browser.contexts()[0] || (await browser.newContext());
const page = await ctx.newPage();
await page.goto(BASE + "/login", { waitUntil: "domcontentloaded", timeout: 60000 });
if (page.url().includes("/login")) {
  await page.fill("input[type=email], input[name=email]", "admin@rayrag.local").catch(() => {});
  await page.fill("input[type=password]", PASSWORD).catch(() => {});
  await page.click("button[type=submit]").catch(() => {});
  await page.waitForTimeout(2500);
}

const problems = [];
const report = {};
for (const path of PAGES) {
  const response = await page.goto(BASE + path, { waitUntil: "domcontentloaded", timeout: 60000 });
  const status = response ? response.status() : 0;
  const html = await page.content();
  const scripts = [...html.matchAll(/<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/gi)].map(
    (m) => m[1],
  );
  const failures = [];
  for (const [index, body] of scripts.entries()) {
    if (!body.trim()) continue;
    try {
      // Same parser the browser uses for a classic script.
      new Function(body);
    } catch (error) {
      failures.push({
        block: index,
        message: String(error.message).slice(0, 160),
        head: body.trim().slice(0, 120),
      });
    }
  }
  report[path] = { status, scripts: scripts.length, failures };
  if (failures.length || status >= 400) problems.push(path);
}
console.log(JSON.stringify({ report, problems }, null, 1));
await page.close();
await browser.close();
process.exit(problems.length ? 1 : 0);
