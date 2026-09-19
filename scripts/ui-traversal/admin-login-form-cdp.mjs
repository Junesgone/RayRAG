// CDP probe for `/admin` (`pages/admin/login.tsx`): three `Spotlight` layers over
// the `BgSvg` bands, the absolutely positioned head (brand `mt-12 ml-12`, title
// `mt-[6.5rem] text-4xl mb-12`), the 540px column at `mt-72 mb-48`, a card split
// into `px-10 pt-14 pb-10` content and `px-10 pt-8 pb-14` footer, one zod-driven
// `FormMessage` per field, a remember-me checkbox that tints its label, and a
// `highlighted` submit. Only invalid input is exercised apart from one real
// sign-in at the end, so the console stays usable for the other probes.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/admin-login-form-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}
const EMAIL = "admin@rayrag.local";
const EMAIL_REQUIRED = "Please input email";
const PASSWORD_REQUIRED = "Please input password";
const INVALID_EMAIL = "Invalid email";

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
const styleOf = (selector, props) =>
  page.locator(selector).evaluate((node, list) => {
    const style = getComputedStyle(node);
    return Object.fromEntries(list.map((prop) => [prop, style[prop]]));
  }, props);

const logins = [];
page.on("request", (request) => {
  const url = new URL(request.url());
  if (url.pathname === "/api/v1/admin/login") logins.push(request.method());
});

try {
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.evaluate(() => {
    for (const key of ["Authorization", "token", "userInfo", "rayrag_token"]) {
      localStorage.removeItem(key);
    }
    document.cookie = "rayrag_token=;path=/;max-age=0";
  });
  await page.goto(ORIGIN + "/admin", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='admin-login-submit']", { timeout: 20000 });

  // The three spotlight gradients, in upstream order.
  results.spotlights = await page.locator(".auth-spotlight").evaluateAll((nodes) =>
    nodes.map((node) => ({
      className: node.className,
      background: getComputedStyle(node).backgroundImage,
      blur: getComputedStyle(node).backdropFilter,
      zIndex: getComputedStyle(node).zIndex,
    })),
  );
  check(
    results.spotlights.length === 3 &&
      results.spotlights[0].background.includes("rgba(128, 255, 248, 0.4)") &&
      results.spotlights[0].background.includes("60%") &&
      results.spotlights[1].background.includes("rgba(128, 255, 248, 0.3)") &&
      results.spotlights[1].background.includes("10%") &&
      results.spotlights[2].background.includes("90%") &&
      results.spotlights.every((layer) => layer.blur === "blur(30px)" && layer.zIndex === "-1"),
    "spotlight_layers",
    JSON.stringify(results.spotlights).slice(0, 400),
  );

  results.geometry = {
    title: await styleOf(".admin-login-title", ["fontSize", "fontWeight", "marginTop", "marginBottom", "textAlign"]),
    column: await styleOf(".admin-login-column", ["maxWidth", "marginTop", "marginBottom"]),
    body: await styleOf(".admin-login-body", ["paddingTop", "paddingRight", "paddingBottom", "paddingLeft"]),
    footer: await styleOf(".admin-login-footer", ["paddingTop", "paddingRight", "paddingBottom", "paddingLeft"]),
    formGap: await styleOf(".admin-login-form", ["rowGap"]),
    emailHeight: await styleOf("#adminEmail", ["height"]),
  };
  check(
    results.geometry.title.fontSize === "36px" &&
      results.geometry.title.fontWeight === "500" &&
      results.geometry.title.marginTop === "104px" &&
      results.geometry.title.marginBottom === "48px" &&
      results.geometry.column.maxWidth === "540px" &&
      results.geometry.column.marginTop === "288px" &&
      results.geometry.column.marginBottom === "192px" &&
      results.geometry.body.paddingTop === "56px" &&
      results.geometry.body.paddingLeft === "40px" &&
      results.geometry.footer.paddingTop === "32px" &&
      results.geometry.footer.paddingBottom === "56px" &&
      results.geometry.formGap.rowGap === "32px" &&
      results.geometry.emailHeight.height === "40px",
    "card_geometry",
    JSON.stringify(results.geometry),
  );

  // Required labels carry the shared marker, and each field owns a message slot.
  results.labels = await page.locator(".admin-login-label").evaluateAll((nodes) =>
    nodes.map((node) => ({
      text: node.textContent.trim(),
      htmlFor: node.getAttribute("for"),
      marker: getComputedStyle(node, "::after").content,
    })),
  );
  check(
    results.labels.length === 2 &&
      results.labels[0].text === "Email" &&
      results.labels[0].htmlFor === "adminEmail" &&
      results.labels[1].text === "Password" &&
      results.labels[1].htmlFor === "adminPassword" &&
      results.labels.every((label) => label.marker.includes("*")),
    "required_labels",
    JSON.stringify(results.labels),
  );

  // Empty submit → one message per field, no request.
  logins.length = 0;
  await page.click("[data-testid='admin-login-submit']");
  await page.waitForTimeout(400);
  results.empty_submit = {
    email: await page.locator("[data-testid='admin-email-error']").innerText(),
    password: await page.locator("[data-testid='admin-password-error']").innerText(),
    dialogLevel: await page.locator("[data-testid='admin-login-error']").innerText(),
    requests: logins.length,
  };
  check(
    results.empty_submit.email.trim() === EMAIL_REQUIRED &&
      results.empty_submit.password.trim() === PASSWORD_REQUIRED &&
      results.empty_submit.dialogLevel.trim() === "" &&
      results.empty_submit.requests === 0,
    "empty_submit_blocks",
    JSON.stringify(results.empty_submit),
  );

  // A non-empty but malformed address gets zod's invalid-email wording; a valid
  // one clears the slot and the password message follows its own rule.
  await page.fill("#adminEmail", "not-an-email");
  await page.waitForTimeout(150);
  const invalidEmail = (await page.locator("[data-testid='admin-email-error']").innerText()).trim();
  await page.fill("#adminEmail", EMAIL);
  await page.fill("#adminPassword", "x");
  await page.waitForTimeout(150);
  results.live_validation = {
    invalidEmail,
    emailCleared: (await page.locator("[data-testid='admin-email-error']").innerText()).trim() === "",
    passwordCleared: (await page.locator("[data-testid='admin-password-error']").innerText()).trim() === "",
  };
  check(
    results.live_validation.invalidEmail === INVALID_EMAIL &&
      results.live_validation.emailCleared &&
      results.live_validation.passwordCleared,
    "live_validation",
    JSON.stringify(results.live_validation),
  );

  // The remember-me label tints with its checkbox.
  results.remember = {
    before: await styleOf(".admin-login-remember", ["color"]),
    checkedClassBefore: await page.locator(".admin-login-remember").evaluate((node) =>
      node.classList.contains("checked"),
    ),
  };
  await page.check("#adminRemember");
  await page.waitForTimeout(150);
  results.remember.after = await styleOf(".admin-login-remember", ["color"]);
  results.remember.checkedClassAfter = await page.locator(".admin-login-remember").evaluate((node) =>
    node.classList.contains("checked"),
  );
  results.remember.checkbox = await styleOf("#adminRemember", ["width", "height", "borderRadius", "appearance"]);
  check(
    results.remember.checkedClassBefore === false &&
      results.remember.checkedClassAfter === true &&
      results.remember.before.color !== results.remember.after.color &&
      results.remember.checkbox.width === "14px" &&
      results.remember.checkbox.height === "14px" &&
      results.remember.checkbox.borderRadius === "2px" &&
      results.remember.checkbox.appearance === "none",
    "remember_tint",
    JSON.stringify(results.remember),
  );

  // A rejected sign-in reports at dialog level only (upstream logs it), and the
  // per-field slots stay empty.
  logins.length = 0;
  await page.fill("#adminPassword", "definitely-wrong-password");
  await page.click("[data-testid='admin-login-submit']");
  await page.waitForTimeout(1500);
  results.bad_login = {
    dialogLevel: (await page.locator("[data-testid='admin-login-error']").innerText()).trim(),
    emailError: (await page.locator("[data-testid='admin-email-error']").innerText()).trim(),
    requests: logins.length,
    stillOnLogin: !page.url().includes("/services"),
  };
  check(
    results.bad_login.dialogLevel.length > 0 &&
      results.bad_login.emailError === "" &&
      results.bad_login.requests === 1 &&
      results.bad_login.stillOnLogin,
    "bad_login_reports",
    JSON.stringify(results.bad_login),
  );

  // The submit button is the highlighted variant from the footer.
  results.submit = await styleOf("[data-testid='admin-login-submit']", [
    "backgroundColor",
    "color",
    "borderBottomWidth",
    "borderBottomColor",
    "height",
    "borderRadius",
    "fontSize",
    "fontWeight",
    "width",
  ]);
  const footerBox = await styleOf(".admin-login-footer", ["width"]);
  check(
    results.submit.height === "40px" &&
      results.submit.borderRadius === "8px" &&
      results.submit.borderBottomWidth === "4px" &&
      results.submit.fontSize === "14px" &&
      results.submit.fontWeight === "500" &&
      parseFloat(results.submit.width) === parseFloat(footerBox.width) - 80,
    "highlighted_submit",
    JSON.stringify({ submit: results.submit, footer: footerBox }),
  );

  // A real sign-in still works and lands on the console.
  await page.fill("#adminPassword", PASSWORD);
  await page.click("[data-testid='admin-login-submit']");
  await page.waitForSelector("[data-testid='admin-console']", { timeout: 30000 });
  results.signed_in = page.url().replace(ORIGIN, "");
  check(results.signed_in === "/admin/services", "sign_in_lands", results.signed_in);

  check(pageErrors.length === 0, "no_page_errors", pageErrors.join(" | "));
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/admin-login-form-failure.png" }).catch(() => {});
} finally {
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
