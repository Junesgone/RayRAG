// Regression probe: the login page must render standalone and must never enter
// the expired-session redirect loop (fetch /api/v1/user/info → 401 →
// location.href='/login' → reload → ...).
//
// Usage:
//   RAYRAG_UI_ORIGIN=http://127.0.0.1:9390 node scripts/ui-traversal/login-loop-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1440, height: 900 });

const navigations = [];
page.on("framenavigated", (frame) => {
  if (frame === page.mainFrame()) navigations.push(frame.url());
});
const requests = [];
page.on("request", (request) => {
  if (request.url().includes("/api/v1/user/info")) requests.push(request.url());
});
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error).slice(0, 200)));

const out = { origin: ORIGIN };
try {
  // 1. Clear any session, then load /login and watch for reload loops.
  await page.goto(ORIGIN + "/login", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.evaluate(() => {
    localStorage.removeItem("rayrag_token");
    document.cookie = "rayrag_token=;path=/;max-age=0";
  });
  await page.goto(ORIGIN + "/login", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  navigations.length = 0;
  requests.length = 0;
  await page.waitForTimeout(9000);
  out.login = {
    navigations: navigations.length,
    userInfoRequests: requests.length,
    url: page.url(),
    hasGlobalHeader: await page.evaluate(
      () => !!document.querySelector("header.hdr"),
    ),
    hasAuthForm: await page.evaluate(
      () => !!document.getElementById("loginForm"),
    ),
    navItems: await page.evaluate(
      () => document.querySelectorAll("header.gnav a, .gnav a").length,
    ),
  };

  // 2. A protected page without a session may bounce to /login exactly once,
  //    but must then stay there.
  navigations.length = 0;
  requests.length = 0;
  await page.goto(ORIGIN + "/datasets", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForTimeout(9000);
  out.protected = {
    navigations: navigations.length,
    finalUrl: page.url(),
    userInfoRequests: requests.length,
  };
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
