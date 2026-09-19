// CDP probe for the search mind-map progress (`next-search/hooks.ts`
// `usePendingMindMap` + `components/ui/progress.tsx`): opening the sheet starts a
// one-second interval whose counter feeds `Number(((count / 43) * 100).toFixed(0))`
// into a `Progress` bar moved by `translateX(-(100 - value)%)`; the interval stops
// itself past 40 and the bar is hidden as soon as the request resolves. The
// mind-map request is stubbed with a delay so the ramp is observable, and the
// probe never leaves the page dirty.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/search-mindmap-progress-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}
const QUESTION = "mindmap probe question";
let datasetId = null;
const MIND = { name: "Probe root", children: [{ name: "Probe child", children: [] }] };

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
await page.setViewportSize({ width: 1500, height: 1000 });
const results = { errors: [], pageErrors };
mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });
const check = (ok, label, detail) => {
  results[label] = ok ? "ok" : "FAIL";
  if (!ok) results.errors.push(label + (detail ? ` (${detail})` : ""));
};

// The result bar (and with it the mind-map trigger) only appears after a search, so
// the retrieval call is answered with one canned chunk.
await page.route("**/api/v1/retrieval", async (route) => {
  await route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify({
      code: 0,
      message: "ok",
      data: {
        total: 1,
        chunks: [
          {
            chunk_id: "probe-chunk",
            content: "Probe chunk content",
            document_id: "probe-doc",
            document_name: "probe-doc.pdf",
            similarity: 0.9,
            vector_similarity: 0.9,
            term_similarity: 0.9,
            document_metadata: {},
          },
        ],
      },
    }),
  });
});

await page.route("**/api/v1/chat/mindmap", async (route) => {
  await new Promise((resolve) => setTimeout(resolve, 3500));
  await route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify({ code: 0, message: "ok", data: MIND }),
  });
});

const progressState = () =>
  page.evaluate(() => {
    const box = document.getElementById("mindProgress");
    const bar = document.getElementById("mindProgressBar");
    return {
      hidden: box.hidden,
      now: Number(box.getAttribute("aria-valuenow")),
      role: box.getAttribute("role"),
      min: box.getAttribute("aria-valuemin"),
      max: box.getAttribute("aria-valuemax"),
      transform: bar ? getComputedStyle(bar).transform : null,
      track: getComputedStyle(box).backgroundColor,
      trackRadius: getComputedStyle(box).borderTopLeftRadius,
      height: getComputedStyle(box).height,
      timerActive: typeof MIND_PROGRESS_TIMER !== "undefined" && MIND_PROGRESS_TIMER !== null,
    };
  });

try {
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.evaluate(async (password) => {
    const res = await fetch("/api/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "admin@rayrag.local", password }),
    });
    const data = await res.json();
    localStorage.setItem("rayrag_token", data.data.access_token);
    document.cookie = "rayrag_token=" + data.data.access_token + ";path=/";
  }, PASSWORD);
  // `searchMindmapOpen()` only runs for a search app (`window.__SAPP__.id`), so the
  // probe drives `/search?sapp={id}` with a throwaway app.
  const seeded = await page.evaluate(async () => {
    const responses = await Promise.all([fetch("/api/v1/datasets"), fetch("/api/v1/searchapps")]);
    let datasets = (await responses[0].json()).data || [];
    const apps = (await responses[1].json()).data || [];
    // An app without knowledge bases opens the settings panel instead of the search
    // view, so a throwaway dataset is created when the tenant has none.
    let createdDataset = null;
    if (!datasets.length) {
      const created = await (
        await fetch("/api/v1/datasets", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ name: "mindmap-probe-kb", description: "probe", language: "English" }),
        })
      ).json();
      createdDataset = created.data && created.data.id;
      datasets = [{ id: createdDataset }];
    }
    const payload = {
      name: "mindmap-progress-probe",
      kb_ids: datasets.length ? [datasets[0].id] : [],
      // `query_mindmap` is what reveals the sheet's trigger in the result bar.
      query_mindmap: true,
      top_k: 10,
    };
    const existing = apps.find((app) => app.name === "mindmap-progress-probe");
    if (existing) {
      await fetch("/api/v1/searchapps/" + existing.id, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      });
      return { appId: existing.id, datasetId: createdDataset };
    }
    const created = await (
      await fetch("/api/v1/searchapps", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      })
    ).json();
    return { appId: created.data && created.data.id, datasetId: createdDataset };
  });
  const appId = seeded.appId;
  datasetId = seeded.datasetId || null;
  results.appId = appId;
  check(!!appId, "search_app_seeded", String(appId));

  await page.goto(ORIGIN + `/search?sapp=${appId}`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.evaluate((question) => {
    const input = document.getElementById("q2");
    input.value = question;
    input.dispatchEvent(new Event("input", { bubbles: true }));
  }, QUESTION);

  // The ramp's arithmetic, straight from the page.
  results.formula = await page.evaluate(() => ({
    zero: mindProgressPercent(0),
    one: mindProgressPercent(1),
    two: mindProgressPercent(2),
    ten: mindProgressPercent(10),
    cap: mindProgressPercent(42),
  }));
  check(
    results.formula.zero === 0 &&
      results.formula.one === 2 &&
      results.formula.two === 5 &&
      results.formula.ten === 23 &&
      results.formula.cap === 98,
    "progress_formula",
    JSON.stringify(results.formula),
  );

  // The search view (and with it the result bar that owns the mind-map trigger) is
  // only revealed by the centred home form's own Search button.
  await page.fill("#query", QUESTION);
  await page.click("#searchHome .sb button");
  await page.waitForSelector("[data-testid='search-mindmap']", { state: "visible", timeout: 20000 });

  // Real click on the mind-map control.
  const before = await progressState();
  await page.click("[data-testid='search-mindmap']");
  await page.waitForTimeout(250);
  const at0 = await progressState();
  await page.waitForTimeout(1050);
  const at1 = await progressState();
  await page.waitForTimeout(1050);
  const at2 = await progressState();
  results.ramp = { before, at0, at1, at2 };
  check(
    before.hidden === true && at0.hidden === false && at0.now === 0,
    "ramp_starts_at_zero",
    JSON.stringify({ before: before.hidden, at0 }),
  );
  check(
    at1.now >= 1 && at1.now <= 4 && at2.now > at1.now && at2.now >= 4 && at2.now <= 8,
    "ramp_climbs_each_second",
    JSON.stringify({ at1: at1.now, at2: at2.now }),
  );
  check(
    at1.role === "progressbar" && at1.min === "0" && at1.max === "100",
    "progress_aria",
    JSON.stringify({ role: at1.role, min: at1.min, max: at1.max }),
  );
  check(
    at1.height === "4px" && at1.trackRadius === "9999px" && at1.track === "rgba(76, 164, 231, 0.05)",
    "progress_track",
    JSON.stringify({ height: at1.height, radius: at1.trackRadius, background: at1.track }),
  );
  // `translateX(-(100 - value)%)` over a 100%-wide indicator.
  const expectedShift = 100 - at1.now;
  const matrix = (at1.transform || "").match(/matrix\(([^)]+)\)/);
  const shifted = matrix ? Number(matrix[1].split(",")[4]) : null;
  const width = await page.locator("#mindProgressBar").evaluate((node) => node.getBoundingClientRect().width);
  results.fill = { now: at1.now, shifted, width };
  check(
    shifted !== null && Math.abs(shifted + (expectedShift / 100) * width) < 1.5,
    "fill_translateX",
    JSON.stringify(results.fill),
  );

  // The response resolves: the bar hides, the tree renders and the timer is gone.
  await page.waitForSelector("#mindTree .mind-root", { timeout: 15000 });
  await page.waitForTimeout(400);
  const after = await progressState();
  results.after = after;
  check(
    after.hidden === true && after.timerActive === false,
    "progress_stops_on_success",
    JSON.stringify(after),
  );
  results.tree = await page.locator("#mindTree").innerText();
  check(results.tree.includes("Probe root") && results.tree.includes("Probe child"), "tree_rendered", results.tree);

  // Reopening restarts the ramp, and closing the sheet mid-flight stops the timer.
  await page.click("[data-testid='mindmap-close']");
  await page.waitForTimeout(300);
  await page.click("[data-testid='search-mindmap']");
  await page.waitForSelector("#mindProgress:not([hidden])", { timeout: 10000 });
  await page.waitForTimeout(1200);
  const midFlight = await progressState();
  results.midFlight = midFlight;
  check(midFlight.hidden === false && midFlight.timerActive === true, "reopen_restarts", JSON.stringify(midFlight));
  await page.click("[data-testid='mindmap-close']");
  await page.waitForTimeout(300);
  const closed = await page.evaluate(() => ({
    sheetHidden: document.getElementById("mindSheet").hidden,
    timerActive: typeof MIND_PROGRESS_TIMER !== "undefined" && MIND_PROGRESS_TIMER !== null,
  }));
  results.closed = closed;
  check(closed.sheetHidden === true && closed.timerActive === false, "close_stops_timer", JSON.stringify(closed));

  check(pageErrors.length === 0, "no_page_errors", pageErrors.join(" | "));
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/search-mindmap-progress-failure.png" }).catch(() => {});
} finally {
  if (datasetId) {
    const cleaned = await page
      .evaluate(async (ids) => {
        const res = await fetch("/api/v1/datasets", {
          method: "DELETE",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ ids }),
        });
        const body = await res.text();
        if (!body) return res.ok ? ids.length : 0;
        try {
          return JSON.parse(body).data.success_count;
        } catch {
          return res.ok ? ids.length : 0;
        }
      }, [datasetId])
      .catch(() => 0);
    results.cleanup_deleted = cleaned === 1 ? "ok" : "FAIL";
    if (cleaned !== 1) results.errors.push(`cleanup_deleted (${cleaned})`);
  }
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
