// CDP probe for the user-setting data-source page
// (web/src/pages/user-setting/data-source/index.tsx): the upstream page has an
// added-sources list, a responsive available-source card grid (1 / 2 / 4
// columns at the sm/lg/2xl breakpoints) and no per-source search input (the
// upstream search is commented out in v0.27.2). Hovering a card reveals Add.
import { chromium } from "playwright-core";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
const EMAIL = process.env.RAYRAG_ADMIN_EMAIL || "admin@rayrag.local";
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required");
  process.exit(2);
}
const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e.message)));
const out = { steps: [], pageErrors };
try {
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const loggedIn = await page.evaluate(
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
  if (!loggedIn) throw new Error("login failed");
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(ORIGIN + "/user-setting/data-source", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("#dsAvailable", { timeout: 20000 });
  await page.waitForFunction(
    () => document.querySelectorAll("#dsAvailable .cc-available-card").length > 0,
    { timeout: 20000 },
  );

  out.cardCount = await page.evaluate(
    () => document.querySelectorAll("#dsAvailable .cc-available-card").length,
  );
  out.hasSearchInput = await page.evaluate(
    () => !!document.querySelector("#dsAvailable input[type='search'], .ds-avail-container input"),
  );
  out.sections = await page.evaluate(() =>
    Array.from(document.querySelectorAll("main > .card, .ct > .card, .card h2")).map((node) =>
      (node.textContent || "").trim().slice(0, 40),
    ),
  );

  // The upstream grid: 1 column, then 2 at >=1024px, 4 at >=1536px (container).
  for (const width of [390, 768, 1024, 1280, 1536, 1920]) {
    await page.setViewportSize({ width, height: 900 });
    await page.waitForTimeout(150);
    const cols = await page.evaluate(() => {
      const grid = document.querySelector(".ds-available-grid");
      if (!grid) return null;
      const css = getComputedStyle(grid).gridTemplateColumns;
      return css.split(" ").filter(Boolean).length;
    });
    out.steps.push({ width, columns: cols });
  }

  await page.setViewportSize({ width: 1500, height: 1000 });
  const firstCard = page.locator("#dsAvailable .cc-available-card").first();
  await firstCard.hover();
  await page.waitForTimeout(300);
  out.hoverAdd = await page.evaluate(() => {
    const card = document.querySelector("#dsAvailable .cc-available-card");
    if (!card) return null;
    const add = Array.from(card.querySelectorAll("button")).find((btn) => /add/i.test(btn.textContent || ""));
    if (!add) return { found: false };
    const style = getComputedStyle(add);
    return { found: true, opacity: style.opacity, visible: style.visibility };
  });
  await page.screenshot({
    path: "docs/ui-traversal/rayrag-audit/data-source-grid-v051.png",
    fullPage: true,
  });
} catch (error) {
  out.error = String(error && error.message ? error.message : error).slice(0, 400);
  await page
    .screenshot({ path: "docs/ui-traversal/rayrag-audit/data-source-grid-failure.png" })
    .catch(() => {});
} finally {
  out.pageErrors = pageErrors;
  console.log(JSON.stringify(out, null, 1));
  await page.close().catch(() => {});
  await b.close().catch(() => {});
  process.exit(out.error || pageErrors.length ? 1 : 0);
}
