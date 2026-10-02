// Traverse the RayRAG memory list like a user: login → list → create dialog
// (memory types + embedding/LLM selectors) → card ⋮ menu, with screenshots.
//
// Usage:
//   UI_DUMP_PASSWORD=... node scripts/ui-traversal/memories-page-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD || process.env.UI_DUMP_PASSWORD;
const EMAIL = process.env.UI_DUMP_EMAIL || "admin@rayrag.local";
const SHOT_DIR = process.env.SHOT_DIR || "/tmp";

if (!PASSWORD) {
  console.error("UI_DUMP_PASSWORD is required");
  process.exit(2);
}

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 200)));

const out = { steps: [] };
try {
  await page.goto(ORIGIN + "/login", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  const login = await page.evaluate(
    async ({ email, password }) => {
      const response = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email, password }),
      });
      const data = await response.json();
      if (data.code === 0 && data.data?.access_token) {
        localStorage.setItem("rayrag_token", data.data.access_token);
        return true;
      }
      return false;
    },
    { email: EMAIL, password: PASSWORD },
  );
  out.login = login;
  if (!login) throw new Error("login failed");

  await page.goto(ORIGIN + "/memories", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForTimeout(3500);
  await page.screenshot({ path: `${SHOT_DIR}/rayrag-memories-list.png` });
  out.steps.push({
    step: "memory-list",
    title: (await page.textContent(".list-title"))?.trim(),
    cards: await page.locator("[data-testid='memory-list-card']").count(),
    button: (await page.textContent("[data-testid='create-memory']"))?.trim(),
  });

  await page
    .locator("[data-testid='create-memory']")
    .click({ timeout: 8000 })
    .catch(() => {});
  await page.waitForTimeout(1500);
  await page.screenshot({ path: `${SHOT_DIR}/rayrag-memories-create.png` });
  out.steps.push({
    step: "create-dialog",
    title: (await page.textContent("#lpFormTitle"))?.trim(),
    namePlaceholder: await page.getAttribute("#lpFormName", "placeholder"),
    types: await page.evaluate(() =>
      Array.from(document.querySelectorAll(".lp-memory-type")).map((el) => ({
        value: el.value,
        checked: el.checked,
        disabled: el.disabled,
      })),
    ),
    embedTrigger: await page.evaluate(() => {
      const trigger = document.getElementById("memListEmbd");
      return trigger ? trigger.dataset.value || "" : null;
    }),
    llmTrigger: await page.evaluate(() => {
      const trigger = document.getElementById("memListLlm");
      return trigger ? trigger.dataset.value || "" : null;
    }),
  });
  await page
    .locator("[data-testid='memory-list-form-dialog'] .btn-o")
    .click({ timeout: 5000 })
    .catch(() => {});
  await page.waitForTimeout(500);

  const more = page.locator("[data-testid='memory-list-card-more']").first();
  if (await more.count()) {
    await more.click({ timeout: 8000 }).catch(() => {});
    await page.waitForTimeout(800);
    await page.screenshot({ path: `${SHOT_DIR}/rayrag-memories-menu.png` });
    out.steps.push({
      step: "card-menu",
      items: await page.evaluate(() =>
        Array.from(document.querySelectorAll("#lpCardMenu button")).map((el) =>
          (el.textContent || "").trim(),
        ),
      ),
    });
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
