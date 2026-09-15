// Skills page "Configure Search" dialog (upstream search-config-modal.tsx):
// open it, check the weight/threshold live labels, toggle a field, adjust its
// weight, save and confirm the config round-trips.
//
// Usage: UI_DUMP_PASSWORD=... node scripts/ui-traversal/skills-config-cdp.mjs
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
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 180)));
const out = { steps: [] };

const snap = async (label) => {
  const state = await page.evaluate(() => ({
    modalOpen: document.getElementById("skillConfigModal").classList.contains("show"),
    title: document.querySelector("#skillConfigModal h2")?.textContent.trim(),
    description: document.querySelector("#skillConfigModal .user-setting-description")?.textContent.trim(),
    weight: document.getElementById("skillVectorWeight")?.value,
    weightLabel: (document.getElementById("skillWeightLabel")?.textContent || "").trim(),
    threshold: document.getElementById("skillThreshold")?.value,
    thresholdLabel: (document.getElementById("skillThresholdLabel")?.textContent || "").trim(),
    topK: document.getElementById("skillTopK")?.value,
    fields: Array.from(document.querySelectorAll("#skillFieldRows .skill-field-row")).map((row) => ({
      label: row.querySelector(".set-sw-label")?.textContent.trim(),
      enabled: row.querySelector("input[type=checkbox]")?.checked,
      weight: row.querySelector("input[type=number]")?.value,
    })),
  }));
  out.steps.push({ label, ...state });
  await page.screenshot({ path: `${SHOT}/rayrag-skills-config-${out.steps.length}-${label}.png` });
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
  await page.goto(`${ORIGIN}/skills`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(2200);
  await page.locator("button[onclick='openSkillConfig()']").click({ timeout: 8000 });
  await page.waitForTimeout(1200);
  await snap("opened");

  // Slider → live hybrid label (upstream getSearchTypeLabel).
  await page.locator("#skillVectorWeight").fill("0.6");
  await page.waitForTimeout(250);
  await snap("weight-0.6");
  await page.locator("#skillThreshold").fill("0.45");
  await page.waitForTimeout(250);
  await snap("threshold-0.45");

  // Toggle the tags field off and bump the description weight.
  await page.locator("#skillField-tags").uncheck({ timeout: 8000 });
  await page.locator("#skillFieldWeight-description").fill("2.5");
  await page.locator("#skillTopK").fill("12");
  await page.waitForTimeout(250);
  await snap("fields-edited");

  // Save and read the persisted config back.
  await page.locator("#skillConfigModal button:has-text('Save')").click({ timeout: 8000 });
  await page.waitForTimeout(1500);
  out.saved = await page.evaluate(async () => (await (await fetch("/api/v1/skills/config?space_id=default")).json()).data);
  await page.locator("button[onclick='openSkillConfig()']").click({ timeout: 8000 });
  await page.waitForTimeout(1200);
  await snap("reopened");
} catch (error) {
  out.error = String(error && error.message ? error.message : error).slice(0, 300);
}
out.pageErrors = pageErrors;
console.log(JSON.stringify(out, null, 1));
await page.close().catch(() => {});
process.exit(0);
