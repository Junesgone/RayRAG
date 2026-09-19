// CDP probe for the admin user forms
// (`pages/admin/forms/user-form.tsx` + `forms/change-password-form.tsx`): both
// forms validate per field through react-hook-form + zod and render one
// `FormMessage` under each invalid field, so a submit with invalid input must
// produce the upstream copy inline and must NOT reach the API. The probe only
// types into the dialogs and cancels them — it never creates an account and
// never changes a password, so the tenant state is untouched.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/admin-user-forms-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}

const EMAIL_INVALID = "Please input a valid email address!";
const PASSWORD_MIN = "Password must be more than 8 characters.";
const CONFIRM_REQUIRED = "Please confirm your password!";
const CONFIRM_MISMATCH = "The password that you entered do not match!";

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

// Every write against the admin user API is counted so the probe can prove that
// an invalid submit is blocked before the request leaves the page.
const writes = [];
page.on("request", (request) => {
  const url = new URL(request.url());
  if (
    url.pathname.startsWith("/api/v1/admin/users") &&
    ["POST", "PUT", "DELETE"].includes(request.method())
  ) {
    writes.push(`${request.method()} ${url.pathname}`);
  }
});

try {
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(ORIGIN + "/admin", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.fill("#adminEmail", "admin@rayrag.local");
  await page.fill("#adminPassword", PASSWORD);
  await page.click("[data-testid='admin-login-submit']");
  await page.waitForURL("**/admin/services", { timeout: 20000 });

  // --- Create-user form ---------------------------------------------------
  await page.click("a[href='/admin/users']");
  await page.waitForURL("**/admin/users", { timeout: 15000 });
  await page.waitForSelector("[data-testid='admin-user-row']", { timeout: 20000 });
  const rowsBefore = await page.locator("[data-testid='admin-user-row']").count();

  await page.click("[data-testid='admin-new-user']");
  await page.waitForSelector("#adminCreateUserDialog.show", { timeout: 10000 });
  results.create_fields = {
    labels: await page
      .locator("#adminCreateUserDialog label.admin-field > span:first-child")
      .allInnerTexts(),
    placeholders: {
      email: await page.locator("[data-testid='admin-create-user-email']").getAttribute("placeholder"),
      password: await page
        .locator("[data-testid='admin-create-user-password']")
        .getAttribute("placeholder"),
      confirm: await page
        .locator("[data-testid='admin-create-user-confirm']")
        .getAttribute("placeholder"),
    },
    emailType: await page.locator("[data-testid='admin-create-user-email']").getAttribute("type"),
    passwordTypes: [
      await page.locator("[data-testid='admin-create-user-password']").getAttribute("type"),
      await page.locator("[data-testid='admin-create-user-confirm']").getAttribute("type"),
    ],
  };
  check(
    JSON.stringify(results.create_fields.labels) ===
      JSON.stringify(["Email", "Password", "Confirm password", "Role"]),
    "create_fields",
    JSON.stringify(results.create_fields.labels),
  );
  // A fresh form never starts out red: the message slots collapse (`FormMessage`
  // returns null when the field has no error).
  for (const slot of ["email", "password", "confirm"]) {
    const shown = await page
      .locator(`[data-testid='admin-create-user-${slot}-error']`)
      .evaluate((node) => getComputedStyle(node).display !== "none");
    check(!shown, `create_${slot}_error_collapsed`);
  }

  // Empty submit → three inline messages, no request.
  await page.click("[data-testid='admin-create-user-submit']");
  await page.waitForTimeout(400);
  results.create_empty_submit = {
    email: await text("[data-testid='admin-create-user-email-error']"),
    password: await text("[data-testid='admin-create-user-password-error']"),
    confirm: await text("[data-testid='admin-create-user-confirm-error']"),
    dialogLevel: await text("#adminCreateUserError"),
    requests: writes.length,
  };
  check(results.create_empty_submit.email === EMAIL_INVALID, "create_empty_email", results.create_empty_submit.email);
  check(
    results.create_empty_submit.password === PASSWORD_MIN,
    "create_empty_password",
    results.create_empty_submit.password,
  );
  check(
    results.create_empty_submit.confirm === CONFIRM_REQUIRED,
    "create_empty_confirm",
    results.create_empty_submit.confirm,
  );
  check(results.create_empty_submit.requests === 0, "create_empty_no_request", String(writes.length));
  check(results.create_empty_submit.dialogLevel === "", "create_empty_dialog_level_silent");

  // A malformed address keeps the e-mail message, a valid one clears it.
  await page.fill("[data-testid='admin-create-user-email']", "not-an-email");
  await page.waitForTimeout(150);
  const badEmail = await text("[data-testid='admin-create-user-email-error']");
  await page.fill("[data-testid='admin-create-user-email']", "probe@example.com");
  await page.waitForTimeout(150);
  const goodEmailShown = await page
    .locator("[data-testid='admin-create-user-email-error']")
    .evaluate((node) => getComputedStyle(node).display !== "none");
  check(badEmail === EMAIL_INVALID && !goodEmailShown, "create_email_live_validation", badEmail);

  // Five characters is still too short (upstream `min(6)`), six clears it.
  await page.fill("[data-testid='admin-create-user-password']", "abcde");
  await page.waitForTimeout(150);
  const shortPassword = await text("[data-testid='admin-create-user-password-error']");
  await page.fill("[data-testid='admin-create-user-password']", "abcdef");
  await page.waitForTimeout(150);
  const passwordShown = await page
    .locator("[data-testid='admin-create-user-password-error']")
    .evaluate((node) => getComputedStyle(node).display !== "none");
  check(shortPassword === PASSWORD_MIN && !passwordShown, "create_password_min_six", shortPassword);

  // The confirmation has to match, and once it does the dialog submits.
  await page.fill("[data-testid='admin-create-user-confirm']", "abcdeg");
  await page.waitForTimeout(150);
  const mismatch = await text("[data-testid='admin-create-user-confirm-error']");
  check(mismatch === CONFIRM_MISMATCH, "create_confirm_mismatch", mismatch);
  await page.fill("[data-testid='admin-create-user-confirm']", "abcdef");
  await page.waitForTimeout(150);
  const confirmShown = await page
    .locator("[data-testid='admin-create-user-confirm-error']")
    .evaluate((node) => getComputedStyle(node).display !== "none");
  check(!confirmShown, "create_confirm_match_clears");

  // Cancel without submitting: the list must be untouched.
  await page.click("#adminCreateUserDialog .admin-dialog-actions .btn-o");
  await page.waitForTimeout(400);
  const rowsAfterCancel = await page.locator("[data-testid='admin-user-row']").count();
  check(
    rowsAfterCancel === rowsBefore && writes.length === 0,
    "create_cancel_leaves_state",
    `${rowsAfterCancel}/${rowsBefore} rows, writes=${writes.length}`,
  );

  // Reopening resets both the values and the messages.
  await page.click("[data-testid='admin-new-user']");
  await page.waitForSelector("#adminCreateUserDialog.show", { timeout: 10000 });
  results.create_reopen = {
    email: await page.locator("[data-testid='admin-create-user-email']").inputValue(),
    confirm: await page.locator("[data-testid='admin-create-user-confirm']").inputValue(),
    emailError: await text("[data-testid='admin-create-user-email-error']"),
  };
  check(
    results.create_reopen.email === "" &&
      results.create_reopen.confirm === "" &&
      results.create_reopen.emailError === "",
    "create_reopen_resets",
    JSON.stringify(results.create_reopen),
  );
  await page.click("#adminCreateUserDialog .admin-dialog-actions .btn-o");
  await page.waitForTimeout(300);

  // --- Change-password form ----------------------------------------------
  const targetRow = page.locator("[data-testid='admin-user-row']").first();
  const targetRowText = (await targetRow.innerText()).replace(/\s+/g, " ").trim();
  await targetRow.getByRole("button", { name: "Change password" }).click();
  await page.waitForSelector("#adminPasswordDialog.show", { timeout: 10000 });
  results.password_fields = {
    labels: await page
      .locator("#adminPasswordDialog label.admin-field > span:first-child")
      .allInnerTexts(),
    email: await page.locator("[data-testid='admin-password-email']").inputValue(),
    emailReadonly: await page.locator("[data-testid='admin-password-email']").getAttribute("readonly"),
    emailBackground: await page
      .locator("[data-testid='admin-password-email']")
      .evaluate((node) => getComputedStyle(node).backgroundColor),
    newPasswordType: await page.locator("[data-testid='admin-new-password']").getAttribute("type"),
    placeholders: {
      new: await page.locator("[data-testid='admin-new-password']").getAttribute("placeholder"),
      confirm: await page
        .locator("[data-testid='admin-new-password-confirm']")
        .getAttribute("placeholder"),
    },
  };
  check(
    JSON.stringify(results.password_fields.labels) ===
      JSON.stringify(["Email", "New password", "Confirm new password"]),
    "password_fields",
    JSON.stringify(results.password_fields.labels),
  );
  check(
    results.password_fields.email !== "" &&
      targetRowText.includes(results.password_fields.email) &&
      results.password_fields.emailReadonly !== null,
    "password_readonly_email",
    `${results.password_fields.email} vs row ${targetRowText}`,
  );
  for (const slot of ["new-password", "new-password-confirm"]) {
    const shown = await page
      .locator(`[data-testid='admin-${slot}-error']`)
      .evaluate((node) => getComputedStyle(node).display !== "none");
    check(!shown, `password_${slot}_error_collapsed`);
  }

  // Seven characters is short (`min(8)`), eight clears it.
  await page.fill("[data-testid='admin-new-password']", "abcdefg");
  await page.waitForTimeout(150);
  const shortNew = await text("[data-testid='admin-new-password-error']");
  await page.fill("[data-testid='admin-new-password']", "abcdefgh");
  await page.waitForTimeout(150);
  const newShown = await page
    .locator("[data-testid='admin-new-password-error']")
    .evaluate((node) => getComputedStyle(node).display !== "none");
  check(shortNew === PASSWORD_MIN && !newShown, "password_min_eight", shortNew);

  // An empty confirmation asks for it, a different one reports the mismatch.
  await page.fill("[data-testid='admin-new-password-confirm']", "abcdefgi");
  await page.waitForTimeout(150);
  const confirmMismatch = await text("[data-testid='admin-new-password-confirm-error']");
  check(confirmMismatch === CONFIRM_MISMATCH, "password_confirm_mismatch", confirmMismatch);
  await page.fill("[data-testid='admin-new-password-confirm']", "abcdefgh");
  await page.waitForTimeout(150);
  const confirmOk = await page
    .locator("[data-testid='admin-new-password-confirm-error']")
    .evaluate((node) => getComputedStyle(node).display !== "none");
  check(!confirmOk, "password_confirm_match_clears");

  // Nothing here may be submitted — the deployed operator password must survive.
  await page.click("#adminPasswordDialog .admin-dialog-actions .btn-o");
  await page.waitForTimeout(400);
  const stillOpen = await page.locator("#adminPasswordDialog").evaluate((node) =>
    node.classList.contains("show"),
  );
  check(writes.length === 0 && !stillOpen, "password_cancel_no_write", `writes=${writes.length}`);
  results.writes = writes;
  results.pageErrors = pageErrors;
  check(pageErrors.length === 0, "no_page_errors", pageErrors.join(" | "));
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/admin-user-forms-failure.png" }).catch(() => {});
} finally {
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
