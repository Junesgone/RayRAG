// Traverse the "Add provider instance" modal on either implementation:
// open it from the first provider card, record every field/button, toggle a
// model, and capture screenshots. Usage:
//   RAGFLOW_UI_ORIGIN=http://127.0.0.1:11526 node scripts/ui-traversal/provider-modal-cdp.mjs
//   UI_DUMP_AUTH=rayrag UI_DUMP_PASSWORD=... RAGFLOW_UI_ORIGIN=http://127.0.0.1:9390 node ...
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAGFLOW_UI_ORIGIN || "http://127.0.0.1:11526";
const AUTH = process.env.UI_DUMP_AUTH || "none";
const PASSWORD = process.env.UI_DUMP_PASSWORD || "";
const SHOT = process.env.SHOT_DIR || "/tmp";
const TAG = process.env.PROBE_TAG || "probe";

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 200)));

if (AUTH === "rayrag") {
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
}

const SNAPSHOT = () => {
  const visible = (el) => {
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return (
      rect.width > 0 &&
      rect.height > 0 &&
      style.visibility !== "hidden" &&
      style.display !== "none"
    );
  };
  const text = (el) =>
    (el.textContent || "").replace(/\s+/g, " ").trim().slice(0, 60);
  const dialogs = Array.from(
    document.querySelectorAll(
      "[role='dialog'], .modal, [data-radix-popper-content-wrapper] > div",
    ),
  ).filter(visible);
  const dialog = dialogs[dialogs.length - 1] || document.body;
  const scope = dialog.contains(document.body) ? document.body : dialog;
  return {
    dialogText: (dialog.textContent || "")
      .replace(/\s+/g, " ")
      .trim()
      .slice(0, 400),
    labels: Array.from(
      scope.querySelectorAll(
        "label, .provider-instance-field, .ragflow-form-item",
      ),
    )
      .filter(visible)
      .map(text)
      .filter(Boolean)
      .slice(0, 20),
    inputs: Array.from(scope.querySelectorAll("input, select, textarea"))
      .filter(visible)
      .map((el) => ({
        type: el.getAttribute("type") || el.tagName.toLowerCase(),
        placeholder: el.getAttribute("placeholder") || "",
        value: el.value ? String(el.value).slice(0, 24) : "",
      }))
      .slice(0, 20),
    buttons: Array.from(scope.querySelectorAll("button"))
      .filter(visible)
      .map(text)
      .filter(Boolean)
      .slice(0, 24),
    switches: Array.from(
      scope.querySelectorAll("[role='switch'], input[type='checkbox']"),
    ).filter(visible).length,
  };
};

const out = { origin: ORIGIN, steps: [] };
try {
  const cdp = await ctx.newCDPSession(page);
  await cdp.send("Network.clearBrowserCache").catch(() => {});
  const path = process.env.MODEL_PATH || "/user-setting/model";
  await page.goto(ORIGIN + path, {
    waitUntil: "domcontentloaded",
    timeout: 45000,
  });
  await page.waitForTimeout(4000);
  await page.screenshot({ path: `${SHOT}/${TAG}-providers.png` });

  // Open the first provider card's Add action.
  const addButton = page
    .locator("button")
    .filter({ hasText: /^\s*(\+\s*)?Add\s*$/ })
    .first();
  out.addButtonFound = await addButton.count();
  if (out.addButtonFound) {
    await addButton.click({ timeout: 8000, force: true }).catch(() => {});
    await page.waitForTimeout(2500);
  }
  await page.screenshot({ path: `${SHOT}/${TAG}-modal.png` });
  out.steps.push({
    step: "modal-open",
    snapshot: await page.evaluate(SNAPSHOT),
  });

  // Toggle "show models" when present.
  const showModels = page
    .locator("button")
    .filter({ hasText: /View models|Show more/ })
    .first();
  if (await showModels.count()) {
    await showModels.click({ timeout: 5000, force: true }).catch(() => {});
    await page.waitForTimeout(1200);
    await page.screenshot({ path: `${SHOT}/${TAG}-modal-models.png` });
    out.steps.push({
      step: "models-expanded",
      snapshot: await page.evaluate(SNAPSHOT),
    });
  }
  out.afterText = (await page.textContent("body"))
    ?.replace(/\s+/g, " ")
    .slice(0, 600);
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
