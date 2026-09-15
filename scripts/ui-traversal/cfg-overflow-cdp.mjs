import { chromium } from "playwright-core";
const CDP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = "http://127.0.0.1:9390";
const b = await chromium.connectOverCDP(CDP);
const ctx = b.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
const errs = []; page.on("pageerror", e => errs.push(String(e).slice(0,200)));
await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded" });
await page.evaluate(async (pw) => {
  const r = await fetch("/api/v1/auth/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: "admin@rayrag.local", password: pw }) });
  const d = await r.json();
  if (d.code === 0) localStorage.setItem("rayrag_token", d.data.access_token);
}, process.env.UI_DUMP_PASSWORD);
const kb = await page.evaluate(async () => (await (await fetch("/api/v1/datasets")).json()).data[0].id);
await page.goto(`${ORIGIN}/dataset/configuration/${kb}`, { waitUntil: "domcontentloaded" });
await page.waitForTimeout(2500);
const out = await page.evaluate(() => {
  const g = (id) => document.getElementById(id);
  const rect = (el) => { if (!el) return null; const r = el.getBoundingClientRect(); return { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) }; };
  const trig = g("embdSel"), pop = g("embdSelPopover"), tree = g("embdSelTree"), val = g("embdSelValue");
  const cs = pop ? getComputedStyle(pop) : null;
  const rows = Array.from(document.querySelectorAll("table tr")).map(tr => (tr.textContent||"").replace(/\s+/g," ").trim().slice(0,40));
  const mid = rect(trig);
  const at = mid ? document.elementFromPoint(mid.x + 20, mid.y + mid.h/2) : null;
  return {
    trigger: rect(trig), popover: rect(pop), tree: rect(tree), value: rect(val),
    popDisplay: cs && cs.display, popVisibility: cs && cs.visibility,
    triggerTextLen: trig ? trig.textContent.length : 0, triggerScrollW: trig ? trig.scrollWidth : 0,
    valueHTML: val ? val.innerHTML.slice(0, 200) : "",
    treeDisplay: tree ? getComputedStyle(tree).display : "",
    popChildren: pop ? pop.children.length : 0,
    docScrollW: document.documentElement.scrollWidth,
    cardRect: rect(document.querySelector(".ds-content .card")),
    treeChildren: tree ? tree.children.length : 0,
    bodyScrollW: document.body.scrollWidth, clientW: document.documentElement.clientWidth,
    elementAt: at ? (at.id || at.className || at.tagName) : "",
    rowLabels: rows.slice(0, 20),
    avatarRow: (() => { const tds = Array.from(document.querySelectorAll("table tr")).find(tr => (tr.textContent||"").includes("Avatar")); return tds ? tds.innerHTML.replace(/\s+/g," ").slice(0, 300) : ""; })(),
  };
});
out.errors = errs;
console.log(JSON.stringify(out, null, 1));
await page.screenshot({ path: "/tmp/rayrag-shots/cfg-overflow.png" }).catch(() => {});
await page.close();
process.exit(0);
