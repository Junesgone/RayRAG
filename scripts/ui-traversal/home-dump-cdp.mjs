// Home page parity dump: records the `/` page structure (welcome heading,
// section titles, segmented control, cards, empty states) for whichever origin
// is configured, so RAGFlow and RayRAG can be compared side by side.
//
// Usage:
//   RAGFLOW_UI_ORIGIN=http://127.0.0.1:11526 node scripts/ui-traversal/home-dump-cdp.mjs
//   UI_DUMP_AUTH=rayrag UI_DUMP_PASSWORD=... RAGFLOW_UI_ORIGIN=http://127.0.0.1:9390 \
//     node scripts/ui-traversal/home-dump-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAGFLOW_UI_ORIGIN || "http://127.0.0.1:11526";
const EMAIL = process.env.UI_DUMP_EMAIL || "admin@rayrag.local";
const PASSWORD = process.env.UI_DUMP_PASSWORD || "";
const AUTH = process.env.UI_DUMP_AUTH || "none";

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 200)));

if (AUTH === "rayrag") {
  if (!PASSWORD) {
    console.error("UI_DUMP_PASSWORD is required when UI_DUMP_AUTH=rayrag");
    process.exit(2);
  }
  await page.goto(ORIGIN + "/login", {
    waitUntil: "domcontentloaded",
    timeout: 45000,
  });
  const login = await page.evaluate(
    async ({ email, password }) => {
      const response = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email, password }),
      });
      const data = await response.json();
      if (data.code !== 0 || !data.data || !data.data.access_token) {
        return { ok: false, body: data };
      }
      localStorage.setItem("rayrag_token", data.data.access_token);
      return { ok: true };
    },
    { email: EMAIL, password: PASSWORD },
  );
  if (!login.ok) {
    console.error("RayRAG login failed: " + JSON.stringify(login));
    process.exit(1);
  }
}

const DUMP = () => {
  const main = document.querySelector("main") || document.body;
  const text = (el) =>
    (el.textContent || "").replace(/\s+/g, " ").trim().slice(0, 80);
  return {
    url: location.href,
    mainText: (main.innerText || "").replace(/\n{2,}/g, "\n").slice(0, 1200),
    headings: Array.from(main.querySelectorAll("h1,h2,h3")).map((el) => ({
      tag: el.tagName.toLowerCase(),
      text: text(el),
      classes: String(el.className || "").slice(0, 80),
    })),
    cards: Array.from(
      main.querySelectorAll(
        "article, [class*='card'], [data-testid*='card'], [data-testid*='app-card']",
      ),
    )
      .map((el) => ({
        testid: el.getAttribute("data-testid") || "",
        text: text(el),
        classes: String(el.className || "").slice(0, 80),
      }))
      .filter((card) => card.text)
      .slice(0, 24),
    segmented: Array.from(
      main.querySelectorAll(
        "[role='tab'], [role='tablist'] button, [class*='segmented'] button, [class*='Segmented'] button",
      ),
    ).map((el) => ({
      text: text(el),
      role: el.getAttribute("role") || "",
      selected: el.getAttribute("aria-selected"),
    })),
    buttons: Array.from(main.querySelectorAll("button"))
      .map((el) => ({
        text: text(el),
        ariaLabel: el.getAttribute("aria-label") || "",
        testid: el.getAttribute("data-testid") || "",
      }))
      .filter((button) => button.text || button.ariaLabel)
      .slice(0, 24),
  };
};

const out = { origin: ORIGIN };
try {
  await page.goto(ORIGIN + "/", {
    waitUntil: "domcontentloaded",
    timeout: 45000,
  });
  await page.waitForTimeout(4000);
  out.dump = await page.evaluate(DUMP);

  // Click every Segmented option like a user and record what each tab shows.
  out.tabs = [];
  for (const label of ["Chat", "Search", "Agent", "Memory"]) {
    // Both implementations render the control differently (radix Segmented vs
    // a plain button group), so match on exact visible text outside the header.
    const clicked = await page.evaluate((text) => {
      const candidates = Array.from(
        document.querySelectorAll("button, [role='tab']"),
      );
      const target = candidates.find((element) => {
        // Nav entries are anchors in both implementations, so an exact-text
        // button match cannot collide with the global header.
        if (element.closest("nav")) return false;
        if ((element.textContent || "").trim() !== text) return false;
        const rect = element.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
      if (!target) return false;
      target.click();
      return true;
    }, label);
    if (!clicked) {
      out.tabs.push({ label, found: false });
      continue;
    }
    await page.waitForTimeout(2500);
    out.tabs.push({
      label,
      found: true,
      sectionTitle: await page.evaluate(() => {
        const headings = Array.from(document.querySelectorAll("h2"));
        const last = headings[headings.length - 1];
        return last ? (last.textContent || "").trim().slice(0, 60) : "";
      }),
      // Card area content: upstream renders dashed EmptyAppCards, RayRAG the
      // .home-empty-card / .home-card pair.
      cards: await page.evaluate(() => {
        // Scope to the applications row when the implementation exposes one,
        // otherwise fall back to the whole page (upstream renders an
        // unlabelled section).
        const host = document.querySelector("#homeApps") || document;
        return Array.from(
          host.querySelectorAll(
            "[data-testid='home-app-card'], [data-testid^='home-empty-'], .home-card, .home-empty-card, article[class*='border-dashed'], [class*='border-dashed']",
          ),
        )
          .map((el) =>
            (el.textContent || "").replace(/\s+/g, " ").trim().slice(0, 60),
          )
          .filter(Boolean)
          .slice(0, 4);
      }),
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
