// Dataset Configuration tag tabs (upstream tag-tabs.tsx): seed tags through the
// metadata API, then verify the word cloud, the table, rename (PUT) and delete
// (DELETE) round-trips against GET /api/v1/datasets/{id}/tags.
//
// Usage: UI_DUMP_PASSWORD=... node scripts/ui-traversal/dataset-tags-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD || process.env.UI_DUMP_PASSWORD;
const SHOT = process.env.SHOT_DIR || "/tmp";

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 180)));
page.on("dialog", async (d) => {
  if (d.type() === "prompt") await d.accept("H2O");
  else await d.accept();
});
const out = { steps: [] };

const snap = async (label) => {
  const state = await page.evaluate(() => ({
    cloudHidden: document.getElementById("dsTagCloud").hidden,
    tableHidden: document.getElementById("dsTagTableWrap").hidden,
    chips: Array.from(document.querySelectorAll(".ds-tag-chip")).map((c) => `${c.dataset.tag}:${c.title}`),
    rows: Array.from(document.querySelectorAll("#dsTagRows tr")).map((r) => r.dataset.tag),
    status: (document.getElementById("dsTagStatus")?.textContent || "").trim(),
    cloudTabChecked: document.getElementById("dsTagTabCloud").getAttribute("aria-checked"),
  }));
  out.steps.push({ label, ...state });
  await page.screenshot({ path: `${SHOT}/rayrag-tags-${out.steps.length}-${label}.png` });
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

  const kb = await page.evaluate(async () => {
    const list = (await (await fetch("/api/v1/datasets")).json()).data || [];
    const withDocs = list.find((d) => (d.doc_count || 0) > 0);
    return (withDocs || list[0]).id;
  });
  // Seed tags on two documents through the documented metadata API.
  out.seeded = await page.evaluate(async (id) => {
    const docs = (await (await fetch(`/api/v1/datasets/${id}/documents`)).json()).data || [];
    const targets = docs.slice(0, 2).map((d) => d.id);
    const results = [];
    for (const [index, docId] of targets.entries()) {
      const res = await fetch(`/api/v1/datasets/${id}/documents/${docId}/metadata`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ meta_fields: { tags: index === 0 ? ["Water", "Fish"] : ["Water", "Feed"] } }),
      });
      results.push(res.status);
    }
    return { targets: targets.length, results };
  }, kb);
  out.api = await page.evaluate(async (id) => (await (await fetch(`/api/v1/datasets/${id}/tags`)).json()).data, kb);

  await page.goto(`${ORIGIN}/dataset/configuration/${kb}`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(2500);
  await snap("cloud");
  await page.locator("#dsTagTabTable").click({ timeout: 8000 });
  await page.waitForTimeout(400);
  await snap("table");

  // Rename via the table action (the prompt is auto-accepted with H2O).
  const firstRename = page.locator("#dsTagRows .ds-tag-rename").first();
  if (await firstRename.count()) {
    await firstRename.click({ timeout: 8000 });
    await page.waitForTimeout(1200);
    await snap("renamed");
  }
  out.apiAfterRename = await page.evaluate(async (id) => (await (await fetch(`/api/v1/datasets/${id}/tags`)).json()).data, kb);

  // Delete the first row's tag.
  const firstDelete = page.locator("#dsTagRows .ds-tag-delete").first();
  if (await firstDelete.count()) {
    await firstDelete.click({ timeout: 8000 });
    await page.waitForTimeout(1200);
    await snap("deleted");
  }
  out.apiAfterDelete = await page.evaluate(async (id) => (await (await fetch(`/api/v1/datasets/${id}/tags`)).json()).data, kb);
} catch (error) {
  out.error = String(error && error.message ? error.message : error).slice(0, 300);
}
out.pageErrors = pageErrors;
console.log(JSON.stringify(out, null, 1));
await page.close().catch(() => {});
process.exit(0);
