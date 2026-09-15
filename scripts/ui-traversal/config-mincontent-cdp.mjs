import { chromium } from "playwright-core";
const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0]; const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
await page.goto("http://127.0.0.1:9390/login", { waitUntil: "domcontentloaded" });
await page.evaluate(async (pw) => { const r = await fetch("/api/v1/auth/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: "admin@rayrag.local", password: pw }) }); const d = await r.json(); if (d.code === 0) localStorage.setItem("rayrag_token", d.data.access_token); }, process.env.UI_DUMP_PASSWORD);
await page.goto(`http://127.0.0.1:9390/dataset/configuration/${process.env.RAYRAG_KB}`, { waitUntil: "domcontentloaded" });
await page.waitForTimeout(2000);
console.log(JSON.stringify(await page.evaluate(() => {
  const table = document.querySelector("#tab3 table");
  const clone = table.cloneNode(true);
  clone.style.width = "min-content";
  clone.style.position = "absolute";
  clone.style.visibility = "hidden";
  document.body.appendChild(clone);
  const rows = Array.from(clone.querySelectorAll("tr")).map((tr) => ({
    text: (tr.textContent || "").replace(/\s+/g, " ").trim().slice(0, 30),
    cells: Array.from(tr.children).map((td) => Math.round(td.getBoundingClientRect().width)),
  }));
  const total = Math.round(clone.getBoundingClientRect().width);
  clone.remove();
  return { minContentTable: total, rows: rows.sort((a, b) => (b.cells[1] || 0) - (a.cells[1] || 0)).slice(0, 6) };
}), null, 1));
await page.close(); process.exit(0);
