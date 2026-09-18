// Search page related questions (upstream next-search `hooks.ts`
// useFetchRelatedQuestions + `search-view.tsx` chips + `search-setting.tsx`
// switch): verifies the switch seeds from the search app config, the
// `chat/recommendation` call carries the question and search app id, up to five
// chips render below the chunks, a chip click re-runs the search with its text,
// and switching the toggle off removes the block.
//
// Usage: UI_DUMP_PASSWORD=... node scripts/ui-traversal/search-related-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.UI_DUMP_PASSWORD;
const SHOT = process.env.SHOT_DIR || "/tmp";

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 180)));
page.on("dialog", async (d) => await d.accept());
const out = { steps: [], pageErrors };

// The API already strips the `N. ` list prefixes (upstream `re.sub`).
const QUESTIONS = [
  "How does retrieval augmented generation work?",
  "What are the benefits of RAG?",
  "How is RAG different from fine-tuning?",
  "What is a vector database?",
  "How does chunking affect retrieval?",
  "What is hybrid search?", // 6th item must be dropped (upstream slices to 5)
];

const snap = () =>
  page.evaluate(() => ({
    switchExists: !!document.getElementById("relatedSearch"),
    switchChecked: document.getElementById("relatedSearch")?.checked ?? null,
    heading: (document.querySelector(".search-related-title")?.textContent || "").trim(),
    chips: Array.from(document.querySelectorAll(".search-related-chip")).map((c) => c.textContent),
    chipsHidden: document.getElementById("relatedQuestions")?.hidden ?? true,
    resultCount: document.querySelectorAll("#results .ri").length,
  }));

const shot = async (label) =>
  page.screenshot({ path: `${SHOT}/rayrag-search-related-${out.steps.length + 1}-${label}.png` });

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

  // A search app with related_search enabled drives the runtime through ?sapp=.
  const appId = await page.evaluate(async () => {
    const responses = await Promise.all([fetch("/api/v1/datasets"), fetch("/api/v1/searchapps")]);
    const datasets = (await responses[0].json()).data || [];
    const apps = (await responses[1].json()).data || [];
    const existing = apps.find((a) => a.name === "related-search-probe");
    const payload = {
      name: "related-search-probe",
      kb_ids: datasets.length ? [datasets[0].id] : [],
      related_search: true,
      top_k: 10,
    };
    if (existing) {
      await fetch("/api/v1/searchapps/" + existing.id, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      });
      return existing.id;
    }
    const created = await (
      await fetch("/api/v1/searchapps", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      })
    ).json();
    return created.data.id;
  });

  const calls = [];
  await page.route("**/api/v1/chat/recommendation", async (route) => {
    calls.push(JSON.parse(route.request().postData() || "{}"));
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ code: 0, data: QUESTIONS }),
    });
  });
  await page.route("**/api/v1/retrieval", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        code: 0,
        data: {
          chunks: [
            {
              content: "RayRAG retrieves chunks and reranks them.",
              doc_name: "probe.md",
              similarity: 0.91,
            },
          ],
          total: 1,
        },
      }),
    }),
  );

  await page.goto(ORIGIN + `/search?sapp=${appId}`, { waitUntil: "domcontentloaded", timeout: 40000 });
  // The settings sidebar lives inside the results view, which starts hidden
  // until the first search runs.
  await page.waitForSelector("#relatedSearch", { state: "attached", timeout: 15000 });
  await page.waitForTimeout(600);
  out.seededSapp = await page.evaluate(() => ({
    related: window.__SAPP__?.related_search,
    id: window.__SAPP__?.id,
    switchChecked: document.getElementById("relatedSearch")?.checked ?? null,
  }));

  await page.fill("#query", "how does rag work");
  await page.click("#searchHome button.btn-p");
  await page.waitForSelector("#relatedSearch", { state: "visible", timeout: 15000 });
  out.seeded = await snap();
  await shot("seeded");
  await page.waitForSelector(".search-related-chip", { timeout: 15000 });
  out.afterSearch = await snap();
  out.calls = calls.slice();
  await shot("chips");

  const chip = (await snap()).chips[1];
  await page.click(".search-related-chip >> nth=1");
  await page.waitForTimeout(600);
  out.afterClick = await snap();
  out.clickCalls = calls.slice();
  out.clickedChip = chip;

  // Toggling the switch off hides the block and stops new calls.
  const before = calls.length;
  await page.uncheck("#relatedSearch");
  await page.waitForTimeout(250);
  out.afterToggleOff = await snap();
  await page.fill("#q2", "second query");
  await page.click("#searchView button.btn-p >> nth=0");
  await page.waitForTimeout(700);
  out.callsAfterOff = calls.length - before;
  await shot("toggle-off");

  // The search app edit page exposes the same switch and persists it.
  await page.goto(ORIGIN + `/searches/${appId}`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("#sa_related", { timeout: 15000 });
  out.editPage = { exists: true, checked: await page.isChecked("#sa_related") };
  await page.uncheck("#sa_related");
  await page.click("button.btn-p >> nth=-1");
  await page.waitForTimeout(2500);
  out.persisted = await page.evaluate(async (id) => {
    const app = (await (await fetch("/api/v1/searchapps")).json()).data.find((a) => a.id === id);
    return app ? app.related_search : null;
  }, appId);
  // Restore the probe app to the enabled state, then delete it.
  await page.evaluate(async (id) => {
    await fetch("/api/v1/searchapps/" + id, { method: "DELETE" });
  }, appId);
  out.cleanedUp = await page.evaluate(async (id) => {
    const apps = (await (await fetch("/api/v1/searchapps")).json()).data || [];
    return !apps.some((a) => a.id === id);
  }, appId);
} catch (error) {
  out.fatal = String(error && error.stack ? error.stack : error).slice(0, 400);
} finally {
  await page.unroute("**/api/v1/chat/recommendation").catch(() => {});
  await page.unroute("**/api/v1/retrieval").catch(() => {});
  console.log(JSON.stringify(out, null, 2));
  await page.close();
  await browser.close();
}
