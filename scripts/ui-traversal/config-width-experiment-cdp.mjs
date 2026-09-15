import { chromium } from "playwright-core";
const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0]; const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
await page.goto("http://127.0.0.1:9390/login", { waitUntil: "domcontentloaded" });
await page.evaluate(async (pw) => { const r = await fetch("/api/v1/auth/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: "admin@rayrag.local", password: pw }) }); const d = await r.json(); if (d.code === 0) localStorage.setItem("rayrag_token", d.data.access_token); }, process.env.UI_DUMP_PASSWORD);
await page.goto(`http://127.0.0.1:9390/dataset/configuration/${process.env.RAYRAG_KB}`, { waitUntil: "domcontentloaded" });
await page.waitForTimeout(2000);
const report = await page.evaluate(() => {
  const results = [];
  const measure = (label) => results.push({ label, doc: document.documentElement.scrollWidth, content: Math.round(document.querySelector(".ds-content").getBoundingClientRect().width) });
  measure("baseline");
  const hide = (sel) => { const el = document.querySelector(sel); if (el) { el.dataset.prevDisplay = el.style.display; el.style.display = "none"; } };
  const restore = (sel) => { const el = document.querySelector(sel); if (el) el.style.display = el.dataset.prevDisplay || ""; };
  hide("#cfgPermSelect"); measure("no permission select"); restore("#cfgPermSelect");
  hide("#cfgAvatarUpload"); measure("no avatar upload"); restore("#cfgAvatarUpload");
  hide("#cfgLang"); measure("no language select"); restore("#cfgLang");
  const lang = document.querySelector("#cfgLang");
  if (lang) { const prev = lang.style.maxWidth; lang.style.maxWidth = "180px"; measure("language 180px"); lang.style.maxWidth = prev; }
  const table = document.querySelector("#tab3 table");
  if (table) { table.style.tableLayout = "fixed"; measure("table fixed"); table.style.tableLayout = ""; }
  return results;
});
console.log(JSON.stringify(report, null, 1));
await page.close(); process.exit(0);
