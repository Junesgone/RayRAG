// CDP probe for the user-setting shell (web/src/pages/user-setting/index.tsx +
// sidebar/hooks.tsx): upstream renders w-16 (64px) on mobile and md:w-[303px] on
// desktop, with exactly one active item derived from the current route.
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
const keys = ["/model", "/data-source", "/chat-channel", "/mcp", "/team", "/profile", "/api"];
const sidebarWidth = () =>
  page.evaluate(() => {
    const el = document.querySelector(".user-setting-sidebar");
    return el ? Math.round(el.getBoundingClientRect().width * 10) / 10 : null;
  });
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

  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(ORIGIN + "/user-setting/model", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector(".user-setting-sidebar", { timeout: 20000 });
  out.desktopWidth = await sidebarWidth();
  out.items = await page.evaluate(() =>
    Array.from(document.querySelectorAll("[data-setting-key]")).map((node) => ({
      key: node.dataset.settingKey,
      label: (node.querySelector(".user-setting-nav-label")?.textContent || "").trim(),
      active: node.classList.contains("active"),
    })),
  );
  await page.setViewportSize({ width: 390, height: 900 });
  await page.waitForTimeout(200);
  out.mobileWidth = await sidebarWidth();
  await page.screenshot({
    path: "docs/ui-traversal/rayrag-audit/user-setting-shell-mobile-v051.png",
    fullPage: true,
  });

  await page.setViewportSize({ width: 1440, height: 900 });
  for (const key of keys) {
    await page.goto(ORIGIN + "/user-setting" + key, { waitUntil: "domcontentloaded", timeout: 40000 });
    await page.waitForSelector(".user-setting-sidebar", { timeout: 20000 });
    const state = await page.evaluate(() => {
      const active = Array.from(document.querySelectorAll("[data-setting-key]")).filter(
        (node) => node.getAttribute("aria-current") === "page",
      );
      return {
        activeKeys: active.map((node) => node.dataset.settingKey),
        h1: (document.querySelector("main h1, .ct h1")?.textContent || "").trim(),
        width: Math.round(document.querySelector(".user-setting-sidebar").getBoundingClientRect().width),
      };
    });
    out.steps.push({ key, ...state });
  }
  await page.screenshot({
    path: "docs/ui-traversal/rayrag-audit/user-setting-shell-desktop-v051.png",
    fullPage: true,
  });

  // Back/forward keeps the active item in sync with the URL.
  await page.goto(ORIGIN + "/user-setting/model", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector(".user-setting-sidebar", { timeout: 20000 });
  await page.goto(ORIGIN + "/user-setting/profile", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector(".user-setting-sidebar", { timeout: 20000 });
  await page.goBack({ waitUntil: "commit", timeout: 15000 }).catch(() => {});
  await page.waitForTimeout(900);
  out.backActive = await page.evaluate(
    () => document.querySelector("[data-setting-key][aria-current='page']")?.dataset.settingKey,
  );
  await page.goForward({ waitUntil: "commit", timeout: 15000 }).catch(() => {});
  await page.waitForTimeout(900);
  out.forwardActive = await page.evaluate(
    () => document.querySelector("[data-setting-key][aria-current='page']")?.dataset.settingKey,
  );
} catch (error) {
  out.error = String(error && error.message ? error.message : error).slice(0, 400);
} finally {
  out.pageErrors = pageErrors;
  console.log(JSON.stringify(out, null, 1));
  await page.close().catch(() => {});
  await b.close().catch(() => {});
  process.exit(out.error || pageErrors.length ? 1 : 0);
}
