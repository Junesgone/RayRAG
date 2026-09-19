// CDP probe for the OAuth landing parameters
// (`hooks/auth-hooks.ts::useOAuthCallback`): the OAuth callback redirects to
// `/?auth={user_id}` on success and `/?error=<code>` on failure, and the page it
// lands on must consume both — store the session and strip the parameter, or show
// the provider's error in a toast and bounce back to `/login`. The probe only
// visits URLs, so no state is created or changed.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/oauth-landing-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
await page.setViewportSize({ width: 1500, height: 950 });
const results = { errors: [], pageErrors };
mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });
const check = (ok, label, detail) => {
  results[label] = ok ? "ok" : "FAIL";
  if (!ok) results.errors.push(label + (detail ? ` (${detail})` : ""));
};

try {
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });

  // The error path: a toast with the provider message, then /login with a clean URL.
  await page.goto(`${ORIGIN}/?error=invalid_state`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='oauth-error-toast']", { timeout: 10000 });
  check(true, "error_toast_renders");
  check(
    (await page.locator("[data-testid='oauth-error-toast']").innerText()).trim() === "invalid_state",
    "error_toast_shows_the_provider_message",
  );
  check(
    (await page.locator("[data-testid='oauth-error-toast']").getAttribute("role")) === "alert",
    "error_toast_is_an_alert",
  );
  await page.waitForURL(/\/login/, { timeout: 10000 });
  check(true, "error_redirects_to_login");
  check(
    !page.url().includes("error="),
    "error_parameter_is_removed",
    page.url(),
  );

  // The success path: the session is stored from `?auth=` and the URL is cleaned
  // without navigating away from the page it landed on.
  const login = await page.evaluate(async (password) => {
    const res = await fetch("/api/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "admin@rayrag.local", password }),
    });
    const data = await res.json();
    if (data.code !== 0) return { ok: false, body: data };
    localStorage.setItem("rayrag_token", data.data.access_token);
    document.cookie = "rayrag_token=" + data.data.access_token + ";path=/";
    return { ok: true, token: data.data.access_token };
  }, PASSWORD);
  if (!login.ok) throw new Error("login failed: " + JSON.stringify(login.body));
  await page.evaluate(() => localStorage.removeItem("rayrag_token"));
  await page.goto(`${ORIGIN}/?auth=probe-token-42`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForFunction(
    () => localStorage.getItem("rayrag_token") === "probe-token-42",
    null,
    { timeout: 10000 },
  );
  check(true, "auth_parameter_is_stored");
  check(
    !page.url().includes("auth="),
    "auth_parameter_is_removed",
    page.url(),
  );
  check(
    (await page.evaluate(() => document.cookie)).includes("rayrag_token=probe-token-42"),
    "auth_parameter_sets_the_cookie",
  );
  // Restore the real session so the browser profile is not left with the probe token.
  await page.evaluate((token) => {
    localStorage.setItem("rayrag_token", token);
    document.cookie = "rayrag_token=" + token + ";path=/";
  }, login.token);
  check(true, "session_restored");
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/oauth-landing.png" });
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
} finally {
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
