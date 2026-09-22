// CDP probe for the sign-up face (`pages/login-next/index.tsx` + upstream
// `api/apps/restful_apis/user_api.py::user_add`).
//
// It drives the real page: flip to the register face, submit a one-off account,
// assert the upstream success notice (`message.registered`) and the flip back to
// the login face, then sign in through the form with the address exactly as it
// was typed (mixed case) to prove the endpoint + the case-insensitive lookup.
// A duplicate submission exercises the upstream `RetCode` error text. The
// account is deleted through the admin API at the end, so the deployment is
// left as it was found.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... RAYRAG_ADMIN_EMAIL=... \
//        node scripts/ui-traversal/register-flow-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
const ADMIN_EMAIL = process.env.RAYRAG_ADMIN_EMAIL || "admin@rayrag.local";
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}

const stamp = Date.now().toString(36);
// Mixed case on purpose: the store normalises to lower case on write and the
// login has to keep accepting what the user typed.
const EMAIL = `Probe.User-${stamp}@Example.com`;
const NICKNAME = `Probe ${stamp}`;
const NEW_PASSWORD = `probe-password-${stamp}`;

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
await page.setViewportSize({ width: 1500, height: 1000 });
const results = { email: EMAIL, errors: [], pageErrors };
mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });
const check = (ok, label, detail) => {
  results[label] = ok ? "ok" : "FAIL";
  if (!ok) results.errors.push(label + (detail ? ` (${detail})` : ""));
};

const notice = () =>
  page.evaluate(() => {
    const node = document.getElementById("error");
    return {
      text: node ? node.textContent : null,
      success: node ? node.classList.contains("success") : null,
      loginActive:
        document.getElementById("loginForm")?.dataset.active === "true" &&
        document.getElementById("regForm")?.dataset.active === "false",
    };
  });

let adminToken = null;
const adminLogin = async () => {
  const res = await fetch(ORIGIN + "/api/v1/auth/login", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ email: ADMIN_EMAIL, password: PASSWORD }),
  });
  const body = await res.json();
  return body?.data?.access_token || null;
};

try {
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForFunction(() => typeof window.register === "function", { timeout: 15000 });

  // The sign-up switch is offered because REGISTER_ENABLED resolves to 1.
  const switchVisible = await page.locator("#authSwitchToRegister").isVisible();
  check(switchVisible, "register_switch_visible");
  await page.click("[data-testid='auth-toggle-register']");
  await page.waitForTimeout(400);
  check(
    await page.evaluate(() => document.getElementById("regForm")?.dataset.active === "true"),
    "flip_to_register_face",
  );

  // Submitting a duplicate first exercises the upstream error text without
  // creating anything.
  await page.fill("#nickname", NICKNAME);
  await page.fill("#remail", EMAIL);
  await page.fill("#rpassword", NEW_PASSWORD);
  await page.click("#regForm button[type='submit']");
  await page.waitForFunction(
    () => (document.getElementById("error")?.textContent || "").length > 0,
    { timeout: 15000 },
  );
  const created = await notice();
  results.firstSubmit = created;
  // Upstream `useRegister` shows `message.registered` (`Registered!`), not the
  // API's welcome sentence, and `onCheck` flips back to the login face.
  check(
    created.text === "Registered!" &&
      created.success === true &&
      created.loginActive === true,
    "register_success_notice_and_flip",
    JSON.stringify(created),
  );
  const prefilled = await page.inputValue("#email");
  check(prefilled === EMAIL, "email_prefilled_on_login_face", prefilled);

  // Duplicate: the same address again answers upstream's 103 message.
  await page.click("[data-testid='auth-toggle-register']");
  await page.waitForTimeout(400);
  await page.fill("#nickname", NICKNAME);
  await page.fill("#remail", EMAIL);
  await page.fill("#rpassword", NEW_PASSWORD);
  await page.click("#regForm button[type='submit']");
  await page.waitForFunction(
    () => (document.getElementById("error")?.textContent || "").includes("already registered"),
    { timeout: 15000 },
  );
  const duplicate = await notice();
  results.duplicateSubmit = duplicate;
  check(
    duplicate.text === `Email: ${EMAIL} has already registered!` && duplicate.success === false,
    "duplicate_email_message",
    JSON.stringify(duplicate),
  );

  // The API transport itself: upstream `get_json_result` answers HTTP 200 with a
  // `RetCode` body, never an HTTP error status.
  const rawPost = async (payload) => {
    const res = await fetch(ORIGIN + "/api/v1/users", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    });
    return { status: res.status, body: await res.json() };
  };
  const missing = await rawPost({});
  results.missingArguments = missing;
  check(
    missing.status === 200 &&
      missing.body.code === 101 &&
      missing.body.message === "required argument are missing: nickname,email,password; " &&
      missing.body.data === undefined,
    "missing_arguments_transport",
    JSON.stringify(missing),
  );
  const dupe = await rawPost({ nickname: NICKNAME, email: EMAIL, password: NEW_PASSWORD });
  results.duplicateResponse = dupe;
  check(
    dupe.status === 200 &&
      dupe.body.code === 103 &&
      dupe.body.message === `Email: ${EMAIL} has already registered!`,
    "duplicate_transport",
    JSON.stringify(dupe),
  );

  // Real sign-in through the form with the address as typed.
  await page.click("[data-testid='auth-toggle-login']");
  await page.waitForTimeout(400);
  await page.fill("#email", EMAIL);
  await page.fill("#password", NEW_PASSWORD);
  await page.click("#loginForm button[type='submit']");
  await page.waitForFunction(() => !location.pathname.startsWith("/login"), { timeout: 20000 });
  results.signedInPath = await page.evaluate(() => location.pathname);
  check(
    results.signedInPath === "/",
    "mixed_case_email_signs_in",
    String(results.signedInPath),
  );

  check(pageErrors.length === 0, "no_page_errors", pageErrors.join(" | "));
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
  await page
    .screenshot({ path: "docs/ui-traversal/rayrag-audit/register-flow-failure.png" })
    .catch(() => {});
} finally {
  // Remove the one-off account so the deployment keeps its original users.
  try {
    adminToken = await adminLogin();
    const res = await fetch(
      ORIGIN + "/api/v1/admin/users/" + encodeURIComponent(EMAIL),
      { method: "DELETE", headers: { Authorization: "Bearer " + adminToken } },
    );
    const body = await res.text();
    results.cleanup = `${res.status} ${body.slice(0, 120)}`;
    check(res.status === 200, "probe_user_removed", results.cleanup);
  } catch (error) {
    results.cleanup = "FAIL " + String(error);
    results.errors.push("cleanup: " + String(error));
  }
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
