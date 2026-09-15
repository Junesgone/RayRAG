import { chromium } from "playwright-core";
const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0]; const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
await page.goto("http://127.0.0.1:9390/login", { waitUntil: "domcontentloaded" });
await page.evaluate(async (pw) => { const r = await fetch("/api/v1/auth/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: "admin@rayrag.local", password: pw }) }); const d = await r.json(); if (d.code === 0) localStorage.setItem("rayrag_token", d.data.access_token); }, process.env.UI_DUMP_PASSWORD);
await page.goto(`http://127.0.0.1:9390/dataset/configuration/${process.env.RAYRAG_KB}`, { waitUntil: "domcontentloaded" });
await page.waitForTimeout(2200);
console.log(JSON.stringify(await page.evaluate(() => {
  const table = document.querySelector("#tab3 table");
  const rows = Array.from(table.querySelectorAll("tr")).slice(0, 12).map((tr) => {
    const cells = Array.from(tr.children).map((td) => Math.round(td.getBoundingClientRect().width));
    const widest = Array.from(tr.querySelectorAll("*")).map((el) => ({ tag: el.tagName, id: el.id, w: Math.round(el.getBoundingClientRect().width) })).sort((a, b) => b.w - a.w)[0];
    return { text: (tr.textContent || "").replace(/\s+/g, " ").trim().slice(0, 24), cells, widest };
  });
  return { tableWidth: Math.round(table.getBoundingClientRect().width), rows };
}), null, 1));
await page.close(); process.exit(0);
