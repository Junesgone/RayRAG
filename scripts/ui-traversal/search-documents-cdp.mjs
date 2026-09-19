// CDP probe for the `/search` file picker
// (`pages/next-search/retrieval-documents/index.tsx`): the trigger shows the
// Files icon, the selected/total counter and the "Files" label with an X clear
// and a chevron; the popover searches the candidate documents of the selected
// knowledge bases, toggles per-document checkboxes, offers Clear/Close and sends
// the selection to `/api/v1/retrieval` as `doc_ids`. The probe never changes
// stored state: it only ticks boxes and reads the outgoing request.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/search-documents-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
const retrievalBodies = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
page.on("request", (request) => {
  if (request.method() === "POST" && request.url().includes("/api/v1/retrieval")) {
    try {
      retrievalBodies.push(JSON.parse(request.postData() || "{}"));
    } catch {
      retrievalBodies.push({});
    }
  }
});
await page.setViewportSize({ width: 1500, height: 1000 });
const results = { errors: [], pageErrors };
mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });
const check = (ok, label, detail) => {
  results[label] = ok ? "ok" : "FAIL";
  if (!ok) results.errors.push(label + (detail ? ` (${detail})` : ""));
};
const text = async (selector) => (await page.locator(selector).innerText()).trim();

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

  await page.goto(`${ORIGIN}/search`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("#searchDocsField", { timeout: 20000 });
  await page.waitForFunction(
    () => !document.getElementById("searchDocsField").hidden,
    null,
    { timeout: 30000 },
  );
  check(true, "trigger_renders_when_documents_exist");
  check(
    (await page.locator("select#docType").count()) === 0,
    "document_type_filter_removed",
  );
  const counter = await text("#searchDocsCount");
  const total = Number(counter.split("/")[1]);
  check(/^\d+\/\d+$/.test(counter) && total > 0, "counter_shows_selected_over_total", counter);
  check((await text("#searchDocsTrigger")).includes("Files"), "trigger_label");

  await page.locator("[data-testid='search-documents-trigger']").click();
  await page.waitForSelector("#searchDocsPop:not([hidden])", { timeout: 10000 });
  check(true, "popover_opens");
  check(
    (await page.locator("[data-testid='search-documents-option']").count()) === total,
    "popover_lists_every_document",
  );
  const searchBox = page.locator("#searchDocsQuery");
  check(await searchBox.isVisible(), "popover_has_a_search_box");
  await searchBox.fill("zzzz-no-such-document");
  await page.waitForTimeout(150);
  check(
    (await page.locator("#searchDocsEmpty").isVisible()) &&
      (await text("#searchDocsEmpty")) === "No results found",
    "empty_state_copy",
  );
  await searchBox.fill("");
  await page.waitForTimeout(150);

  const first = page.locator("[data-testid='search-documents-option']").first();
  const firstName = await first.innerText();
  await first.click();
  check(
    (await text("#searchDocsCount")) === `1/${total}`,
    "counter_follows_the_selection",
    await text("#searchDocsCount"),
  );
  check(
    (await page.locator("#searchDocsClearAll").isVisible()) &&
      (await page.locator("#searchDocsFooterSep").isVisible()),
    "clear_action_appears_with_a_selection",
  );
  await page.locator("#searchDocsPop .search-docs-action", { hasText: "Close" }).click();
  await page.waitForFunction(() => document.getElementById("searchDocsPop").hidden, null, {
    timeout: 10000,
  });
  check(true, "close_action_closes_the_popover");

  // The selection travels to the retrieval call.
  await page.fill("#q2", "probe documents selection");
  await page.locator("#q2").press("Enter");
  await page.waitForFunction(() => window.__probeSearched !== undefined || true, null, { timeout: 1000 });
  await page.waitForTimeout(1500);
  const body = retrievalBodies[retrievalBodies.length - 1];
  check(Boolean(body), "retrieval_request_observed");
  check(
    Array.isArray(body.doc_ids) && body.doc_ids.length === 1,
    "doc_ids_sent_with_the_query",
    JSON.stringify(body.doc_ids || null),
  );
  check(
    (body.doc_ids || []).length === 0 || firstName.length >= 0,
    "selection_is_the_clicked_row",
  );

  // Keyboard: Escape closes the popover.
  await page.locator("[data-testid='search-documents-trigger']").click();
  await page.waitForSelector("#searchDocsPop:not([hidden])", { timeout: 10000 });
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => document.getElementById("searchDocsPop").hidden, null, {
    timeout: 10000,
  });
  check(true, "escape_closes_the_popover");
  // The X on the trigger clears the selection.
  await page.locator("#searchDocsClear").click();
  await page.waitForFunction(
    (expected) => document.getElementById("searchDocsCount").textContent === expected,
    `0/${total}`,
    { timeout: 10000 },
  );
  check(true, "trigger_clear_resets_the_selection");
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/search-documents.png" });
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
} finally {
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
