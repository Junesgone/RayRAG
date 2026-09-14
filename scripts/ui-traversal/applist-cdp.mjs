// Traverse the RayRAG application list pages (/chats, /searches) like a user:
// login → list → create dialog → card ⋮ menu → delete dialog, with screenshots.
//
// Usage:
//   UI_DUMP_PASSWORD=... node scripts/ui-traversal/applist-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.UI_DUMP_PASSWORD;
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

const out = { pages: [] };
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

  for (const [path, testid, name] of [
    ["/chats", "chats-list", "chats"],
    ["/searches", "search-list", "searches"],
  ]) {
    const record = { path };
    await page.goto(ORIGIN + path, {
      waitUntil: "domcontentloaded",
      timeout: 40000,
    });
    await page.waitForTimeout(3000);
    await page.screenshot({ path: `${SHOT_DIR}/rayrag-${name}-list.png` });
    record.cards = await page.locator(`[data-testid='${testid}-card']`).count();
    record.title = (await page.textContent(".list-title"))?.trim();
    record.buttons = await page.evaluate(() =>
      Array.from(document.querySelectorAll(".list-bar-actions button")).map(
        (el) => (el.textContent || "").trim(),
      ),
    );

    // Create dialog
    await page
      .locator(
        `[data-testid='${testid === "chats-list" ? "create-chat" : "create-search"}']`,
      )
      .click({ timeout: 8000 })
      .catch(() => {});
    await page.waitForTimeout(900);
    await page.screenshot({ path: `${SHOT_DIR}/rayrag-${name}-create.png` });
    record.createDialog = {
      visible: await page.isVisible(`[data-testid='${testid}-form-dialog']`),
      title: (await page.textContent("#lpFormTitle"))?.trim(),
      placeholder: await page.getAttribute("#lpFormName", "placeholder"),
    };
    await page
      .locator(`[data-testid='${testid}-form-dialog'] .btn-o`)
      .click({ timeout: 5000 })
      .catch(() => {});
    await page.waitForTimeout(400);

    // Card ⋮ menu
    const more = page.locator(`[data-testid='${testid}-card-more']`).first();
    if (await more.count()) {
      await more.click({ timeout: 8000 }).catch(() => {});
      await page.waitForTimeout(800);
      await page.screenshot({ path: `${SHOT_DIR}/rayrag-${name}-menu.png` });
      record.menu = await page.evaluate(() =>
        Array.from(document.querySelectorAll("#lpCardMenu button")).map((el) =>
          (el.textContent || "").trim(),
        ),
      );
      // Delete confirm dialog (do not confirm)
      const del = page.locator("#lpCardMenu button.danger").first();
      if (await del.count()) {
        await del.click({ timeout: 5000 }).catch(() => {});
        await page.waitForTimeout(700);
        await page.screenshot({
          path: `${SHOT_DIR}/rayrag-${name}-delete.png`,
        });
        record.deleteDialog = {
          visible: await page.isVisible(
            `[data-testid='${testid}-delete-dialog']`,
          ),
          title: (await page.textContent("#lpDeleteTitle"))?.trim(),
        };
        await page
          .locator(`[data-testid='${testid}-delete-dialog'] .btn-o`)
          .click({ timeout: 5000 })
          .catch(() => {});
      }
    }
    out.pages.push(record);
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
