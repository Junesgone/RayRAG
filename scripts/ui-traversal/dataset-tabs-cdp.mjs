// Traverse the dataset detail sidebar like a user: click each upstream route
// link (Files / Retrieval testing / Logs / Configuration / Knowledge graph) and
// record which panel is visible, with screenshots.
//
// Usage: UI_DUMP_PASSWORD=... node scripts/ui-traversal/dataset-tabs-cdp.mjs
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
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 200)));

const out = { tabs: [] };
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
    const data = await res.json();
    return (data.data || [])[0]?.id || "";
  });
  out.kb = kb;

  // The bare detail path redirects into the Files tab.
  await page.goto(`${ORIGIN}/dataset/${kb}`, {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForTimeout(2500);
  out.redirectedTo = page.url();

  for (const [tab, label] of [
    ["files", "Files"],
    ["retrieval", "Retrieval testing"],
    ["logs", "Logs"],
    ["configuration", "Configuration"],
    ["knowledge-graph", "Knowledge graph"],
  ]) {
    const link = page.locator(`a[href$='/dataset/${tab}/${kb}']`).first();
    if (await link.count()) {
      await link.click({ timeout: 8000 }).catch(() => {});
    } else {
      await page.goto(`${ORIGIN}/dataset/${tab}/${kb}`, {
        waitUntil: "domcontentloaded",
        timeout: 40000,
      });
    }
    await page.waitForTimeout(1800);
    const state = await page.evaluate(() => {
      const navs = Array.from(document.querySelectorAll(".ds-nav a"));
      const active = navs.find((a) => a.classList.contains("active"));
      const visible = Array.from(document.querySelectorAll("[id^='tab']"))
        .filter((el) => getComputedStyle(el).display !== "none")
        .map((el) => el.id);
      return {
        activeNav: active ? active.textContent.trim() : "",
        visiblePanels: visible,
        logsPanelInsideOverview: !!document.querySelector("#tab1 #datasetLogs"),
      };
    });
    out.tabs.push({
      tab,
      label,
      url: page.url().replace(ORIGIN, ""),
      ...state,
    });
    await page.screenshot({ path: `${SHOT}/rayrag-dataset-${tab}.png` });
  }
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
