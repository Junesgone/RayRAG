import { chromium } from "playwright-core";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required");
  process.exit(2);
}
const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const b = await chromium.connectOverCDP(CDP_EP);
const ctx = b.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1440, height: 900 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 240)));
const bodies = [];
page.on("request", (req) => {
  const u = req.url();
  if (u.includes("/api/v1/retrieval") && req.method() === "POST" && req.postData()) {
    try {
      bodies.push(JSON.parse(req.postData()));
    } catch {}
  }
});
const results = { steps: [], errors: [] };
const authHeaders = "() => ({ 'Content-Type': 'application/json', Authorization: 'Bearer ' + (localStorage.getItem('rayrag_token') || '') })";

try {
  await ctx.addCookies([
    { name: "lang", value: "zh-CN", domain: "127.0.0.1", path: "/" },
  ]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const login = await page.evaluate(async ({ password }) => {
    const res = await fetch("/api/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "admin@rayrag.local", password }),
    });
    const data = await res.json();
    if (data.code !== 0 || !data.data || !data.data.access_token) {
      return { ok: false, body: data };
    }
    localStorage.setItem("rayrag_token", data.data.access_token);
    return { ok: true };
  }, { password: PASSWORD });
  results.steps.push("login:" + JSON.stringify(login));
  if (!login.ok) throw new Error("login failed");

  const cleaned = await page.evaluate(async () => {
    const h = { "Content-Type": "application/json", Authorization: "Bearer " + (localStorage.getItem("rayrag_token") || "") };
    const out = { apps: [], kbs: [] };
    const appsRes = await fetch("/api/v1/searchapps", { headers: h });
    const appsData = await appsRes.json();
    for (const app of (appsData.data || [])) {
      if (typeof app.name === "string" && app.name.indexOf("CDP-SAPP-") === 0) {
        await fetch("/api/v1/searchapps/" + app.id, { method: "DELETE", headers: h });
        out.apps.push(app.name);
      }
    }
    const kbRes = await fetch("/api/v1/datasets?page=1&page_size=100", { headers: h });
    const kbData = await kbRes.json();
    for (const kb of ((kbData.data || {}).items || kbData.data || [])) {
      if (typeof kb.name === "string" && kb.name.indexOf("CDP-SAPP-KB-") === 0) {
        await fetch("/api/v1/datasets/" + kb.id, { method: "DELETE", headers: h });
        out.kbs.push(kb.name);
      }
    }
    return out;
  });
  results.cleaned = cleaned;

  const kb = await page.evaluate(async () => {
    const res = await fetch("/api/v1/datasets", {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: "Bearer " + (localStorage.getItem("rayrag_token") || "") },
      body: JSON.stringify({ name: "CDP-SAPP-KB-" + Date.now(), description: "cdp sapp runtime probe" }),
    });
    const data = await res.json();
    if (data.code !== 0 || !data.data || !data.data.id) return { ok: false, body: data };
    return { ok: true, id: data.data.id };
  });
  results.kb = kb;
  if (!kb.ok) throw new Error("probe kb create failed");

  const models = await page.evaluate(async () => {
    const res = await fetch("/api/v1/models", {
      headers: { Authorization: "Bearer " + (localStorage.getItem("rayrag_token") || "") },
    });
    const data = await res.json();
    const list = (data.data || [])
      .filter((m) =>
        Array.isArray(m.model_type)
          ? m.model_type.indexOf("rerank") >= 0
          : m.model_type === "rerank",
      )
      .filter((m) => m.selector);
    return { count: list.length, selector: list.length ? list[0].selector : "" };
  });
  results.models = models;
  if (!models.selector) throw new Error("no rerank model selector in catalog");

  const app = await page.evaluate(async ({ kbId, rerankId }) => {
    const res = await fetch("/api/v1/searchapps", {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: "Bearer " + (localStorage.getItem("rayrag_token") || "") },
      body: JSON.stringify({
        name: "CDP-SAPP-" + Date.now(),
        kb_ids: [kbId],
        hybrid: true,
        rerank: true,
        rerank_id: rerankId,
        top_k: 8,
      }),
    });
    const data = await res.json();
    if (data.code !== 0 || !data.data || !data.data.id) return { ok: false, body: data };
    return { ok: true, id: data.data.id };
  }, { kbId: kb.id, rerankId: models.selector });
  results.app = app;
  if (!app.ok) throw new Error("probe app create failed");

  await page.goto(ORIGIN + "/search?sapp=" + app.id, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(1800);

  const seeded = await page.evaluate(() => {
    const s = typeof window.__SAPP__ === "undefined" ? null : window.__SAPP__;
    const rerankCk = document.getElementById("rerank");
    const hybridCk = document.getElementById("hybrid");
    const topK = document.getElementById("topK");
    const chip = document.querySelector(".kbCk[value='" + (s ? s.kb_ids.join("") : "") + "']");
    const allChip = Array.from(document.querySelectorAll(".kbCk")).find((c) => c.value === "__all__");
    return {
      sappPresent: !!s,
      sappRerankId: s ? s.rerank_id : null,
      sappTopK: s ? s.top_k : null,
      rerankChecked: !!(rerankCk || {}).checked,
      hybridChecked: !!(hybridCk || {}).checked,
      topKValue: topK ? topK.value : null,
      kbChipChecked: chip ? !!chip.checked : null,
      allChipChecked: allChip ? !!allChip.checked : null,
    };
  });
  results.seeded = seeded;

  await page.fill("#query", "probe sapp question").catch(() => {});
  await page.evaluate(() => {
    const el = Array.from(document.querySelectorAll("button")).find((x) => x.getAttribute("onclick") === "doSearch()");
    if (el) el.click();
  });
  await page.waitForTimeout(2500);

  results.putBody = bodies[bodies.length - 1] || null;
  const after = await page.evaluate(() => {
    const res = document.getElementById("results");
    return { resultsHtmlLen: res ? res.innerHTML.length : 0 };
  });
  results.after = after;

  const cleanup = await page.evaluate(async ({ appId, kbId }) => {
    const h = { "Content-Type": "application/json", Authorization: "Bearer " + (localStorage.getItem("rayrag_token") || "") };
    const a = await fetch("/api/v1/searchapps/" + appId, { method: "DELETE", headers: h });
    const k = await fetch("/api/v1/datasets/" + kbId, { method: "DELETE", headers: h });
    return { app: a.status, kb: k.status };
  }, { appId: app.id, kbId: kb.id });
  results.cleanup = cleanup;
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 1));
  await page.close();
  process.exit(0);
} catch (e) {
  results.errors.push(String(e && e.message ? e.message : e).slice(0, 300));
  console.log(JSON.stringify(results, null, 1));
  await page.close().catch(() => {});
  process.exit(1);
}
