// Traverse the RayRAG login page like a user: flip to register, flip back,
// remember me, channel area, and the post-login redirect, with screenshots.
//
// Usage: UI_DUMP_PASSWORD=... node scripts/ui-traversal/login-page-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD || process.env.UI_DUMP_PASSWORD;
const SHOT = process.env.SHOT_DIR || "/tmp";

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1440, height: 900 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 200)));

const out = { steps: [] };
try {
  await page.goto(ORIGIN + "/login", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForTimeout(1500);
  await page.screenshot({ path: `${SHOT}/rayrag-login-front.png` });
  out.steps.push({
    step: "login-face",
    activeFace: await page.evaluate(
      () => document.querySelector("[data-testid='auth-card-active']")?.id,
    ),
    activeForm: await page.evaluate(
      () =>
        document.querySelector("[data-testid='auth-form'][data-active='true']")
          ?.id,
    ),
    hasHeader: await page.evaluate(
      () => !!document.querySelector("header.hdr"),
    ),
    hasRemember: await page.isVisible("#remember"),
    channelHidden: await page.getAttribute("#authChannels", "hidden"),
  });

  await page
    .locator("[data-testid='auth-toggle-register']")
    .click({ timeout: 8000 });
  await page.waitForTimeout(1200);
  await page.screenshot({ path: `${SHOT}/rayrag-login-flipped.png` });
  out.steps.push({
    step: "register-face",
    activeFace: await page.evaluate(
      () => document.querySelector("[data-testid='auth-card-active']")?.id,
    ),
    activeForm: await page.evaluate(
      () =>
        document.querySelector("[data-testid='auth-form'][data-active='true']")
          ?.id,
    ),
    flipped: await page.evaluate(() =>
      document.getElementById("authStage").classList.contains("flipped"),
    ),
  });

  await page
    .locator("[data-testid='auth-toggle-login']")
    .click({ timeout: 8000 });
  await page.waitForTimeout(1200);
  out.steps.push({
    step: "back-to-login",
    activeFace: await page.evaluate(
      () => document.querySelector("[data-testid='auth-card-active']")?.id,
    ),
  });

  // Real login with remember-me, then confirm the home redirect.
  await page.fill("#email", "admin@rayrag.local");
  await page.fill("#password", PASSWORD || "");
  await page.check("#remember");
  await page
    .locator("#loginForm [data-testid='auth-submit']")
    .click({ timeout: 8000 });
  await page.waitForTimeout(4000);
  out.steps.push({
    step: "after-login",
    url: page.url(),
    rememberedEmail: await page.evaluate(() =>
      localStorage.getItem("rayrag_email"),
    ),
    hasToken: await page.evaluate(() => !!localStorage.getItem("rayrag_token")),
  });
  await page.screenshot({ path: `${SHOT}/rayrag-login-after.png` });
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
