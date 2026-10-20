// CDP probe for the `/search` view residuals closed in v0.3.6c
// (`web/src/pages/next-search/search-view.tsx` + `mindmap-sheet.tsx`):
//   * the greeting input row (greeting placeholder, clear X, separator and the
//     round submit button that turns into a stop control while streaming),
//   * the AI-summary block (heading, skeleton while streaming, streamed answer,
//     citation chips, divider),
//   * the result bar with the mind-map button and the mind-map sheet
//     (progress bar, indented tree, close button).
//
// It runs a real query against a probe search app, waits for the streamed
// answer, then reads the mind map, and finally deletes the probe app.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/search-answer-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
const EMAIL = process.env.RAYRAG_ADMIN_EMAIL || "admin@rayrag.local";
if (!PASSWORD) {
  console.error(
    "RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)",
  );
  process.exit(2);
}
const QUESTION = "What does this knowledge base cover?";

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
await page.setViewportSize({ width: 1440, height: 1000 });
const results = { errors: [], pageErrors };
const shots = "docs/ui-traversal/rayrag-audit";
mkdirSync(shots, { recursive: true });

try {
  await ctx.addCookies([
    { name: "lang", value: "en", domain: "127.0.0.1", path: "/" },
  ]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const login = await page.evaluate(async ({ email, password }) => {
    const res = await fetch("/api/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email, password }),
    });
    const data = await res.json();
    if (data.code !== 0) return { ok: false };
    localStorage.setItem("rayrag_token", data.data.access_token);
    return { ok: true };
  }, { email: EMAIL, password: PASSWORD });
  if (!login.ok) throw new Error("login failed");

  // Probe search app over the existing knowledge bases, with the AI summary,
  // mind map and related search switched on.
  const app = await page.evaluate(async (name) => {
    const datasets = await fetch("/api/v1/datasets").then((response) =>
      response.json(),
    );
    const withDocs = (datasets.data || []).filter(
      (item) => (item.doc_count || 0) > 0,
    );
    const pool = withDocs.length ? withDocs : datasets.data || [];
    // A single knowledge base keeps the embedding model consistent: mixing KBs
    // with different embedding models is rejected by the retrieval path (and by
    // the completion), which would leave the probe with an empty result list.
    const kbIds = pool.slice(0, 1).map((item) => item.id);
    const created = await fetch("/api/v1/searchapps", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        name,
        kb_ids: kbIds,
        summary: true,
        query_mindmap: true,
        related_search: true,
      }),
    }).then((response) => response.json());
    return {
      ok: created.code === 0,
      id: created.data && created.data.id,
      kbIds,
      withDocs: withDocs.map((item) => [item.name, item.doc_count]),
      message: created.message,
    };
  }, `cdp-search-probe-${Date.now()}`);
  results.app = app;
  if (!app.ok) throw new Error("probe search app failed: " + app.message);

  const searchUrl = `${ORIGIN}/search?sapp=${app.id}`;
  await page.goto(searchUrl, { waitUntil: "domcontentloaded", timeout: 30000 });
  await page.waitForSelector("#query", { timeout: 10000 });
  // Upstream starts on the search home; the greeting row lives in the results
  // view, so the labels are checked after the switch below.
  await page.fill("#query", QUESTION);
  await page.click("#searchHome button");
  // Hybrid retrieval keeps the vector weight: without it the page sends
  // `vector_similarity_weight: 0` (keyword-only), which finds nothing for a
  // question phrased without the document's keywords. The switch only exists in
  // the results view, so it is flipped after the transition and the query is
  // re-run through the greeting row.
  await page.setChecked("#hybrid", true);
  await page.waitForSelector("[data-testid='search-ask-submit']", {
    state: "visible",
    timeout: 15000,
  });
  results.inputRow = await page.evaluate(() => ({
    placeholder: document.getElementById("q2").placeholder,
    hasClear: !!document.getElementById("searchAskClear"),
    hasSeparator: !!document.querySelector(".search-ask-sep"),
    aiSummaryHidden: document.getElementById("aiSummaryBlock").hidden,
  }));
  // Idle state first: the round submit button carries the lucide Search glyph
  // until a query starts.
  results.idleSubmit = await page.evaluate(() => ({
    hasSearchIcon: !!document.querySelector(
      "[data-testid='search-ask-submit'] svg circle",
    ),
    streaming: document
      .getElementById("searchAskSubmit")
      .classList.contains("streaming"),
  }));
  await page.screenshot({ path: `${shots}/search-view-home-20260919.png` });

  // Real query: fill the input and submit, then watch the summary stream.
  await page.fill("#q2", QUESTION);
  await page.click("[data-testid='search-ask-submit']");
  // The submit button flips to the stop control while the answer streams; the
  // second click below would stop the stream instead of re-running it, so the
  // query is submitted exactly once with hybrid retrieval already enabled.
  await page
    .waitForFunction(
      () =>
        document
          .getElementById("searchAskSubmit")
          .classList.contains("streaming"),
      null,
      { timeout: 15000 },
    )
    .catch(() => {});
  results.streaming = await page.evaluate(() => ({
    streaming: document
      .getElementById("searchAskSubmit")
      .classList.contains("streaming"),
    busy: document.getElementById("searchAskSubmit").getAttribute("aria-busy"),
    skeleton: !!document.querySelector(".search-ai-skeleton"),
    summaryHidden: document.getElementById("aiSummaryBlock").hidden,
  }));

  // Wait for the streamed answer (or a reported failure) to settle.
  await page
    .waitForFunction(
      () => {
        const button = document.getElementById("searchAskSubmit");
        return button && !button.classList.contains("streaming");
      },
      null,
      { timeout: 60000 },
    )
    .catch(() => {});
  await page
    .waitForFunction(
      () => {
        const body = document.getElementById("aiSummaryBody");
        if (!body) return false;
        return (
          !!body.querySelector(".search-ai-answer") ||
          !!body.querySelector(".search-ai-status")
        );
      },
      null,
      { timeout: 60000 },
    )
    .catch(() => {});
  await page.waitForTimeout(600);
  results.answer = await page.evaluate(() => {
    const body = document.getElementById("aiSummaryBody");
    const answer = body ? body.querySelector(".search-ai-answer") : null;
    return {
      title: document.querySelector(".search-ai-title")
        ? document.querySelector(".search-ai-title").textContent
        : null,
      hasAnswer: !!answer,
      answerLength: answer ? answer.textContent.length : 0,
      answerHead: answer ? answer.textContent.slice(0, 80) : null,
      refs: document.querySelectorAll(".search-ai-ref").length,
      status: body && body.querySelector(".search-ai-status")
        ? body.querySelector(".search-ai-status").textContent
        : null,
      divider: !!document.querySelector(".search-ai-divider"),
      total: document.getElementById("searchTotal").textContent,
      resultItems: document.querySelectorAll("#results .ri").length,
      relatedChips: document.querySelectorAll(".search-related-chip").length,
      streaming: document
        .getElementById("searchAskSubmit")
        .classList.contains("streaming"),
      hasSearchIcon: !!document.querySelector(
        "[data-testid='search-ask-submit'] svg",
      ),
    };
  });
  await page.screenshot({ path: `${shots}/search-view-answer-20260919.png` });

  // Mind map: real click on the button, then read the sheet.
  const mindVisible = await page.evaluate(
    () => !document.getElementById("searchMindBtn").hidden,
  );
  results.mindButtonVisible = mindVisible;
  if (mindVisible) {
    await page.click("[data-testid='search-mindmap']");
    await page.waitForTimeout(400);
    results.mindSheetOpening = await page.evaluate(() => ({
      hidden: document.getElementById("mindSheet").hidden,
      progressVisible: !document.getElementById("mindProgress").hidden,
    }));
    await page
      .waitForFunction(
        () => document.getElementById("mindProgress").hidden,
        null,
        { timeout: 60000 },
      )
      .catch(() => {});
    await page.waitForTimeout(500);
    results.mindSheet = await page.evaluate(() => ({
      title: document.getElementById("mindSheetTitle").textContent,
      hidden: document.getElementById("mindSheet").hidden,
      progressHidden: document.getElementById("mindProgress").hidden,
      nodes: document.querySelectorAll("#mindTree li").length,
      rootText: (document.querySelector("#mindTree li span") || {})
        .textContent,
      bodyText: document.getElementById("mindTree").textContent.slice(0, 60),
      hasClose: !!document.querySelector("[data-testid='mindmap-close']"),
    }));
    await page.screenshot({ path: `${shots}/search-view-mindmap-20260919.png` });
    await page.click("[data-testid='mindmap-close']");
    await page.waitForTimeout(300);
    results.mindSheetClosed = await page.evaluate(
      () => document.getElementById("mindSheet").hidden,
    );
  }

  // Clear button resets the query and hides the summary block.
  await page.click("#searchAskClear");
  await page.waitForTimeout(300);
  results.cleared = await page.evaluate(() => ({
    value: document.getElementById("q2").value,
    summaryHidden: document.getElementById("aiSummaryBlock").hidden,
  }));
} catch (error) {
  results.errors.push(String(error && error.stack ? error.stack : error));
  await page.screenshot({ path: `${shots}/search-answer-error-20260919.png` });
} finally {
  if (results.app && results.app.ok) {
    results.cleanup = await page
      .evaluate(async (id) => {
        const response = await fetch("/api/v1/searchapps/" + id, {
          method: "DELETE",
        });
        const data = await response.json().catch(() => ({}));
        return { ok: response.ok && data.code === 0, message: data.message };
      }, results.app.id)
      .catch((error) => ({ ok: false, message: String(error) }));
  }
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
}
if (results.errors.length || results.pageErrors.length) process.exit(1);
