// Verify the dataset Configuration tab like a user: open the page, read the
// Basic field order, toggle the language select, then screenshot.
//
// Usage: UI_DUMP_PASSWORD=... node scripts/ui-traversal/dataset-config-cdp.mjs
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
  await page.goto(`${ORIGIN}/dataset/configuration/${kb}`, {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForTimeout(3000);
  await page.screenshot({
    path: `${SHOT}/rayrag-dataset-config.png`,
    fullPage: true,
  });

  out.basicOrder = await page.evaluate(() => {
    const ids = [
      "cfgName",
      "cfgLang",
      "cfgAvatar",
      "cfgDescription",
      "cfgPerm",
      "cfgPageRank",
      "cfgTagSets",
    ];
    const rows = Array.from(document.querySelectorAll("#tab3 tr"));
    return rows
      .map((row) => {
        const control = row.querySelector("input, select, textarea");
        const id = control ? control.id : "";
        return ids.includes(id) ? id : null;
      })
      .filter(Boolean);
  });
  out.languageOptions = await page.evaluate(() =>
    Array.from(document.querySelectorAll("#cfgLang option")).map((o) =>
      o.textContent.trim(),
    ),
  );
  out.saveButton = await page.getAttribute(
    "[data-testid='ds-settings-basic-save-btn']",
    "class",
  );
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
