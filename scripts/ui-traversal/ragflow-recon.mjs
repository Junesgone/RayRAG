// Reconnaissance for the RAGFlow UI at RAGFLOW_ORIGIN: visit a list of routes in
// the persistent Chrome profile and dump what actually renders (title, visible
// text, buttons/links/fields). Read-only: nothing is clicked or submitted.
//
// Usage: RAGFLOW_ORIGIN=http://127.0.0.1:11526 node scripts/ui-traversal/ragflow-recon.mjs /chunk /dataflow-result
import { chromium } from "playwright-core";
import { mkdirSync, writeFileSync } from "node:fs";

const ORIGIN = process.env.RAGFLOW_ORIGIN || "http://127.0.0.1:11526";
const routes = process.argv.slice(2);
if (!routes.length) {
  console.error("usage: node ragflow-recon.mjs <route> [route...]");
  process.exit(2);
}

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1500, height: 1000 });
mkdirSync("docs/ui-traversal/ragflow-pages", { recursive: true });
const out = { origin: ORIGIN, generatedAt: new Date().toISOString(), pages: [] };

for (const route of routes) {
  const entry = { route, ok: false };
  try {
    // The SPA serves one index for every route and the browser container's
    // network hiccups, so navigation is retried instead of failing the run.
    let lastError = null;
    for (let attempt = 0; attempt < 3; attempt += 1) {
      try {
        await page.goto(ORIGIN + route, { waitUntil: "domcontentloaded", timeout: 40000 });
        lastError = null;
        break;
      } catch (error) {
        lastError = error;
        await page.waitForTimeout(1500);
      }
    }
    if (lastError) throw lastError;
    await page.waitForTimeout(3000);
    entry.finalUrl = page.url();
    entry.title = await page.title();
    entry.state = await page.evaluate(() => {
      const text = (document.body?.innerText || "").replace(/\s+/g, " ").trim();
      const visible = (node) => {
        const rect = node.getBoundingClientRect();
        const style = getComputedStyle(node);
        return rect.width > 0 && rect.height > 0 && style.visibility !== "hidden" && style.display !== "none";
      };
      const collect = (selector, map) =>
        Array.from(document.querySelectorAll(selector))
          .filter(visible)
          .map(map)
          .filter(Boolean);
      return {
        path: location.pathname + location.search,
        head: text.slice(0, 600),
        buttons: collect("button", (n) => (n.innerText || n.getAttribute("aria-label") || "").trim()),
        links: collect("a", (n) => (n.innerText || "").trim()),
        fields: collect("input,textarea,select", (n) => n.id || n.name || n.placeholder || n.type),
        headers: collect("th", (n) => (n.innerText || "").trim()),
        tabs: collect("[role=tab]", (n) => (n.innerText || "").trim()),
        testids: Array.from(document.querySelectorAll("[data-testid]"))
          .filter(visible)
          .map((n) => n.getAttribute("data-testid"))
          .slice(0, 60),
      };
    });
    entry.ok = true;
    await page.screenshot({
      path: `docs/ui-traversal/ragflow-pages/recon${route.replace(/[^A-Za-z0-9]+/g, "_")}.png`,
    });
  } catch (error) {
    entry.error = String(error && error.message ? error.message : error);
  }
  out.pages.push(entry);
}

writeFileSync("docs/ui-traversal/ragflow-pages/recon.json", JSON.stringify(out, null, 1));
console.log(JSON.stringify(out, null, 1));
await page.close();
await b.close();
