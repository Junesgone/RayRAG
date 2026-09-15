// Login page: verify the upstream BgSvg/Spotlight background renders, the dash
// animation state, and the register gate driven by /api/v1/system/config.
//
// Usage: UI_DUMP_PASSWORD=... node scripts/ui-traversal/login-bg-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
const SHOT = process.env.SHOT_DIR || "/tmp";

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 180)));

const out = {};
try {
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(1600);
  out.background = await page.evaluate(() => {
    const bands = Array.from(document.querySelectorAll(".auth-bg-band"));
    const spotlight = Array.from(document.querySelectorAll(".spotlight"));
    const glow = document.querySelector(".animate-glow");
    const highlight = document.querySelector(".animate-highlight");
    const style = glow ? getComputedStyle(glow) : null;
    return {
      bands: bands.map((b) => ({
        cls: b.className,
        h: Math.round(b.getBoundingClientRect().height),
        w: Math.round(b.getBoundingClientRect().width),
        svg: !!b.querySelector("svg"),
        viewBox: b.querySelector("svg")?.getAttribute("viewBox") || "",
      })),
      spotlights: spotlight.length,
      glowAnimation: style ? style.animationName : "",
      glowDuration: style ? style.animationDuration : "",
      highlightAnimation: highlight ? getComputedStyle(highlight).animationName : "",
      maskStrokeWidth: style ? style.strokeWidth : "",
      bgPointerEvents: getComputedStyle(document.querySelector(".auth-bg")).pointerEvents,
    };
  });
  await page.screenshot({ path: `${SHOT}/rayrag-login-bg.png` });

  // Register gate: default (enabled) shows the toggle, disabled hides it and
  // blocks the flip.
  out.gateEnabled = await page.evaluate(() => ({
    switchVisible: !document.getElementById("authSwitchToRegister").hidden,
  }));
  await page.route("**/api/v1/system/config", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ code: 0, data: { registerEnabled: 0, disablePasswordLogin: false } }),
    }),
  );
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.waitForTimeout(1200);
  out.gateDisabled = await page.evaluate(() => ({
    switchHidden: document.getElementById("authSwitchToRegister").hidden,
    registerEnabledFlag: typeof AUTH_REGISTER_ENABLED === "boolean" ? AUTH_REGISTER_ENABLED : null,
    activeFace: document.querySelector("[data-testid='auth-card-active']")?.id || "",
  }));
  await page.screenshot({ path: `${SHOT}/rayrag-login-bg-gated.png` });

  // SSO-only mode: `disablePasswordLogin` hides both password forms and the
  // register/login toggles while keeping the channel section.
  await page.unroute("**/api/v1/system/config");
  await page.route("**/api/v1/system/config", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ code: 0, data: { registerEnabled: 0, disablePasswordLogin: true } }),
    }),
  );
  await page.route("**/api/v1/auth/login/channels", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        code: 0,
        data: [{ channel: "github", display_name: "Github", icon: "github" }],
      }),
    }),
  );
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.waitForTimeout(1200);
  out.ssoOnly = await page.evaluate(() => ({
    passwordFormHidden: document.getElementById("loginForm").hidden,
    registerFormHidden: document.getElementById("regForm").hidden,
    switchToRegisterHidden: document.getElementById("authSwitchToRegister").hidden,
    switchToLoginHidden: document.getElementById("authSwitchToLogin").hidden,
    channelsVisible: !document.getElementById("authChannels").hidden,
    channelsSolo: document.getElementById("authChannels").classList.contains("auth-channels-solo"),
    channelButtons: Array.from(document.querySelectorAll(".auth-channel-btn")).map((b) => b.textContent.trim()),
    headingHidden: document.getElementById("authTitle").hidden,
    passwordLoginDisabled: AUTH_PASSWORD_LOGIN_DISABLED,
  }));
  await page.screenshot({ path: `${SHOT}/rayrag-login-sso-only.png` });
  await page.unroute("**/api/v1/system/config");
  await page.unroute("**/api/v1/auth/login/channels");

  // Endpoints themselves.
  out.endpoints = await page.evaluate(async () => {
    const config = await (await fetch("/api/v1/system/config")).json();
    const version = await (await fetch("/api/v1/system/version")).json();
    return { config, version };
  });
} catch (error) {
  out.error = String(error && error.message ? error.message : error).slice(0, 300);
}
out.pageErrors = pageErrors;
console.log(JSON.stringify(out, null, 1));
await page.close().catch(() => {});
process.exit(0);
