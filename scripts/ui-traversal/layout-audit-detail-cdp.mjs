import { chromium } from "playwright-core";
const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9485";
const CDP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
const SHOTS = "docs/ui-traversal/rayrag-audit";
const b = await chromium.connectOverCDP(CDP);
const ctx = await b.newContext();
const page = await ctx.newPage();
const report = { pages: {}, totals: { overflow: 0, invisible: 0, truncated: 0, lowContrast: 0, small: 0, ragflow: 0 } };
const scan = async (slug) => {
  const entry = { counts: {}, offenders: {}, overflowX: 0 };
  try {
    const data = await page.evaluate(() => {
      const vw = window.innerWidth;
      const sw = document.documentElement.scrollWidth;
      const short = (el) => {
        const cls = el.className && typeof el.className === "string" ? el.className.split(/\s+/).find(Boolean) : "";
        return el.tagName.toLowerCase() + (el.id ? "#" + el.id : "") + (cls ? "." + cls : "");
      };
      const lum = (c) => {
        const m = c.match(/rgba?\((\d+), ?(\d+), ?(\d+)(?:, ?([\d.]+))?\)/);
        if (!m) return null;
        if (m[4] !== undefined && parseFloat(m[4]) < 0.95) return null;
        const f = (v) => { v = v / 255; return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); };
        return 0.2126 * f(+m[1]) + 0.7152 * f(+m[2]) + 0.0722 * f(+m[3]);
      };
      const opaque = (bg) => {
        const m = bg && bg.match(/rgba?\((\d+), ?(\d+), ?(\d+)(?:, ?([\d.]+))?\)/);
        if (!m) return false;
        return m[4] === undefined || parseFloat(m[4]) >= 0.95;
      };
      const effBg = (el) => {
        let cur = el;
        while (cur) {
          const bg = getComputedStyle(cur).backgroundColor;
          if (opaque(bg)) return bg;
          cur = cur.parentElement;
        }
        return document.documentElement.classList.contains("light") ? "rgb(255, 255, 255)" : "rgb(11, 10, 18)";
      };
      const overflow = [], invisible = [], truncated = [], lowContrast = [], small = [], ragflow = [];
      for (const el of document.querySelectorAll("body *")) {
        const r = el.getBoundingClientRect();
        if (r.width <= 0 || r.height <= 0) continue;
        const cs = getComputedStyle(el);
        if (cs.visibility === "hidden" || cs.display === "none" || parseFloat(cs.opacity) === 0) continue;
        const text = el.childNodes && Array.from(el.childNodes).some((n) => n.nodeType === 3 && n.textContent.trim().length > 1);
        if (!text) continue;
        const snip = el.textContent.trim().slice(0, 40);
        if (/ragflow/i.test(el.textContent)) ragflow.push({ sel: short(el), text: snip });
        if (r.right > vw + 1.5 || r.left < -1.5) overflow.push({ sel: short(el), text: snip, left: Math.round(r.left), right: Math.round(r.right), vw });
        if (cs.color === cs.backgroundColor && cs.backgroundColor !== "rgba(0, 0, 0, 0)") invisible.push({ sel: short(el), text: snip, color: cs.color });
        const lc = lum(cs.color);
        const lb = lum(effBg(el));
        if (lc != null && lb != null && el.tagName !== "OPTION") {
          const ratio = (Math.max(lc, lb) + 0.05) / (Math.min(lc, lb) + 0.05);
          if (ratio < 1.7) lowContrast.push({ sel: short(el), text: snip, ratio: Math.round(ratio * 100) / 100, color: cs.color });
        }
        if (el.scrollWidth > el.clientWidth + 2 && (cs.textOverflow === "ellipsis" || cs.overflowX === "hidden" || cs.overflow === "hidden")) truncated.push({ sel: short(el), text: snip, sw: el.scrollWidth, cw: el.clientWidth });
        if (parseFloat(cs.fontSize) < 10) small.push({ sel: short(el), text: snip.slice(0, 30), fs: cs.fontSize });
      }
      return { vw, sw, overflow: overflow.slice(0, 12), invisible: invisible.slice(0, 12), truncated: truncated.slice(0, 12), lowContrast: lowContrast.slice(0, 12), small: small.slice(0, 8), ragflow: ragflow.slice(0, 12), counts: { overflow: overflow.length, invisible: invisible.length, truncated: truncated.length, lowContrast: lowContrast.length, small: small.length, ragflow: ragflow.length } };
    });
    entry.overflowX = Math.max(0, data.sw - data.vw);
    entry.counts = data.counts;
    entry.offenders = { overflow: data.overflow, invisible: data.invisible, truncated: data.truncated, lowContrast: data.lowContrast, small: data.small, ragflow: data.ragflow };
    for (const k of Object.keys(report.totals)) report.totals[k] += data.counts[k] || 0;
    await page.screenshot({ path: SHOTS + "/layout2-" + slug + ".png", fullPage: false });
  } catch (err) { entry.error = String(err && err.message ? err.message : err).slice(0, 120); }
  report.pages[slug] = entry;
};
try {
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 30000 });
  await page.evaluate(async (pw) => {
    const res = await fetch("/api/v1/auth/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: "admin@rayrag.local", password: pw }) });
    const d = await res.json();
    if (d.code === 0) { localStorage.setItem("rayrag_token", d.data.access_token); document.cookie = "rayrag_token=" + d.data.access_token + ";path=/"; }
  }, PASSWORD);
  const seed = async (path, body) => await page.evaluate(async ({ path, body }) => {
    try { const res = await fetch(path, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) }); const d = await res.json(); return d && d.data ? (d.data.id || (d.data.dataset && d.data.dataset.id) || null) : null; } catch (e) { return null; }
  }, { path, body });
  const stamp = Date.now();
  const ds = await seed("/api/v1/datasets", { name: "layout-probe-ds-" + stamp });
  const chat = await seed("/api/v1/chats", { name: "layout-probe-chat-" + stamp });
  const srch = await seed("/api/v1/searches", { name: "layout-probe-search-" + stamp });
  const agent = await seed("/api/v1/agents", { title: "layout-probe-agent-" + stamp, name: "layout-probe-agent-" + stamp });
  const detail = [
    ["dataset-detail", ds ? "/datasets/" + ds : null],
    ["chat-detail", chat ? "/chats/" + chat : null],
    ["search-detail", srch ? "/searches/" + srch : null],
    ["agent-detail", agent ? "/agents/" + agent : null],
  ];
  for (const [slug, path] of detail) {
    if (!path) { report.pages[slug] = { skipped: "seed-missing" }; continue; }
    await page.goto(ORIGIN + path, { waitUntil: "domcontentloaded", timeout: 40000 });
    await page.waitForTimeout(1200);
    await scan(slug);
    const tabs = page.locator("[role='tab'], .kb-tab, .tab-link");
    const n = Math.min(await tabs.count(), 3);
    for (let i = 0; i < n; i += 1) {
      try {
        await tabs.nth(i).click({ timeout: 4000 });
        await page.waitForTimeout(700);
        await scan(slug + "-tab" + (i + 1));
      } catch (e) { report.pages[slug + "-tab" + (i + 1)] = { skipped: "tab-click" }; }
    }
  }
  const staticPages = [["files", "/files"], ["user-setting", "/user-setting"]];
  for (const [slug, path] of staticPages) {
    await page.goto(ORIGIN + path, { waitUntil: "domcontentloaded", timeout: 40000 });
    await page.waitForTimeout(900);
    await scan(slug);
  }
  const dialogs = [
    ["dlg-create-user", "/admin/users", "[data-testid='admin-new-user']"],
    ["dlg-service-detail", "/admin/services", "[data-testid='admin-service-row'] button[title='Show details']"],
    ["dlg-service-extra", "/admin/services", "[data-testid='admin-service-row'] button[title='Extra information']"],
  ];
  for (const [slug, path, sel] of dialogs) {
    try {
      await page.goto(ORIGIN + path, { waitUntil: "domcontentloaded", timeout: 40000 });
      await page.waitForTimeout(1200);
      await page.locator(sel).first().click({ timeout: 8000 });
      await page.waitForTimeout(700);
      await scan(slug);
      await page.evaluate(() => document.querySelectorAll(".modal-overlay.show").forEach((n) => n.classList.remove("show")));
    } catch (err) { report.pages[slug] = { skipped: "dialog-open", error: String(err && err.message ? err.message : err).slice(0, 90) }; }
  }
} finally {
  console.log(JSON.stringify(report, null, 1));
  await page.close(); await ctx.close(); await b.close();
}
