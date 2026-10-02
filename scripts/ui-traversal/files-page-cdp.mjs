// Traverse the RayRAG file manager like a user: login → Add file dropdown →
// New folder dialog → row selection/bulk bar → screenshots at each step.
//
// Usage:
//   UI_DUMP_PASSWORD=... RAYRAG_UI_ORIGIN=http://127.0.0.1:9390 \
//     node scripts/ui-traversal/files-page-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
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

  await page.goto(ORIGIN + "/files", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForTimeout(3500);
  await page.screenshot({ path: `${SHOT_DIR}/rayrag-files-list.png` });
  out.steps.push({
    step: "files-list",
    rows: await page.locator("#fl tr").count(),
    header: await page.evaluate(() =>
      Array.from(document.querySelectorAll(".file-table th")).map((th) =>
        (th.textContent || "").trim(),
      ),
    ),
  });

  // Add file dropdown.
  await page
    .locator("[data-testid='files-add']")
    .click({ timeout: 8000 })
    .catch(() => {});
  await page.waitForTimeout(700);
  await page.screenshot({ path: `${SHOT_DIR}/rayrag-files-addmenu.png` });
  out.steps.push({
    step: "add-file-menu",
    items: await page.evaluate(() =>
      Array.from(document.querySelectorAll("#addFileMenu .menu-item")).map(
        (el) => (el.textContent || "").trim(),
      ),
    ),
    expanded: await page.getAttribute(
      "[data-testid='files-add']",
      "aria-expanded",
    ),
  });

  // New folder dialog.
  await page
    .locator("[data-testid='files-new-folder']")
    .click({ timeout: 8000 })
    .catch(() => {});
  await page.waitForTimeout(800);
  await page.screenshot({ path: `${SHOT_DIR}/rayrag-files-newfolder.png` });
  out.steps.push({
    step: "new-folder-dialog",
    visible: await page.isVisible("#newFolderModal"),
    title: (await page.textContent("#newFolderTitle"))?.trim(),
    buttons: await page.evaluate(() =>
      Array.from(document.querySelectorAll("#newFolderModal button")).map(
        (el) => (el.textContent || "").trim(),
      ),
    ),
  });
  await page
    .locator("#newFolderModal .btn-o")
    .click({ timeout: 5000 })
    .catch(() => {});
  await page.waitForTimeout(400);

  // Row selection → bulk bar.
  const firstRow = page.locator("#fl tr").first();
  if (await firstRow.count()) {
    const box = firstRow.locator("input.fileCk");
    if (await box.count()) {
      await box.check({ timeout: 5000 }).catch(() => {});
      await page.waitForTimeout(600);
    }
  }
  await page.screenshot({ path: `${SHOT_DIR}/rayrag-files-bulk.png` });
  out.steps.push({
    step: "bulk-bar",
    hidden: await page.getAttribute("#fileBatchBar", "hidden"),
    count: (await page.textContent("#fileSelCount"))?.trim(),
    buttons: await page.evaluate(() =>
      Array.from(document.querySelectorAll("#fileBatchBar button")).map((el) =>
        (el.textContent || "").trim(),
      ),
    ),
    pagination: (await page.textContent("#filePagination"))
      ?.replace(/\s+/g, " ")
      .trim(),
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
