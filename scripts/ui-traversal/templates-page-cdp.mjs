// Traverse the RayRAG agent template gallery like a user: login → category
// clicks → hover a card → use-template dialog, with screenshots.
//
// Usage: UI_DUMP_PASSWORD=... node scripts/ui-traversal/templates-page-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.UI_DUMP_PASSWORD;
const EMAIL = process.env.UI_DUMP_EMAIL || "admin@rayrag.local";
const SHOT = process.env.SHOT_DIR || "/tmp";

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
      const res = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email, password }),
      });
      const data = await res.json();
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

  // API contract first
  out.api = await page.evaluate(async () => {
    const list = await (await fetch("/api/v1/agents/templates")).json();
    const items = list.data || [];
    const first = items[0];
    const detail = first
      ? await (
          await fetch(
            "/api/v1/agents/templates/" + encodeURIComponent(first.id),
          )
        ).json()
      : null;
    return {
      count: items.length,
      firstId: first?.id,
      firstTitle: first?.title?.en,
      canvasTypes: [...new Set(items.map((x) => x.canvas_type))].slice(0, 8),
      detailHasDsl: !!detail?.data?.dsl,
    };
  });

  await page.goto(ORIGIN + "/agent-templates", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForTimeout(3000);
  await page.screenshot({ path: `${SHOT}/rayrag-templates-list.png` });
  out.steps.push({
    step: "templates-list",
    categories: await page.evaluate(() =>
      Array.from(document.querySelectorAll(".template-sidebar-item")).map(
        (el) => ({
          text: (el.textContent || "").trim(),
          active: el.classList.contains("active"),
        }),
      ),
    ),
    cards: await page.locator("[data-testid='agent-template-card']").count(),
  });

  // Category switch: Ingestion Pipeline
  await page
    .locator("[data-testid='template-category-ingestion-pipeline']")
    .click({ timeout: 8000 })
    .catch(() => {});
  await page.waitForTimeout(900);
  await page.screenshot({ path: `${SHOT}/rayrag-templates-pipeline.png` });
  out.steps.push({
    step: "category-pipeline",
    cards: await page.locator("[data-testid='agent-template-card']").count(),
    active: await page.evaluate(() =>
      document
        .querySelector(".template-sidebar-item.active")
        ?.textContent?.trim(),
    ),
  });

  // Back to Recommended, hover + open the use dialog
  await page
    .locator("[data-testid='template-category-recommended']")
    .click({ timeout: 8000 })
    .catch(() => {});
  await page.waitForTimeout(700);
  const card = page.locator("[data-testid='agent-template-card']").first();
  if (await card.count()) {
    await card.hover({ timeout: 5000 }).catch(() => {});
    await page.waitForTimeout(400);
    await page.screenshot({ path: `${SHOT}/rayrag-templates-hover.png` });
    await card.click({ timeout: 8000 }).catch(() => {});
    await page.waitForTimeout(900);
    await page.screenshot({ path: `${SHOT}/rayrag-templates-dialog.png` });
    out.steps.push({
      step: "use-dialog",
      visible: await page.isVisible("[data-testid='agent-template-dialog']"),
      title: (await page.textContent("#templateDialogTitle"))?.trim(),
      name: await page.inputValue("#templateAgentName"),
    });
    await page
      .locator("[data-testid='agent-template-dialog'] .btn-o")
      .click({ timeout: 5000 })
      .catch(() => {});
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
