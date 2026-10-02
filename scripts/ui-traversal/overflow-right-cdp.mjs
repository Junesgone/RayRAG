import { chromium } from "playwright-core";
const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0]; const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
await page.goto("http://127.0.0.1:9390/login", { waitUntil: "domcontentloaded" });
await page.evaluate(async (pw) => { const r = await fetch("/api/v1/auth/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: "admin@rayrag.local", password: pw }) }); const d = await r.json(); if (d.code === 0) localStorage.setItem("rayrag_token", d.data.access_token); }, process.env.RAYRAG_LOGIN_PASSWORD || process.env.UI_DUMP_PASSWORD);
const kb = process.env.RAYRAG_KB;
await page.goto(`http://127.0.0.1:9390/dataset/configuration/${kb}`, { waitUntil: "domcontentloaded" });
await page.waitForTimeout(2200);
console.log(JSON.stringify(await page.evaluate(() => {
  const out = [];
  document.querySelectorAll("*").forEach((el) => {
    const r = el.getBoundingClientRect();
    const overflowRight = r.right - document.documentElement.clientWidth;
    if (overflowRight > 2) out.push({ tag: el.tagName, id: el.id, cls: String(el.className).slice(0, 40), right: Math.round(r.right), w: Math.round(r.width) });
  });
  return { overflowing: out.slice(0, 12), count: out.length };
}), null, 1));
await page.close(); process.exit(0);
