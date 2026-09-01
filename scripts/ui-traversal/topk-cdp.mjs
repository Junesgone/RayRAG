import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const KB = process.env.RAYRAG_KB_ID;
const APP = process.env.RAYRAG_APP_ID;
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD || !KB || !APP) {
  console.error("RAYRAG_LOGIN_PASSWORD, RAYRAG_KB_ID and RAYRAG_APP_ID are required");
  process.exit(2);
}

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1440, height: 900 });
const results = { steps: [], errors: [] };
try {
  await ctx.addCookies([
    { name: "lang", value: "zh-CN", domain: "127.0.0.1", path: "/", httpOnly: false },
  ]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const login = await page.evaluate(async ({ email, password }) => {
    const res = await fetch("/api/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email, password }),
    });
    const data = await res.json();
    if (data.code !== 0 || !data.data || !data.data.access_token) {
      return { ok: false, body: data };
    }
    localStorage.setItem("rayrag_token", data.data.access_token);
    return { ok: true };
  }, { email: "admin@rayrag.local", password: PASSWORD });
  results.steps.push("login:" + JSON.stringify(login));
  if (!login.ok) throw new Error("login failed");

  // skills page: click the config trigger, verify zh skillSearch.topK label.
  await page.goto(ORIGIN + "/skills", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(1200);
  const trigger = page.locator("button[onclick*='openSkillConfig']").first();
  await trigger.click({ timeout: 8000 });
  await page.waitForSelector("#skillConfigModal.show", { timeout: 5000 });
  const modalText = await page.locator("#skillConfigModal").textContent();
  results.skillModal = {
    hasZhResults: modalText.includes("返回结果数量"),
    hasStaleTopK: /\bTop K\b/.test(modalText.replace("Top K Results", "")),
  };
  mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/skills-topk-zh-20260901.png" });
  await page.locator("#skillConfigModal .btn-o").first().click({ timeout: 8000 });
  await page.waitForSelector("#skillConfigModal.show", { state: "hidden", timeout: 5000 });
  results.steps.push("skillModal:open/close ok");

  // kb detail (zh): retrieval config + testing labels render Top-K.
  await page.goto(ORIGIN + "/kbs/" + KB, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(1000);
  results.kbZh = {
    hasTopK: (await page.locator("body").textContent()).includes("Top-K"),
    hasStale: (await page.locator("body").textContent()).includes("召回条数"),
  };
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/kb-topk-zh-20260901.png" });

  // search app detail (zh): basic settings renders Top-K.
  await page.goto(ORIGIN + "/searches/" + APP, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(1000);
  results.appZh = {
    hasTopK: (await page.locator("body").textContent()).includes("Top-K"),
    hasStale: (await page.locator("body").textContent()).includes("召回条数"),
  };

  // search page (zh): sidebar stays literal Top K.
  await page.goto(ORIGIN + "/search", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(1000);
  results.searchZh = {
    hasLiteralTopK: (await page.locator("body").textContent()).includes("Top K"),
    hasStale: (await page.locator("body").textContent()).includes("召回条数"),
  };
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/search-topk-zh-20260901.png" });

  const ok = results.skillModal?.hasZhResults
    && !results.skillModal?.hasStaleTopK
    && results.kbZh?.hasTopK && !results.kbZh?.hasStale
    && results.appZh?.hasTopK && !results.appZh?.hasStale
    && results.searchZh?.hasLiteralTopK && !results.searchZh?.hasStale;
  results.result = ok ? "PASS" : "FAIL";
} catch (e) {
  results.errors.push(String(e && e.message ? e.message : e));
  results.result = "FAIL";
}
console.log(JSON.stringify(results, null, 2));
await page.close();
await b.close();
process.exit(results.result === "PASS" ? 0 : 1);
