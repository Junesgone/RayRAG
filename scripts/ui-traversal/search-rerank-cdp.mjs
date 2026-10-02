// Search page rerank field (upstream search-setting.tsx): toggle the switch,
// confirm the model tree is required, pick a model, run a search and inspect
// the retrieval payload; then disable and confirm top_k resets to 1024.
//
// Usage: UI_DUMP_PASSWORD=... node scripts/ui-traversal/search-rerank-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD || process.env.UI_DUMP_PASSWORD;
const SHOT = process.env.SHOT_DIR || "/tmp";

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1100 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 180)));
const bodies = [];
page.on("request", (req) => {
  if (req.method() === "POST" && req.url().includes("/api/v1/")) {
    try {
      bodies.push({ url: req.url().replace(ORIGIN, ""), body: req.postDataJSON() });
    } catch {
      bodies.push({ url: req.url().replace(ORIGIN, ""), body: req.postData() });
    }
  }
});

const out = { steps: [] };
const snap = async (label) => {
  const state = await page.evaluate(() => ({
    rerankChecked: document.getElementById("rerank").checked,
    rowVisible: document.getElementById("searchRerankRow").style.display !== "none",
    errorText: (document.getElementById("searchRerankError")?.textContent || "").trim(),
    selected: (document.getElementById("searchRerankSel")?.dataset.value || ""),
    topK: document.getElementById("topK").value,
    label: document.querySelector("label:has(#rerank)")?.textContent.trim(),
  }));
  out.steps.push({ label, ...state });
  await page.screenshot({ path: `${SHOT}/rayrag-search-rerank-${out.steps.length}-${label}.png` });
  return state;
};

try {
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.evaluate(async (pw) => {
    const r = await fetch("/api/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "admin@rayrag.local", password: pw }),
    });
    const d = await r.json();
    if (d.code === 0) localStorage.setItem("rayrag_token", d.data.access_token);
  }, PASSWORD);
  await page.goto(`${ORIGIN}/search`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(2200);
  await snap("initial");

  // The quick bar lives in the results view, which appears after the first
  // search from the home view (upstream `search-home` → `search-view`).
  await page.evaluate(() => {
    document.getElementById("query").value = "welcome";
    doSearch();
  });
  await page.waitForTimeout(1800);
  await snap("results-view");

  // Enable rerank without a model and try to search → upstream validation copy.
  await page.locator("#rerank").check({ timeout: 8000 });
  await page.waitForTimeout(250);
  await snap("enabled");
  await page.evaluate(() => { document.getElementById("q2").value = "welcome"; doSearch2(); });
  await page.waitForTimeout(700);
  await snap("missing-model");

  // Pick a rerank model from the shared tree and search again.
  await page.locator("[data-testid='default-model-rerank_id']").first().click({ timeout: 8000 });
  await page.waitForTimeout(600);
  const leaf = page.locator("#searchRerankSelPopover .default-model-node").last();
  const picked = (await leaf.count()) ? (await leaf.textContent()) || "" : "";
  if (await leaf.count()) await leaf.click({ timeout: 8000 });
  await page.waitForTimeout(400);
  await snap("model-picked");
  await page.evaluate(() => doSearch2());
  await page.waitForTimeout(1500);
  await snap("searched");

  // Disabling resets top_k to the upstream default.
  await page.locator("#rerank").uncheck({ timeout: 8000 });
  await page.waitForTimeout(400);
  await snap("disabled");
  out.picked = picked.trim().slice(0, 40);
  out.payloads = bodies.filter((b) => b.url.includes("/retrieval") || b.url.includes("/search"));
} catch (error) {
  out.error = String(error && error.message ? error.message : error).slice(0, 300);
}
out.pageErrors = pageErrors;
console.log(JSON.stringify(out, null, 1));
await page.close().catch(() => {});
process.exit(0);
