// Locate horizontal-overflow sources on a RayRAG page.
// Usage: UI_DUMP_PASSWORD=... [RAYRAG_PATH=/dataset/configuration/<kb>] node scripts/ui-traversal/overflow-cdp.mjs
import { chromium } from "playwright-core";
const CDP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
const b = await chromium.connectOverCDP(CDP);
const ctx = b.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded" });
await page.evaluate(async (pw) => {
  const r = await fetch("/api/v1/auth/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: "admin@rayrag.local", password: pw }) });
  const d = await r.json();
  if (d.code === 0) localStorage.setItem("rayrag_token", d.data.access_token);
}, process.env.RAYRAG_LOGIN_PASSWORD || process.env.UI_DUMP_PASSWORD);
const path = process.env.RAYRAG_PATH || (await page.evaluate(async () => `/dataset/configuration/${(await (async () => { const list = await (await fetch("/api/v1/datasets")).json(); if (list.data && list.data.length) return list.data[0].id; const created = await (await fetch("/api/v1/datasets", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ name: "overflow-probe-" + Date.now() }) })).json(); return created.data && created.data.id; })())}`));
await page.goto(ORIGIN + path, { waitUntil: "domcontentloaded" });
await page.waitForTimeout(2500);
const out = await page.evaluate(() => {
  const wide = [];
  document.querySelectorAll("*").forEach((el) => {
    if (el.scrollWidth > 1600 || el.getBoundingClientRect().width > 1610) {
      const r = el.getBoundingClientRect();
      wide.push({ tag: el.tagName, id: el.id, cls: String(el.className).slice(0, 60), w: Math.round(r.width), sw: el.scrollWidth, display: getComputedStyle(el).display });
    }
  });
  return { path: location.pathname, docScrollW: document.documentElement.scrollWidth, clientW: document.documentElement.clientWidth, wide: wide.slice(0, 60) };
});
console.log(JSON.stringify(out, null, 1));
await page.close();
process.exit(0);
