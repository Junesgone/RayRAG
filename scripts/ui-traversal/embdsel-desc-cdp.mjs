import { chromium } from "playwright-core";
const CDP = "http://127.0.0.1:16002", ORIGIN = "http://127.0.0.1:9390";
const b = await chromium.connectOverCDP(CDP); const ctx = b.contexts()[0]; const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded" });
await page.evaluate(async (pw) => { const r = await fetch("/api/v1/auth/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: "admin@rayrag.local", password: pw }) }); const d = await r.json(); if (d.code === 0) localStorage.setItem("rayrag_token", d.data.access_token); }, process.env.RAYRAG_LOGIN_PASSWORD || process.env.UI_DUMP_PASSWORD);
const kb = await page.evaluate(async () => {
  const list = await (await fetch("/api/v1/datasets")).json();
  const rows = list.data || [];
  if (rows.length) return rows[0].id;
  const created = await (await fetch("/api/v1/datasets", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ name: "probe-" + Date.now() }) })).json();
  return created.data && created.data.id;
});
await page.goto(`${ORIGIN}/dataset/configuration/${kb}`, { waitUntil: "domcontentloaded" });
await page.waitForTimeout(2500);
console.log(JSON.stringify(await page.evaluate(() => {
  const btn = document.getElementById("embdSel");
  const pop = document.getElementById("embdSelPopover");
  const rows = [];
  btn.querySelectorAll("*").forEach((el) => { const r = el.getBoundingClientRect(); if (r.width > 5) rows.push({ tag: el.tagName, id: el.id, cls: String(el.className).slice(0, 40), w: Math.round(r.width), x: Math.round(r.x), disp: getComputedStyle(el).display }); });
  return { hiddenProp: pop.hidden, hiddenAttr: pop.hasAttribute("hidden"), btnWhiteSpace: getComputedStyle(btn).whiteSpace, btnOverflow: getComputedStyle(btn).overflow, wideDesc: rows.slice(0, 10), wideCount: rows.length, treeKids: document.getElementById("embdSelTree").children.length, firstKidTag: document.getElementById("embdSelTree").firstElementChild ? document.getElementById("embdSelTree").firstElementChild.tagName + "/" + document.getElementById("embdSelTree").firstElementChild.className : "" };
}), null, 1));
await page.close(); process.exit(0);
