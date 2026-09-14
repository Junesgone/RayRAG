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
const searchBodies = [];
// Upstream `/search` keeps its settings drawer in react-hook-form state: picking
// a rerank model must NOT PATCH the tenant default (`/api/v1/models/default`).
const defaultPatchCalls = [];
page.on("request", (req) => {
  const u = req.url();
  if (
    u.includes("/api/v1/retrieval") &&
    req.method() === "POST" &&
    req.postData()
  ) {
    try {
      searchBodies.push(JSON.parse(req.postData()));
    } catch {}
  }
  if (u.includes("/api/v1/models/default") && req.method() === "PATCH") {
    defaultPatchCalls.push(u);
  }
});
const results = { steps: [], errors: [] };

try {
  await ctx.addCookies([
    { name: "lang", value: "zh-CN", domain: "127.0.0.1", path: "/" },
  ]);
  await page.goto(ORIGIN + "/login", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  const login = await page.evaluate(
    async ({ password }) => {
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
    },
    { password: PASSWORD },
  );
  results.steps.push("login:" + JSON.stringify(login));
  if (!login.ok) throw new Error("login failed");

  const cleaned = await page.evaluate(async () => {
    const h = {
      "Content-Type": "application/json",
      Authorization: "Bearer " + (localStorage.getItem("rayrag_token") || ""),
    };
    const out = { kbs: [] };
    const kbRes = await fetch("/api/v1/datasets?page=1&page_size=100", {
      headers: h,
    });
    for (const kb of ((await kbRes.json()).data || {}).items || []) {
      if (
        typeof kb.name === "string" &&
        kb.name.indexOf("CDP-RPAGE-KB-") === 0
      ) {
        await fetch("/api/v1/datasets/" + kb.id, {
          method: "DELETE",
          headers: h,
        });
        out.kbs.push(kb.name);
      }
    }
    return out;
  });
  results.cleaned = cleaned;

  const kb = await page.evaluate(async () => {
    const res = await fetch("/api/v1/datasets", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: "Bearer " + (localStorage.getItem("rayrag_token") || ""),
      },
      body: JSON.stringify({
        name: "CDP-RPAGE-KB-" + Date.now(),
        description: "cdp search page rerank probe",
      }),
    });
    const data = await res.json();
    if (data.code !== 0 || !data.data || !data.data.id)
      return { ok: false, body: data };
    return { ok: true, id: data.data.id };
  });
  results.kb = kb;
  if (!kb.ok) throw new Error("probe kb create failed");

  // ---- stage A: plain search page rerank picker (real user flow: the
  // results view is revealed by the first search before the rerank switch) ----
  await page.goto(ORIGIN + "/search", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForTimeout(2000);
  await page.fill("#query", "probe rpage question").catch(() => {});
  await page.evaluate(() => {
    const el = Array.from(document.querySelectorAll("button")).find(
      (x) => x.getAttribute("onclick") === "doSearch()",
    );
    if (el) el.click();
  });
  await page.waitForTimeout(1500);
  await page.evaluate(() => {
    const c = document.getElementById("rerank");
    if (c) c.click();
  });
  await page.waitForTimeout(500);
  const toggled = await page.evaluate(() => {
    const row = document.getElementById("searchRerankRow");
    return {
      rowVisible: row ? getComputedStyle(row).display !== "none" : false,
      catalogLen:
        typeof DEFAULT_MODEL_CATALOG === "undefined"
          ? -1
          : DEFAULT_MODEL_CATALOG.length,
      rowCount: document.querySelectorAll(
        "#searchRerankSelTree .default-model-node",
      ).length,
    };
  });
  results.toggled = toggled;

  await page
    .locator("[data-testid='default-model-rerank_id']")
    .click({ timeout: 8000 });
  await page.waitForTimeout(700);
  let picked = "";
  const pick = await page.evaluate(() => {
    const rows = Array.from(
      document.querySelectorAll(
        "#searchRerankSelTree button.default-model-node",
      ),
    );
    const diag = rows
      .slice(0, 5)
      .map((el) => String(el.className).slice(0, 60));
    let leaf = null;
    for (const r of rows) {
      if (String(r.className).indexOf("default-model-leaf") >= 0) {
        leaf = r;
        break;
      }
    }
    if (!leaf && rows.length) leaf = rows[rows.length - 1];
    if (leaf) leaf.click();
    return diag;
  });
  results.pickDiag = pick;
  await page.waitForTimeout(600);
  picked = await page.evaluate(() => {
    const t = document.querySelector("[data-testid='default-model-rerank_id']");
    return (t && t.dataset.value) || "";
  });
  results.picked = picked;

  await page.evaluate(() => {
    const el = Array.from(document.querySelectorAll("button")).find(
      (x) => x.getAttribute("onclick") === "doSearch2()",
    );
    if (el) el.click();
  });
  await page.waitForTimeout(2500);
  results.searchBodyA = searchBodies[searchBodies.length - 1] || null;
  results.firstSearchBody =
    searchBodies.length > 1 ? searchBodies[searchBodies.length - 2] : null;

  // ---- stage B: search app runtime seeds the picker ----
  const app = await page.evaluate(
    async ({ kbId, rerankId }) => {
      const res = await fetch("/api/v1/searchapps", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization:
            "Bearer " + (localStorage.getItem("rayrag_token") || ""),
        },
        body: JSON.stringify({
          name: "CDP-RPAGE-" + Date.now(),
          kb_ids: [kbId],
          hybrid: true,
          rerank: true,
          rerank_id: rerankId,
          top_k: 8,
        }),
      });
      const data = await res.json();
      if (data.code !== 0 || !data.data || !data.data.id)
        return { ok: false, body: data };
      return { ok: true, id: data.data.id };
    },
    { kbId: kb.id, rerankId: picked },
  );
  results.app = app;
  if (!app.ok) throw new Error("probe app create failed");

  await page.goto(ORIGIN + "/search?sapp=" + app.id, {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForTimeout(2000);
  const seededSel = await page.evaluate(() => {
    const t = document.querySelector("[data-testid='default-model-rerank_id']");
    const row = document.getElementById("searchRerankRow");
    const s = typeof window.__SAPP__ === "undefined" ? null : window.__SAPP__;
    return {
      triggerValue: (t && t.dataset.value) || "",
      rowVisible: row ? getComputedStyle(row).display !== "none" : false,
      sappRerankId: s ? s.rerank_id : null,
    };
  });
  results.seededSel = seededSel;

  const cleanup = await page.evaluate(
    async ({ appId, kbId }) => {
      const h = {
        "Content-Type": "application/json",
        Authorization: "Bearer " + (localStorage.getItem("rayrag_token") || ""),
      };
      const a = await fetch("/api/v1/searchapps/" + appId, {
        method: "DELETE",
        headers: h,
      });
      const k = await fetch("/api/v1/datasets/" + kbId, {
        method: "DELETE",
        headers: h,
      });
      return { app: a.status, kb: k.status };
    },
    { appId: app.id, kbId: kb.id },
  );
  results.cleanup = cleanup;
  results.pageErrors = pageErrors;
  results.defaultPatchCalls = defaultPatchCalls.length;
  console.log(JSON.stringify(results, null, 1));
  await page.close();
  process.exit(0);
} catch (e) {
  results.errors.push(String(e && e.message ? e.message : e).slice(0, 300));
  console.log(JSON.stringify(results, null, 1));
  await page.close().catch(() => {});
  process.exit(1);
}
