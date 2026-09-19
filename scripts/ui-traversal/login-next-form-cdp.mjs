// CDP probe for `/login` (`pages/login-next/index.tsx`): both card faces share one
// zod schema, so an empty login form reports "Please input email" / "Please input
// password" under their own fields, a malformed address reports zod's
// "Invalid email", the register face adds a required nickname (with
// `NICKNAME_PATTERN`) and the remember-me label tints with its checkbox. The card
// is the upstream 540px / `pt-14 pl-10 pr-10 pb-2` block with the
// `metallic-gradient` submit at `my-8`. Nothing is submitted to the API.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/login-next-form-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const EMAIL_REQUIRED = "Please input email";
const PASSWORD_REQUIRED = "Please input password";
const EMAIL_INVALID = "Invalid email";
const NICKNAME_REQUIRED = "Please input nickname";
const NICKNAME_INVALID = "Name can only contain letters, numbers, spaces, and . _ ' -";

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
const text = async (selector) => (await page.locator(selector).innerText()).trim();
// Both flip faces carry `.auth-card`, so every style lookup takes the first match.
const styleOf = (selector, props) =>
  page.locator(selector).first().evaluate((node, list) => {
    const style = getComputedStyle(node);
    return Object.fromEntries(list.map((prop) => [prop, style[prop]]));
  }, props);

const writes = [];
page.on("request", (request) => {
  const url = new URL(request.url());
  if (["/api/v1/auth/login", "/api/v1/user/register"].includes(url.pathname)) {
    writes.push(`${request.method()} ${url.pathname}`);
  }
});

try {
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='auth-submit']", { timeout: 20000 });

  // Login face: empty submit → one message per field, nothing sent.
  await page.click("[data-testid='auth-submit']");
  await page.waitForTimeout(400);
  results.empty_login = {
    email: await text("[data-testid='auth-email-error']"),
    password: await text("[data-testid='auth-password-error']"),
    dialogLevel: await text("#error"),
    requests: writes.length,
  };
  check(
    results.empty_login.email === EMAIL_REQUIRED &&
      results.empty_login.password === PASSWORD_REQUIRED &&
      results.empty_login.dialogLevel === "" &&
      results.empty_login.requests === 0,
    "empty_login_blocks",
    JSON.stringify(results.empty_login),
  );

  // Malformed then valid address, plus a password.
  await page.fill("[data-testid='auth-email']", "not-an-email");
  await page.waitForTimeout(150);
  const invalid = await text("[data-testid='auth-email-error']");
  await page.fill("[data-testid='auth-email']", "user@example.com");
  await page.fill("[data-testid='auth-password']", "secret");
  await page.waitForTimeout(150);
  results.live_login = {
    invalid,
    emailCleared: (await text("[data-testid='auth-email-error']")) === "",
    passwordCleared: (await text("[data-testid='auth-password-error']")) === "",
  };
  check(
    results.live_login.invalid === EMAIL_INVALID &&
      results.live_login.emailCleared &&
      results.live_login.passwordCleared,
    "live_login_validation",
    JSON.stringify(results.live_login),
  );

  // Remember-me tint.
  results.remember = {
    before: (await styleOf(".auth-remember", ["color"])).color,
    checkedBefore: await page.locator(".auth-remember").evaluate((node) => node.classList.contains("checked")),
  };
  await page.check("#remember");
  await page.waitForTimeout(150);
  results.remember.after = (await styleOf(".auth-remember", ["color"])).color;
  results.remember.checkedAfter = await page.locator(".auth-remember").evaluate((node) => node.classList.contains("checked"));
  results.remember.box = await styleOf("#remember", ["width", "height", "padding", "appearance", "borderRadius"]);
  check(
    results.remember.checkedBefore === false &&
      results.remember.checkedAfter === true &&
      results.remember.before !== results.remember.after &&
      results.remember.box.width === "14px" &&
      results.remember.box.height === "14px" &&
      results.remember.box.padding === "0px" &&
      results.remember.box.appearance === "none" &&
      results.remember.box.borderRadius === "2px",
    "remember_tint",
    JSON.stringify(results.remember),
  );

  // Card + submit geometry.
  results.geometry = {
    card: await styleOf(".auth-card", ["maxWidth", "paddingTop", "paddingRight", "paddingBottom", "paddingLeft", "borderRadius"]),
    heading: await styleOf(".auth-heading", ["marginBottom"]),
    headingFont: await styleOf(".auth-heading h2", ["fontSize", "fontWeight"]),
    formGap: (await styleOf(".auth-form", ["rowGap"])).rowGap,
    submit: await styleOf("[data-testid='auth-submit']", ["height", "marginTop", "marginBottom", "borderBottomWidth", "borderBottomColor", "backgroundImage"]),
    switch: await styleOf("#authSwitchToRegister", ["marginTop", "fontSize"]),
  };
  check(
    results.geometry.card.maxWidth === "540px" &&
      results.geometry.card.paddingTop === "56px" &&
      results.geometry.card.paddingRight === "40px" &&
      results.geometry.card.paddingBottom === "8px" &&
      results.geometry.card.borderRadius === "10px" &&
      results.geometry.heading.marginBottom === "32px" &&
      results.geometry.headingFont.fontSize === "20px" &&
      results.geometry.headingFont.fontWeight === "600" &&
      results.geometry.formGap === "32px" &&
      results.geometry.submit.height === "40px" &&
      results.geometry.submit.marginTop === "32px" &&
      results.geometry.submit.marginBottom === "32px" &&
      results.geometry.submit.borderBottomWidth === "2px" &&
      results.geometry.submit.backgroundImage.includes("linear-gradient") &&
      results.geometry.switch.marginTop === "40px",
    "auth_geometry",
    JSON.stringify(results.geometry),
  );

  // Register face: flip the card and check the nickname rules.
  await page.click("[data-testid='auth-toggle-register']");
  await page.waitForTimeout(600);
  results.register_face = await page.locator("#authRegisterFace").evaluate((node) => ({
    testid: node.getAttribute("data-testid"),
  }));
  await page.click("#regForm [data-testid='auth-submit']");
  await page.waitForTimeout(400);
  results.empty_register = {
    nickname: await text("[data-testid='auth-nickname-error']"),
    email: await text("[data-testid='auth-register-email-error']"),
    password: await text("[data-testid='auth-register-password-error']"),
    requests: writes.length,
  };
  check(
    results.empty_register.nickname === NICKNAME_REQUIRED &&
      results.empty_register.email === EMAIL_REQUIRED &&
      results.empty_register.password === PASSWORD_REQUIRED &&
      results.empty_register.requests === 0,
    "empty_register_blocks",
    JSON.stringify(results.empty_register),
  );
  await page.fill("[data-testid='auth-nickname']", "bad*name!");
  await page.waitForTimeout(150);
  const nicknameInvalid = await text("[data-testid='auth-nickname-error']");
  await page.fill("[data-testid='auth-nickname']", "good name");
  await page.waitForTimeout(150);
  results.nickname = {
    invalid: nicknameInvalid,
    cleared: (await text("[data-testid='auth-nickname-error']")) === "",
  };
  check(
    results.nickname.invalid === NICKNAME_INVALID && results.nickname.cleared,
    "nickname_pattern",
    JSON.stringify(results.nickname),
  );

  check(pageErrors.length === 0, "no_page_errors", pageErrors.join(" | "));
  results.writes = writes;
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/login-next-form-failure.png" }).catch(() => {});
} finally {
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
