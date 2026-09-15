// Traverse the dataset Retrieval testing tab: open the page, perform a real
// retrieval run, and record the payload + visible result count.
//
// Usage: UI_DUMP_PASSWORD=... node scripts/ui-traversal/dataset-testing-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.UI_DUMP_PASSWORD;
const SHOT = process.env.SHOT_DIR || "/tmp";

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1100 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 200)));
const retrievalBodies = [];
page.on("request", (req) => {
  if (
    req.url().includes("/api/v1/retrieval") &&
    req.method() === "POST" &&
    req.postData()
  ) {
    try {
      retrievalBodies.push(JSON.parse(req.postData()));
    } catch {}
  }
});

const out = {};
try {
  await page.goto(ORIGIN + "/login", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  const ok = await page.evaluate(
    async ({ password }) => {
      const res = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email: "admin@rayrag.local", password }),
      });
      const data = await res.json();
      if (data.code === 0 && data.data?.access_token) {
        localStorage.setItem("rayrag_token", data.data.access_token);
        return true;
      }
      return false;
    },
    { password: PASSWORD },
  );
  if (!ok) throw new Error("login failed");

  const kb = await page.evaluate(async () => {
    const res = await fetch("/api/v1/datasets");
    return ((await res.json()).data || [])[0]?.id || "";
  });
  await page.goto(`${ORIGIN}/dataset/retrieval/${kb}`, {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForTimeout(3000);
  await page.screenshot({
    path: `${SHOT}/rayrag-dataset-testing.png`,
    fullPage: true,
  });

  out.title = (await page.textContent("#tab2 h2"))?.trim();
  out.leftHeader = (await page.textContent("#tab2 h3"))?.trim();
  out.questionPlaceholder = await page.getAttribute("#testQ", "placeholder");
  out.runLabel = (await page.textContent("#tab2 button.btn-p"))?.trim();
  out.hasKg = await page.isVisible("#testUseKg");
  out.crossLanguageOptions = await page.evaluate(
    () => document.querySelectorAll("#testCrossLangs option").length,
  );

  // real run
  await page.fill("#testQ", "probe retrieval test");
  await page.check("#testUseKg");
  await page.evaluate(() => {
    const select = document.getElementById("testCrossLangs");
    if (select) select.options[0].selected = true;
  });
  await page
    .locator("#tab2 button.btn-p")
    .click({ timeout: 8000 })
    .catch(() => {});
  await page.waitForTimeout(4000);
  out.lastRetrievalBody = retrievalBodies[retrievalBodies.length - 1] || null;
  await page.screenshot({
    path: `${SHOT}/rayrag-dataset-testing-run.png`,
    fullPage: true,
  });
} catch (error) {
  out.error = String(error && error.message ? error.message : error).slice(
    0,
    300,
  );
}
out.pageErrors = pageErrors;
console.log(JSON.stringify(out, null, 1));
await page.close().catch(() => {});
process.exit(0);
