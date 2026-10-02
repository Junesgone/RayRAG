// Verify the retrieval-testing Use-knowledge-graph switch and reranking label.
import { chromium } from "playwright-core";
const CDP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
const b = await chromium.connectOverCDP(CDP); const ctx = b.contexts()[0]; const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
const errs = []; page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded" });
await page.evaluate(async (pw) => { const r = await fetch("/api/v1/auth/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: "admin@rayrag.local", password: pw }) }); const d = await r.json(); if (d.code === 0) localStorage.setItem("rayrag_token", d.data.access_token); }, process.env.RAYRAG_LOGIN_PASSWORD || process.env.UI_DUMP_PASSWORD);
const cdp = await ctx.newCDPSession(page);
await cdp.send("Network.clearBrowserCache").catch(() => {});
const kb = await page.evaluate(async () => {
  const list = await (await fetch("/api/v1/datasets")).json();
  const rows = list.data || [];
  if (rows.length) return rows[0].id;
  const created = await (await fetch("/api/v1/datasets", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ name: "testing-switch-probe-" + Date.now() }) })).json();
  return created.data && created.data.id;
});
await page.goto(`${ORIGIN}/dataset/retrieval/${kb}`, { waitUntil: "domcontentloaded" });
await page.waitForTimeout(2000);
console.log(JSON.stringify(await page.evaluate(() => {
  const cb = document.getElementById("testUseKg");
  const cs = cb ? getComputedStyle(cb) : null;
  const r = cb ? cb.getBoundingClientRect() : null;
  const left = document.querySelector("#tab2 .card > div > div");
  return {
    switchStyle: cs ? { appearance: cs.appearance, width: cs.width, height: cs.height, radius: cs.borderRadius, background: cs.backgroundColor } : null,
    switchRect: r ? { w: Math.round(r.width), h: Math.round(r.height) } : null,
    settingColumnText: left ? (left.textContent || "").replace(/\s+/g, " ").trim().slice(0, 200) : "",
    settingColumnHTMLHead: left ? left.innerHTML.slice(0, 400) : "",
    rerankLabel: (() => { const l = document.querySelector("#testRerankingSelLabel"); return l ? (l.textContent || "").trim() : ""; })(),
    switchOuter: cb ? cb.outerHTML.slice(0, 140) : "",
    switchParent: cb && cb.parentElement ? cb.parentElement.className : "",
    cssHasSwitchRule: Array.from(document.querySelectorAll("style")).some((s) => s.textContent.includes(".set-switch input[type=checkbox],")),
    cssomRules: (() => { const hits = []; for (const sheet of Array.from(document.styleSheets)) { let rules = []; try { rules = Array.from(sheet.cssRules); } catch (e) { continue; } for (const rule of rules) { if (rule.selectorText && rule.selectorText.includes("set-switch")) hits.push(rule.selectorText.slice(0, 120)); } } return hits.slice(0, 6); })(),
    cssomRuleCount: (() => { let n = 0; for (const sheet of Array.from(document.styleSheets)) { try { n += sheet.cssRules.length; } catch (e) { n += -1; } } return n; })(),
    matchesSwitchSelector: cb ? cb.matches(".set-switch input[type=checkbox]") : null,
    lastSelectors: (() => { const all = []; for (const sheet of Array.from(document.styleSheets)) { let rules = []; try { rules = Array.from(sheet.cssRules); } catch (e) { continue; } for (const rule of rules) if (rule.selectorText) all.push(rule.selectorText.slice(0, 90)); } return all.slice(-8); })(),
    strayTag: !!document.querySelector("reranking"),
    strayTagOuter: document.querySelector("reranking") ? document.querySelector("reranking").outerHTML.slice(0, 160) : "",
  };
}), null, 1));
console.log("errors", JSON.stringify(errs));
await page.screenshot({ path: `${process.env.SHOT_DIR || "/tmp"}/rayrag-testing-switch.png` });
await page.close(); process.exit(0);
