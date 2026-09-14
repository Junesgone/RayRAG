// RAGFlow UI traversal helper: establish (or reuse) an authenticated session in
// the fnOS Chrome profile so later page dumps start from a logged-in state.
//
// Usage:
//   RAYRAG_CDP_ENDPOINT=http://127.0.0.1:16002 \
//   RAGFLOW_UI_ORIGIN=http://127.0.0.1:11526 \
//   RAGFLOW_UI_EMAIL=... RAGFLOW_UI_PASSWORD=... \
//   node scripts/ui-traversal/ragflow-auth-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAGFLOW_UI_ORIGIN || "http://127.0.0.1:11526";
const EMAIL = process.env.RAGFLOW_UI_EMAIL || "";
const PASSWORD = process.env.RAGFLOW_UI_PASSWORD || "";
const NICKNAME = process.env.RAGFLOW_UI_NICKNAME || "rayrag-audit";

if (!EMAIL || !PASSWORD) {
  console.error("RAGFLOW_UI_EMAIL and RAGFLOW_UI_PASSWORD are required");
  process.exit(2);
}

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
const out = { steps: [], pageErrors: [], authCalls: [] };
page.on("pageerror", (e) => out.pageErrors.push(String(e).slice(0, 240)));
page.on("response", async (response) => {
  const url = response.url();
  if (!/\/api\/v1\/(users|auth\/login)/.test(url)) return;
  let body = "";
  try {
    body = (await response.text()).slice(0, 300);
  } catch {}
  out.authCalls.push({ url, status: response.status(), body });
});

const state = async () =>
  page.evaluate(() => ({
    url: location.href,
    token: String(localStorage.getItem("Token") || "").slice(0, 24),
    lsKeys: Object.keys(localStorage).slice(0, 12),
  }));

try {
  await page.goto(ORIGIN + "/login", {
    waitUntil: "domcontentloaded",
    timeout: 45000,
  });
  await page.waitForTimeout(3000);
  out.steps.push({ after: "goto", state: await state() });

  // Already authenticated? The login page redirects/logs in when a token exists.
  const already = await page.evaluate(() => {
    const keys = Object.keys(localStorage);
    return keys.some((k) => {
      if (!/token/i.test(k)) return false;
      const value = localStorage.getItem(k);
      return !!value && value !== "undefined" && value !== "null";
    });
  });
  if (already) {
    out.steps.push({ after: "reuse-session", ok: true });
  } else {
    // The upstream login page renders a 3D flip card with both faces in the
    // DOM, so every query must be scoped to the active face and driven through
    // the DOM (Playwright actionability rejects the rotated face).
    const fill = async (testid, value) => {
      return page.evaluate(
        ({ testid, value }) => {
          const form = document.querySelector(
            "[data-testid='auth-form'][data-active='true']",
          );
          const root = form || document;
          const input = root.querySelector(`[data-testid='${testid}']`);
          if (!input) return false;
          const proto =
            input instanceof HTMLTextAreaElement
              ? HTMLTextAreaElement.prototype
              : HTMLInputElement.prototype;
          const setter = Object.getOwnPropertyDescriptor(proto, "value").set;
          setter.call(input, value);
          input.dispatchEvent(new Event("input", { bubbles: true }));
          input.dispatchEvent(new Event("change", { bubbles: true }));
          return true;
        },
        { testid, value },
      );
    };
    const submit = async () => {
      return page.evaluate(() => {
        const form = document.querySelector(
          "[data-testid='auth-form'][data-active='true']",
        );
        if (!form) return false;
        const button = form.querySelector("[data-testid='auth-submit']");
        if (button) {
          button.click();
          return true;
        }
        if (form.requestSubmit) {
          form.requestSubmit();
          return true;
        }
        return false;
      });
    };
    const toggleRegister = async () =>
      page.evaluate(() => {
        const button = document.querySelector(
          "[data-testid='auth-toggle-register']",
        );
        if (!button) return false;
        button.click();
        return true;
      });

    // Attempt a plain login first.
    out.filledEmail = await fill("auth-email", EMAIL);
    out.filledPassword = await fill("auth-password", PASSWORD);
    out.submitted = await submit();
    await page.waitForTimeout(5000);
    let after = await state();
    out.steps.push({ after: "login-submit", state: after });

    // Not registered yet → switch the flip card to the register face.
    if (/\/login/.test(after.url)) {
      const toggled = await toggleRegister();
      if (toggled) {
        await page.waitForTimeout(1500);
        await fill("auth-nickname", NICKNAME);
        await fill("auth-email", EMAIL);
        await fill("auth-password", PASSWORD);
        await submit();
        await page.waitForTimeout(6000);
        after = await state();
        out.steps.push({ after: "register-submit", state: after });
        if (/\/login/.test(after.url)) {
          // Registration may have succeeded but not logged in: log in again.
          await page.goto(ORIGIN + "/login", {
            waitUntil: "domcontentloaded",
            timeout: 45000,
          });
          await page.waitForTimeout(2500);
          await fill("auth-email", EMAIL);
          await fill("auth-password", PASSWORD);
          await submit();
          await page.waitForTimeout(4000);
          after = await state();
          out.steps.push({ after: "login-retry", state: after });
        }
      } else {
        out.steps.push({ registerToggleFound: false });
      }
    }

    // Report any inline form error text so the caller can react.
    out.formMessages = await page.evaluate(() =>
      Array.from(
        document.querySelectorAll(
          "[data-slot='form-message'], .text-destructive, p",
        ),
      )
        .map((el) => (el.textContent || "").trim())
        .filter((t) => t && t.length < 120)
        .slice(0, 12),
    );
  }
} catch (error) {
  out.error = String(error && error.message ? error.message : error).slice(
    0,
    400,
  );
}

out.finalState = await state().catch(() => null);
console.log(JSON.stringify(out, null, 1));
await page.close().catch(() => {});
process.exit(out.error ? 1 : 0);
