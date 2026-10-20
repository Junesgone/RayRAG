// CDP probe for the admin sandbox page's Test button
// (web/src/pages/admin/sandbox-settings.tsx): pick the E2B / Aliyun provider,
// fill its credentials, press Test and read the returned probe output. The
// executor endpoint is the deterministic mock (RAYRAG_SANDBOX_EXECUTOR_URL).
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
// The schema controls carry id/data-field = the config key (sbFieldControl).
const fillField = async (fieldId, value) => {
  const filled = await page.evaluate(
    ({ fieldId, value }) => {
      const input = document.querySelector(
        "[data-field='" + fieldId + "'], #" + fieldId,
      );
      if (!input) return false;
      input.value = value;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      input.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    },
    { fieldId, value },
  );
  return filled;
};
const selectProvider = async (providerId) => {
  await page.click("[data-testid='admin-sandbox-provider-" + providerId + "']");
  await page.waitForTimeout(400);
};
const runTest = async () => {
  await page.click("[data-testid='admin-sandbox-test']");
  await page.waitForSelector("[data-testid='admin-sandbox-test-message']", {
    timeout: 30000,
  });
  await page.waitForTimeout(300);
  return page.evaluate(() => ({
    summary: (document.getElementById("sbTestSummary") || {}).textContent || "",
    message: (document.querySelector("[data-testid='admin-sandbox-test-message']") || {})
      .textContent || "",
    stdout: (document.querySelector("[data-testid='admin-sandbox-test-stdout']") || {})
      .textContent || "",
  }));
};
try {
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const login = await page.evaluate(
    async ({ email, password }) => {
      const res = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email, password }),
      });
      const data = await res.json();
      if (data.code === 0 && data.data?.access_token) {
        localStorage.setItem("rayrag_token", data.data.access_token);
        document.cookie =
          "rayrag_token=" + data.data.access_token + ";path=/";
        return { ok: true, token: data.data.access_token };
      }
      return { ok: false };
    },
    { email: EMAIL, password: PASSWORD },
  );
  if (!login.ok) throw new Error("login failed");
  // Admin console pages authenticate server-side from the cookie.
  await ctx.addCookies([
    { name: "rayrag_token", value: login.token, domain: "127.0.0.1", path: "/" },
  ]);
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(ORIGIN + "/admin/sandbox-settings", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForSelector("[data-testid='admin-sandbox-providers']", {
    timeout: 20000,
  });

  await selectProvider("e2b");
  out.e2bFieldFilled = await fillField("api_key", "e2b_sk_test");
  out.e2b = await runTest();
  await page.screenshot({
    path: "docs/ui-traversal/rayrag-audit/sandbox-e2b-test-20261021.png",
    fullPage: true,
  });
  await page.evaluate(() => document.getElementById("sbTestModal").classList.remove("show"));

  await selectProvider("aliyun_codeinterpreter");
  const fields = [
    ["access_key_id", "ak"],
    ["access_key_secret", "sk"],
    ["account_id", "123"],
  ];
  out.aliyunFieldsFilled = [];
  for (const [label, value] of fields) {
    out.aliyunFieldsFilled.push(await fillField(label, value));
  }
  out.aliyun = await runTest();
  await page.screenshot({
    path: "docs/ui-traversal/rayrag-audit/sandbox-aliyun-test-20261021.png",
    fullPage: true,
  });
} catch (error) {
  out.error = String(error && error.message ? error.message : error).slice(0, 400);
  await page
    .screenshot({ path: "docs/ui-traversal/rayrag-audit/sandbox-executor-failure.png" })
    .catch(() => {});
} finally {
  out.pageErrors = pageErrors;
  console.log(JSON.stringify(out, null, 1));
  await page.close().catch(() => {});
  await b.close().catch(() => {});
  process.exit(out.error || pageErrors.length ? 1 : 0);
}
