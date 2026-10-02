import { chromium } from "playwright-core";
const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9485";
const CDP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
const SHOTS = "docs/ui-traversal/rayrag-audit";
const PAGES = [
  ["login", "/login"], ["root", "/"], ["datasets", "/datasets"], ["chats", "/chats"],
  ["searches", "/searches"], ["agents", "/agents"], ["memories", "/memories"],
  ["files", "/files"], ["user-setting", "/user-setting"],
  ["admin-services", "/admin/services"], ["admin-users", "/admin/users"],
  ["admin-sandbox", "/admin/sandbox-settings"], ["admin-whitelist", "/admin/whitelist"],
  ["admin-roles", "/admin/roles"], ["admin-monitoring", "/admin/monitoring"],
];
const b = await chromium.connectOverCDP(CDP);
const ctx = await b.newContext();
const page = await ctx.newPage();
const report = { pages: {}, totals: { overflow: 0, invisible: 0, clipped: 0, small: 0, ragflow: 0 } };
try {
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 30000 });
  await page.evaluate(async (pw) => {
    const res = await fetch("/api/v1/auth/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: "admin@rayrag.local", password: pw }) });
    const d = await res.json();
    if (d.code === 0) { localStorage.setItem("rayrag_token", d.data.access_token); document.cookie = "rayrag_token=" + d.data.access_token + ";path=/"; }
  }, PASSWORD);
  for (const [slug, path] of PAGES) {
    const entry = { url: path, overflowX: 0, offenders: {}, counts: {} };
    try {
      await page.goto(ORIGIN + path, { waitUntil: "domcontentloaded", timeout: 40000 });
      await page.waitForTimeout(path.startsWith("/admin") ? 1500 : 900);
      const data = await page.evaluate(() => {
        const vw = window.innerWidth;
        const sw = document.documentElement.scrollWidth;
        const short = (el) => {
          const cls = el.className && typeof el.className === "string" ? el.className.split(/\s+/).find(Boolean) : "";
          return el.tagName.toLowerCase() + (el.id ? "#" + el.id : "") + (cls ? "." + cls : "");
        };
        const overflow = [], invisible = [], clipped = [], small = [], ragflow = [];
        for (const el of document.querySelectorAll("body *")) {
          const r = el.getBoundingClientRect();
          if (r.width <= 0 || r.height <= 0) continue;
          const cs = getComputedStyle(el);
          if (cs.visibility === "hidden" || cs.display === "none" || parseFloat(cs.opacity) === 0) continue;
          const text = el.childNodes && Array.from(el.childNodes).some((n) => n.nodeType === 3 && n.textContent.trim().length > 1);
          if (!text) continue;
          const snippet = el.textContent.trim().slice(0, 40);
          if (/ragflow/i.test(el.textContent)) ragflow.push({ sel: short(el), text: snippet });
          if (r.right > vw + 1.5 || r.left < -1.5) overflow.push({ sel: short(el), text: snippet, left: Math.round(r.left), right: Math.round(r.right), vw });
          if (cs.color === cs.backgroundColor && cs.backgroundColor !== "rgba(0, 0, 0, 0)") invisible.push({ sel: short(el), text: snippet, color: cs.color });
          if (el.scrollWidth > el.clientWidth + 2 && (cs.overflowX === "hidden" || cs.overflow === "hidden") && cs.textOverflow !== "ellipsis") clipped.push({ sel: short(el), text: snippet, sw: el.scrollWidth, cw: el.clientWidth });
          if (parseFloat(cs.fontSize) < 10) small.push({ sel: short(el), text: snippet.slice(0, 30), fs: cs.fontSize });
        }
        return { vw, sw, overflow: overflow.slice(0, 12), invisible: invisible.slice(0, 12), clipped: clipped.slice(0, 12), small: small.slice(0, 8), ragflow: ragflow.slice(0, 12), counts: { overflow: overflow.length, invisible: invisible.length, clipped: clipped.length, small: small.length, ragflow: ragflow.length } };
      });
      entry.overflowX = Math.max(0, data.sw - data.vw);
      entry.offenders = { overflow: data.overflow, invisible: data.invisible, clipped: data.clipped, small: data.small, ragflow: data.ragflow };
      entry.counts = data.counts;
      for (const k of Object.keys(report.totals)) report.totals[k] += data.counts[k] || 0;
      await page.screenshot({ path: SHOTS + "/layout-" + slug + ".png", fullPage: false });
    } catch (err) {
      entry.error = String(err && err.message ? err.message : err).slice(0, 120);
    }
    report.pages[slug] = entry;
  }
} finally {
  console.log(JSON.stringify(report, null, 1));
  await page.close(); await ctx.close(); await b.close();
}
