// Exercise the provider-modal Base-Url combobox like a user: open a provider
// card with Base-Url, inspect the trigger, open the popover, switch region,
// clear, type a custom URL and confirm the derived `region` submit field.
//
// Usage: UI_DUMP_PASSWORD=... [RAYRAG_PROVIDER=SILICONFLOW] node scripts/ui-traversal/provider-base-url-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD || process.env.UI_DUMP_PASSWORD;
const SHOT = process.env.SHOT_DIR || "/tmp";
const PROVIDER = process.env.RAYRAG_PROVIDER || "SILICONFLOW";

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 180)));

const out = { provider: PROVIDER, steps: [] };
const snap = async (label) => {
  const state = await page.evaluate(() => {
    const input = document.getElementById("piBase");
    const selected = document.getElementById("piBaseSelected");
    const chip = selected ? selected.querySelector(".input-select-region") : null;
    const popover = document.getElementById("piBasePopover");
    const rows = Array.from(document.querySelectorAll("#piBaseList .input-select-option")).map(
      (row) => (row.textContent || "").replace(/\s+/g, " ").trim().slice(0, 90),
    );
    return {
      inputValue: input ? input.value : null,
      inputHidden: input ? input.hidden : null,
      selectedVisible: selected ? !selected.hidden : null,
      selectedChip: chip ? chip.textContent.trim() : "",
      popoverOpen: popover ? !popover.hidden : null,
      rows,
      derivedRegion:
        typeof providerRegionForBaseUrl === "function"
          ? providerRegionForBaseUrl(input ? input.value : "")
          : "n/a",
    };
  });
  out.steps.push({ label, ...state });
  await page.screenshot({ path: `${SHOT}/rayrag-base-url-${out.steps.length}-${label}.png` });
  return state;
};

try {
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
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
  const cdp = await ctx.newCDPSession(page);
  await cdp.send("Network.clearBrowserCache").catch(() => {});

  await page.goto(`${ORIGIN}/user-setting/model`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(2000);

  const card = page.locator(`[data-testid='available-model-card'][data-provider='${PROVIDER}']`).first();
  if (!(await card.count())) throw new Error(`provider card ${PROVIDER} not found`);
  out.cardShowBase = await card.getAttribute("data-show-base");
  out.cardBaseOptions = await card.getAttribute("data-api-base-options");
  await card.scrollIntoViewIfNeeded();
  await page.screenshot({ path: `${SHOT}/rayrag-base-url-0-card.png` });
  const addButton = page.locator(
    `[data-testid='available-model-card'][data-provider='${PROVIDER}'] .available-provider-add`,
  ).first();
  await (addButton.count() ? addButton.click({ timeout: 8000 }) : card.click({ timeout: 8000 }));
  await page.waitForTimeout(900);
  out.modalOpen = await page.evaluate(() => {
    const modal = document.getElementById("providerInstanceModal");
    return !!modal && modal.classList.contains("show");
  });
  out.tip = await page.evaluate(() => {
    const tip = document.getElementById("piBaseTip");
    return tip && !tip.hidden ? { label: tip.getAttribute("aria-label"), text: tip.textContent } : null;
  });
  out.placeholder = await page.evaluate(() => {
    const input = document.getElementById("piBase");
    return input ? input.placeholder : null;
  });
  await snap("opened");

  // Open the popover (click the chevron/trigger like a user would).
  await page.locator("#piBaseTrigger").click({ timeout: 8000 });
  await page.waitForTimeout(400);
  await snap("popover");

  // Switch to a non-default region when the provider advertises one.
  const target = page.locator("#piBaseList .input-select-option").filter({ hasNotText: "Add " });
  const rowCount = await target.count();
  const pickIndex = rowCount > 1 ? 1 : 0;
  const pickedText = rowCount ? (await target.nth(pickIndex).textContent()) || "" : "";
  if (rowCount) {
    await target.nth(pickIndex).click({ timeout: 8000 });
    await page.waitForTimeout(400);
  }
  out.pickedRow = pickedText.replace(/\s+/g, " ").trim();
  await snap("picked");

  // Clear button empties the field again.
  await page.locator("#piBaseClear").click({ timeout: 8000 });
  await page.waitForTimeout(300);
  await snap("cleared");

  // Typing a custom URL offers the "Add "…"" row and yields no region.
  await page.locator("#piBase").fill("https://example.invalid/v1");
  await page.waitForTimeout(400);
  const custom = await snap("custom");
  out.customRow = custom.rows.find((row) => row.startsWith("Add ")) || "";
  out.customRowOk = out.customRow.includes("https://example.invalid/v1");
} catch (error) {
  out.error = String(error && error.message ? error.message : error).slice(0, 300);
}
out.pageErrors = pageErrors;
console.log(JSON.stringify(out, null, 1));
await page.close().catch(() => {});
process.exit(0);
