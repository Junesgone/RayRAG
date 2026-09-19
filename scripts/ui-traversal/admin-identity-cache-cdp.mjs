// CDP probe for the console identity cache (`pages/admin/layouts/root-layout.tsx`
// + `layouts/../login.tsx` + `layouts/navigation-layout.tsx`): a successful
// console login writes `Authorization` / `token` / `userInfo` — the last one the
// login payload plus `name: nickname` — and lifts the context to
// `{userInfo, source: 'serverRequest'}`; a reload reads that cache back as
// `source: 'localStorage'`; logging out removes all three keys and returns to
// `/admin`. The probe leaves the browser context signed in again so other probes
// keep working.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/admin-identity-cache-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}
const EMAIL = "admin@rayrag.local";
const KEYS = ["Authorization", "token", "userInfo", "rayrag_token"];

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
await page.setViewportSize({ width: 1500, height: 1000 });
const results = { errors: [], pageErrors };
mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });
const check = (ok, label, detail) => {
  results[label] = ok ? "ok" : "FAIL";
  if (!ok) results.errors.push(label + (detail ? ` (${detail})` : ""));
};

const signIn = async () => {
  await page.goto(ORIGIN + "/admin", { waitUntil: "domcontentloaded", timeout: 40000 });
  if (!(await page.locator("#adminEmail").count())) {
    // A live session short-circuits `/admin`; drop it so the form is reachable.
    await page.evaluate(() => {
      for (const key of ["Authorization", "token", "userInfo", "rayrag_token"]) {
        localStorage.removeItem(key);
      }
      document.cookie = "rayrag_token=;path=/;max-age=0";
    });
    await page.goto(ORIGIN + "/admin", { waitUntil: "domcontentloaded", timeout: 40000 });
  }
  await page.fill("#adminEmail", EMAIL);
  await page.fill("#adminPassword", PASSWORD);
  await page.click("[data-testid='admin-login-submit']");
  await page.waitForURL("**/admin/services", { timeout: 20000 });
};

const storage = () =>
  page.evaluate((keys) => {
    const out = {};
    for (const key of keys) out[key] = localStorage.getItem(key);
    return out;
  }, KEYS);

try {
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await signIn();

  // The login page wrote the upstream trio and lifted the context.
  const afterLogin = await storage();
  results.after_login = {
    hasAuthorization: /^Bearer .+/.test(afterLogin.Authorization || ""),
    tokenMatchesAuthorization:
      (afterLogin.Authorization || "").replace(/^Bearer /, "") === afterLogin.token,
    rayragTokenMatches: afterLogin.rayrag_token === afterLogin.token,
    userInfoKeys: Object.keys(JSON.parse(afterLogin.userInfo || "{}")).sort(),
    name: JSON.parse(afterLogin.userInfo || "{}").name,
    nickname: JSON.parse(afterLogin.userInfo || "{}").nickname,
    email: JSON.parse(afterLogin.userInfo || "{}").email,
    source: await page.evaluate(() => window.ADMIN_USER_INFO.source),
  };
  check(
    results.after_login.hasAuthorization &&
      results.after_login.tokenMatchesAuthorization &&
      results.after_login.rayragTokenMatches &&
      results.after_login.email === EMAIL &&
      results.after_login.name === results.after_login.nickname &&
      results.after_login.source === "serverRequest",
    "login_writes_the_cache",
    JSON.stringify(results.after_login),
  );

  // A reload reads the cache back through `getLocalStorageUserInfo()`.
  await page.goto(ORIGIN + "/admin/users", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='admin-user-row']", { timeout: 20000 });
  await page.waitForTimeout(500);
  results.after_reload = {
    source: await page.evaluate(() => adminCurrentUserInfo().source),
    email: await page.evaluate(() => adminCurrentUserInfo().userInfo.email),
    identityEmail: await page.evaluate(() => adminConsoleIdentityEmail()),
    adminMe: await page.evaluate(() => ADMIN_ME),
    adminMeServer: await page.evaluate(() => ADMIN_ME_SERVER),
  };
  check(
    results.after_reload.source === "localStorage" &&
      results.after_reload.email === EMAIL &&
      results.after_reload.identityEmail === EMAIL &&
      results.after_reload.adminMe === EMAIL &&
      results.after_reload.adminMeServer === EMAIL,
    "reload_reads_the_cache",
    JSON.stringify(results.after_reload),
  );

  // The own row still gets the badge (the check upstream drives from
  // `userInfo?.email`).
  results.own_row_badges = await page
    .locator("[data-testid='admin-user-status-badge']")
    .count();
  check(results.own_row_badges === 1, "own_row_badge", String(results.own_row_badges));

  // Logging out clears the trio, empties the context and lands back on `/admin`.
  await page.click(".admin-console-logout");
  await page.waitForURL("**/admin", { timeout: 20000 });
  await page.waitForTimeout(600);
  const afterLogout = await storage();
  results.after_logout = {
    values: afterLogout,
    source: await page.evaluate(() => window.ADMIN_USER_INFO.source),
    userInfo: await page.evaluate(() => window.ADMIN_USER_INFO.userInfo),
  };
  check(
    KEYS.every((key) => !afterLogout[key]) &&
      results.after_logout.source === null &&
      results.after_logout.userInfo === null,
    "logout_clears_the_cache",
    JSON.stringify(results.after_logout),
  );

  check(pageErrors.length === 0, "no_page_errors", pageErrors.join(" | "));

  // Leave the shared browser context signed in for the other probes.
  await signIn();
  results.relogin = /^Bearer .+/.test((await storage()).Authorization || "") ? "ok" : "FAIL";
  if (results.relogin !== "ok") results.errors.push("relogin");
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/admin-identity-cache-failure.png" }).catch(() => {});
} finally {
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
