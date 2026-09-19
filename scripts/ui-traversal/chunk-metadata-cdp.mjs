// CDP probe for the retrieval metadata chips
// (`pages/next-search/search-view.tsx`): every retrieval row renders the
// `document_metadata` chips under its content, the chip values follow
// `formatMetadataValue` (arrays joined, objects JSON, scalars stringified) and
// the request that produces them asks for `include_metadata`. The probe stubs the
// retrieval response so a chunk with metadata can be inspected without needing a
// document that has metadata enabled in this deployment; nothing is written.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/chunk-metadata-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}

const CHUNK = {
  chunk_id: "probe-chunk-1",
  content: "Probe chunk about metadata chips.",
  doc_name: "probe-metadata.pdf",
  similarity: 0.87,
  document_metadata: {
    author: "张三",
    tags: ["alpha", "beta"],
    stats: { pages: 3, lang: "zh" },
    reviewed: true,
  },
};

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
const bodies = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
await page.setViewportSize({ width: 1500, height: 1000 });
const results = { errors: [], pageErrors };
mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });
const check = (ok, label, detail) => {
  results[label] = ok ? "ok" : "FAIL";
  if (!ok) results.errors.push(label + (detail ? ` (${detail})` : ""));
};

try {
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const login = await page.evaluate(async (password) => {
    const res = await fetch("/api/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "admin@rayrag.local", password }),
    });
    const data = await res.json();
    if (data.code !== 0) return { ok: false, body: data };
    localStorage.setItem("rayrag_token", data.data.access_token);
    document.cookie = "rayrag_token=" + data.data.access_token + ";path=/";
    return { ok: true };
  }, PASSWORD);
  if (!login.ok) throw new Error("login failed: " + JSON.stringify(login.body));

  // Answer the retrieval call with one chunk that carries metadata.
  await page.route("**/api/v1/retrieval", async (route) => {
    bodies.push(route.request().postData() || "");
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        code: 0,
        message: "ok",
        data: { total: 1, chunks: [CHUNK], doc_aggs: [], graph_context: [] },
      }),
    });
  });

  await page.goto(`${ORIGIN}/search`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("#query", { timeout: 20000 });
  await page.fill("#query", "probe metadata chips");
  await page.locator("#query").press("Enter");
  await page.waitForSelector("[data-testid='chunk-metadata']", { timeout: 30000 });
  check(true, "metadata_chips_render");

  const chips = await page.locator("[data-testid='chunk-metadata'] .chunk-metadata-chip").allInnerTexts();
  check(chips.length === 4, "one_chip_per_metadata_key", String(chips.length));
  const text = chips.map((entry) => entry.replace(/\s+/g, " ").trim());
  check(text.includes("author: 张三"), "string_value_rendered", text.join(" | "));
  check(text.includes("tags: alpha, beta"), "array_value_joined", text.join(" | "));
  check(
    text.some((entry) => entry === 'stats: {"pages":3,"lang":"zh"}'),
    "object_value_json",
    text.join(" | "),
  );
  check(text.includes("reviewed: true"), "boolean_value_stringified", text.join(" | "));
  const keys = await page.locator("[data-testid='chunk-metadata'] .chunk-metadata-key").first().innerText();
  check(keys.trim() === "author:", "key_carries_the_colon", keys);
  const gap = await page
    .locator("[data-testid='chunk-metadata']")
    .evaluate((node) => getComputedStyle(node).gap);
  check(gap === "8px", "chip_row_uses_the_upstream_gap", gap);

  const body = bodies.length ? JSON.parse(bodies[bodies.length - 1]) : {};
  check(body.include_metadata === true, "request_asks_for_metadata", JSON.stringify(body.include_metadata));

  // A chunk without metadata renders no empty chip row.
  await page.unroute("**/api/v1/retrieval");
  await page.route("**/api/v1/retrieval", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        code: 0,
        message: "ok",
        data: {
          total: 1,
          chunks: [{ ...CHUNK, chunk_id: "probe-chunk-2", document_metadata: {} }],
          doc_aggs: [],
          graph_context: [],
        },
      }),
    });
  });
  await page.locator("#q2").press("Enter");
  await page.waitForFunction(
    () => document.querySelectorAll("[data-testid='chunk-metadata']").length === 0,
    null,
    { timeout: 20000 },
  );
  check(true, "empty_metadata_renders_nothing");
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/chunk-metadata.png" });
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
} finally {
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
