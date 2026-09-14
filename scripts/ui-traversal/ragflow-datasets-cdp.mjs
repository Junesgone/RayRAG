// Traverse the RAGFlow dataset list page like a user: empty state → create
// dialog → created card → card overflow menu → delete. Records every
// intermediate DOM surface so the RayRAG `/datasets` page can be matched
// control by control. All interactions use real Playwright clicks: RAGFlow's
// React handlers do not fire for synthetic `element.click()` on wrappers.
//
// Usage:
//   RAGFLOW_UI_ORIGIN=http://127.0.0.1:11526 node scripts/ui-traversal/ragflow-datasets-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAGFLOW_UI_ORIGIN || "http://127.0.0.1:11526";
const NAME = process.env.DATASET_PROBE_NAME || "CDP-DATASET-PARITY";

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 200)));

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
  const label = (el) =>
    (
      el.getAttribute("aria-label") ||
      el.getAttribute("title") ||
      el.getAttribute("placeholder") ||
      (el.textContent || "").trim()
    )
      .replace(/\s+/g, " ")
      .slice(0, 60);
  return {
    url: location.href,
    text: (document.body.innerText || "")
      .replace(/\n{2,}/g, "\n")
      .slice(0, 700),
    buttons: Array.from(document.querySelectorAll("button"))
      .filter(visible)
      .map(label)
      .filter(Boolean)
      .slice(0, 30),
    inputs: Array.from(document.querySelectorAll("input, textarea, select"))
      .filter(visible)
      .map((el) => ({
        tag: el.tagName.toLowerCase(),
        type: el.getAttribute("type") || "",
        placeholder: el.getAttribute("placeholder") || "",
        name: el.getAttribute("name") || "",
        value: el.value ? String(el.value).slice(0, 30) : "",
      }))
      .slice(0, 20),
    dialogs: Array.from(document.querySelectorAll("[role='dialog']"))
      .filter(visible)
      .map((el) =>
        (el.textContent || "").replace(/\s+/g, " ").trim().slice(0, 300),
      ),
    cards: Array.from(document.querySelectorAll("article"))
      .filter(visible)
      .map((el) =>
        (el.textContent || "").replace(/\s+/g, " ").trim().slice(0, 60),
      )
      .filter(Boolean)
      .slice(0, 12),
    menuItems: Array.from(
      document.querySelectorAll(
        "[role='menuitem'], [role='menu'] button, [data-radix-popper-content-wrapper] button",
      ),
    )
      .filter(visible)
      .map(label)
      .filter(Boolean)
      .slice(0, 12),
  };
};

const clickText = async (text, { exact = true, timeout = 8000 } = {}) => {
  const pattern = exact
    ? new RegExp(`^\\s*${text}\\s*$`)
    : new RegExp(text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  const locator = page
    .locator(
      "[role='dialog'] button, [role='menu'] button, [role='menuitem'], main button, main a, main [role='button']",
    )
    .filter({ hasText: pattern })
    .first();
  if (!(await locator.count())) return false;
  await locator.click({ timeout, force: true }).catch(() => {});
  await page.waitForTimeout(2000);
  return true;
};

const out = { origin: ORIGIN, steps: [] };
try {
  const cdp = await ctx.newCDPSession(page);
  await cdp.send("Network.clearBrowserCache").catch(() => {});
  await page.goto(ORIGIN + "/datasets", {
    waitUntil: "domcontentloaded",
    timeout: 45000,
  });
  await page.waitForTimeout(4000);
  out.steps.push({
    step: "datasets-initial",
    snapshot: await page.evaluate(SNAPSHOT),
  });

  // 1. The empty card opens the create dialog (upstream EmptyAppCard onClick).
  const emptyCard = page
    .locator("article", { hasText: /No dataset created yet|尚未创建知识库/ })
    .last();
  if (await emptyCard.count()) {
    await emptyCard.click({ timeout: 8000, force: true }).catch(() => {});
    await page.waitForTimeout(2500);
  }
  out.steps.push({
    step: "after-empty-card-click",
    snapshot: await page.evaluate(SNAPSHOT),
  });

  // 2. Fill the create dialog (Name + embedding model + parse type radios).
  out.filled = await page.evaluate(
    ({ name }) => {
      const dialog = document.querySelector("[role='dialog']");
      if (!dialog) return { ok: false };
      const input = dialog.querySelector(
        "input[type='text'], input:not([type])",
      );
      if (!input) return { ok: false };
      const setter = Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        "value",
      ).set;
      setter.call(input, name);
      input.dispatchEvent(new Event("input", { bubbles: true }));
      input.dispatchEvent(new Event("change", { bubbles: true }));
      return {
        ok: true,
        radios: Array.from(dialog.querySelectorAll("input[type='radio']")).map(
          (radio) => ({ value: radio.value, checked: radio.checked }),
        ),
      };
    },
    { name: NAME },
  );
  await page.waitForTimeout(600);
  out.steps.push({
    step: "dialog-filled",
    snapshot: await page.evaluate(SNAPSHOT),
  });
  out.created = await clickText("Save");
  await page.waitForTimeout(5000);
  out.steps.push({
    step: "after-create",
    snapshot: await page.evaluate(SNAPSHOT),
  });

  // 3. Card overflow menu (the ⋮ trigger inside the created card).
  const moreButton = page.locator("article button").last();
  out.menuOpened = false;
  if (await moreButton.count()) {
    await moreButton.click({ timeout: 8000, force: true }).catch(() => {});
    out.menuOpened = true;
    await page.waitForTimeout(1500);
  }
  out.steps.push({
    step: "card-menu",
    snapshot: await page.evaluate(SNAPSHOT),
  });

  // 4. Delete through the menu so the audit account stays clean.
  out.deleted = await clickText("Delete");
  if (out.deleted) {
    await clickText("OK");
    await page.waitForTimeout(3000);
  }
  out.steps.push({
    step: "after-delete",
    snapshot: await page.evaluate(SNAPSHOT),
  });

  // 5. API fallback cleanup keeps the account empty even if the menu missed.
  out.apiCleanup = await page.evaluate(
    async ({ name }) => {
      const headers = { "Content-Type": "application/json" };
      const listed = await fetch("/api/v1/datasets?page=1&page_size=50", {
        headers,
      });
      const body = await listed.json();
      const items = (body.data && (body.data.items || body.data)) || [];
      const removed = [];
      for (const item of items) {
        if (item.name !== name) continue;
        const response = await fetch("/api/v1/datasets", {
          method: "DELETE",
          headers,
          body: JSON.stringify({ ids: [item.id] }),
        });
        removed.push({ id: item.id, status: response.status });
      }
      return removed;
    },
    { name: NAME },
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
