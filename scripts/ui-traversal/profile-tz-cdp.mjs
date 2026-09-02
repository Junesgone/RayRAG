import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error(
    "RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)",
  );
  process.exit(2);
}

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1440, height: 900 });
const results = { steps: [], errors: [] };
try {
  await ctx.addCookies([
    {
      name: "lang",
      value: "zh-CN",
      domain: "127.0.0.1",
      path: "/",
      httpOnly: false,
    },
  ]);
  await page.goto(ORIGIN + "/login", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  const login = await page.evaluate(
    async ({ password }) => {
      const res = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email: "admin@rayrag.local", password }),
      });
      const data = await res.json();
      if (data.code !== 0 || !data.data || !data.data.access_token) {
        return { ok: false, body: data };
      }
      localStorage.setItem("rayrag_token", data.data.access_token);
      return { ok: true };
    },
    { password: PASSWORD },
  );
  results.steps.push("login:" + JSON.stringify(login));
  if (!login.ok) throw new Error("login failed");

  await page.goto(ORIGIN + "/user-setting/profile", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForTimeout(1500);
  const before = await page.evaluate(() => ({
    display: document.getElementById("profileTimezone").textContent,
    staleList:
      typeof Intl.supportedValuesOf === "function" ? "computed" : "n/a",
  }));
  results.before = before;
  mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });
  await page.screenshot({
    path: "docs/ui-traversal/rayrag-audit/profile-tz-before-zh-20260901.png",
  });

  // Simulated user click: Edit next to Time Zone.
  await page
    .locator("button[onclick*='timezone']")
    .first()
    .click({ timeout: 8000 });
  await page.waitForSelector("#profileEditDialog.show", { timeout: 5000 });
  await page.waitForSelector("#profileTimezoneFields:not([hidden])", {
    timeout: 5000,
  });
  const opened = await page.evaluate(() => ({
    title: document.getElementById("profileEditTitle").textContent,
    prefill: document.getElementById("profileEditTimezone").value,
    placeholder: document.getElementById("profileEditTimezone").placeholder,
    listCount: document.getElementById("profileTzList").children.length,
  }));
  results.opened = opened;

  // Search filter: type Tokyo, then click the matching option.
  await page.locator("#profileEditTimezone").fill("Tokyo");
  await page.waitForTimeout(300);
  const filtered = await page.evaluate(() =>
    Array.from(document.getElementById("profileTzList").children).map(
      (n) => n.textContent,
    ),
  );
  results.filtered = filtered;
  await page
    .locator("#profileTzList div", { hasText: "GMT+09:00 Asia/Tokyo" })
    .first()
    .click();
  const picked = await page.evaluate(
    () => document.getElementById("profileEditTimezone").value,
  );
  results.picked = picked;

  // Save and wait for the dialog to close + reload.
  await page.locator("#profileEditSave").click({ timeout: 8000 });
  await page.waitForSelector("#profileEditDialog.show", {
    state: "hidden",
    timeout: 8000,
  });
  await page.waitForTimeout(1500);
  const after = await page.evaluate(() => ({
    display: document.getElementById("profileTimezone").textContent,
  }));
  results.after = after;
  await page.screenshot({
    path: "docs/ui-traversal/rayrag-audit/profile-tz-after-zh-20260901.png",
  });

  const ok =
    opened.placeholder === "Select a timeZone" &&
    opened.listCount > 300 &&
    filtered.every((t) => t.includes("Tokyo")) &&
    picked === "GMT+09:00 Asia/Tokyo" &&
    after.display === "GMT+09:00 Asia/Tokyo";
  results.result = ok ? "PASS" : "FAIL";
} catch (e) {
  results.errors.push(String(e && e.message ? e.message : e));
  results.result = "FAIL";
}
console.log(JSON.stringify(results, null, 2));
await page.close();
await b.close();
process.exit(results.result === "PASS" ? 0 : 1);
