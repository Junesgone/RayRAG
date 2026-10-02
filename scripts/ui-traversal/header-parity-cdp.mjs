// Parity probe for the global header: drives the header the way a user does
// (hover the nav pill, open the language dropdown, click the avatar and the
// external icon links) and records what each interaction produces. Run it
// against RAGFlow (reference) and against RayRAG (UI_DUMP_AUTH=rayrag) to diff
// the two headers control by control.
//
// Usage:
//   RAGFLOW_UI_ORIGIN=http://127.0.0.1:11526 node scripts/ui-traversal/header-parity-cdp.mjs
//   UI_DUMP_AUTH=rayrag UI_DUMP_PASSWORD=... RAGFLOW_UI_ORIGIN=http://127.0.0.1:9390 \
//     node scripts/ui-traversal/header-parity-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAGFLOW_UI_ORIGIN || "http://127.0.0.1:11526";
const OUT = process.env.HEADER_PROBE_OUT || "";
const AUTH = process.env.UI_DUMP_AUTH || "none";
const EMAIL = process.env.UI_DUMP_EMAIL || "admin@rayrag.local";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD || process.env.UI_DUMP_PASSWORD || "";

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

const HEADER_DUMP = () => {
  const header = document.querySelector("header");
  if (!header) return { header: null };
  const describe = (el) => ({
    tag: el.tagName.toLowerCase(),
    testid: el.getAttribute("data-testid") || "",
    text: (
      el.getAttribute("aria-label") ||
      el.getAttribute("title") ||
      (el.textContent || "").trim()
    )
      .replace(/\s+/g, " ")
      .slice(0, 60),
    href: el.getAttribute("href") || "",
    target: el.getAttribute("target") || "",
    ariaCurrent: el.getAttribute("aria-current") || "",
    classes: String(el.className || "").slice(0, 90),
  });
  return {
    header: true,
    controls: Array.from(
      header.querySelectorAll("a, button, [role='button'], img"),
    ).map(describe),
    navOrder: Array.from(header.querySelectorAll("nav a")).map((a) => ({
      text: (a.textContent || "").trim().slice(0, 30),
      href: a.getAttribute("href") || "",
      ariaCurrent: a.getAttribute("aria-current") || "",
    })),
  };
};

const out = { origin: ORIGIN, steps: [] };
try {
  await page.goto(ORIGIN + "/", {
    waitUntil: "domcontentloaded",
    timeout: 45000,
  });
  await page.waitForTimeout(3500);
  out.steps.push({ step: "initial", dump: await page.evaluate(HEADER_DUMP) });

  // 1. Avatar / settings entrypoint: does it navigate or open a menu?
  const avatar = page.locator("[data-testid='settings-entrypoint']").first();
  if (await avatar.count()) {
    const before = page.url();
    await avatar.click({ timeout: 8000 }).catch((e) => {
      out.avatarClickError = String(e.message).slice(0, 120);
    });
    await page.waitForTimeout(2500);
    out.steps.push({
      step: "avatar-click",
      before,
      after: page.url(),
      menus: await page.evaluate(() =>
        Array.from(
          document.querySelectorAll(
            "[role='menu'], [role='menuitem'], [data-radix-menu-content]",
          ),
        )
          .map((el) => (el.textContent || "").trim().slice(0, 120))
          .filter(Boolean),
      ),
    });
  } else {
    out.steps.push({ step: "avatar-click", found: false });
  }

  // 2. Language dropdown.
  await page.goto(ORIGIN + "/", {
    waitUntil: "domcontentloaded",
    timeout: 45000,
  });
  await page.waitForTimeout(2500);
  const langButton = page
    .locator("header button[aria-label], header button")
    .filter({ hasText: /English|简体|中文/ })
    .first();
  if (await langButton.count()) {
    out.steps.push({
      step: "language-open",
      buttonText: (await langButton.textContent())?.trim().slice(0, 40),
      items: await (async () => {
        await langButton.click({ timeout: 8000 }).catch(() => {});
        await page.waitForTimeout(1200);
        return page.evaluate(() => ({
          expanded: (() => {
            const trigger = document.querySelector(
              "header [aria-haspopup='menu']",
            );
            return trigger ? trigger.getAttribute("aria-expanded") : null;
          })(),
          items: Array.from(
            document.querySelectorAll(
              "[role='menuitem'], [role='menuitemradio'], [role='menuitemcheckbox'], [data-radix-popper-content-wrapper] a, [data-radix-popper-content-wrapper] div[role='menuitem'], .menu.open .menu-item",
            ),
          )
            .map((el) => (el.textContent || "").trim().slice(0, 40))
            .filter(Boolean),
        }));
      })(),
    });
    await page.keyboard.press("Escape").catch(() => {});
  } else {
    out.steps.push({ step: "language-open", found: false });
  }

  // 3. Every nav item: record hrefs (no navigation side effects).
  out.steps.push({
    step: "nav-items",
    items: await page.evaluate(() =>
      Array.from(document.querySelectorAll("header nav a")).map((a) => ({
        text: (a.textContent || "").trim().slice(0, 30),
        href: a.getAttribute("href") || "",
        testid: a.getAttribute("data-testid") || "",
        ariaLabel: a.getAttribute("aria-label") || "",
      })),
    ),
  });

  // 4. Hover the nav pill: upstream slides an active pill via CSS anchor
  // positioning; record the computed background of the active item.
  const activeNav = page.locator("header nav a[aria-current='page']").first();
  if (await activeNav.count()) {
    await activeNav.hover({ timeout: 5000 }).catch(() => {});
    await page.waitForTimeout(600);
    out.steps.push({
      step: "nav-active-style",
      style: await activeNav.evaluate((el) => {
        const style = getComputedStyle(el);
        return {
          background: style.backgroundColor,
          color: style.color,
          borderRadius: style.borderRadius,
        };
      }),
    });
  }

  // 5. External icon links in the right cluster.
  out.steps.push({
    step: "right-cluster",
    items: await page.evaluate(() => {
      const header = document.querySelector("header");
      if (!header) return [];
      const right = header.lastElementChild;
      if (!right) return [];
      return Array.from(right.querySelectorAll("a, button")).map((el) => ({
        tag: el.tagName.toLowerCase(),
        text: (el.textContent || "").trim().slice(0, 30),
        ariaLabel: el.getAttribute("aria-label") || "",
        href: el.getAttribute("href") || "",
        target: el.getAttribute("target") || "",
      }));
    }),
  });
} catch (error) {
  out.error = String(error && error.message ? error.message : error).slice(
    0,
    300,
  );
}
out.pageErrors = pageErrors;
console.log(JSON.stringify(out, null, 1));
if (OUT) {
  const fs = await import("node:fs/promises");
  await fs.writeFile(OUT, JSON.stringify(out, null, 1));
}
await page.close().catch(() => {});
process.exit(0);
