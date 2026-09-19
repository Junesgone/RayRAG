// CDP probe for the `/user-setting/model` page shell
// (upstream `pages/user-setting/setting-model/index.tsx`): the bordered rounded
// card, its 3/5 + 2/5 columns, the left column's right border and the single
// `<Spotlight />` gradient behind the content. Real login + navigation only;
// nothing is saved.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/model-page-layout-cdp.mjs
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
await page.setViewportSize({ width: 1600, height: 1000 });
const results = { errors: [], pageErrors };
mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });

const measure = (lang) =>
  page.evaluate((lang) => {
    const shell = document.querySelector("[data-testid='model-providers-page']");
    const left = document.querySelector(".model-providers-left");
    const right = document.querySelector(".model-providers-right");
    const spot = document.querySelector(".model-providers-spotlight");
    const card = getComputedStyle(shell);
    return {
      lang,
      shellBorder: card.borderTopWidth,
      shellRadius: card.borderTopLeftRadius,
      shellDisplay: card.display,
      leftShare: Math.round((left.getBoundingClientRect().width / shell.getBoundingClientRect().width) * 100),
      rightShare: Math.round((right.getBoundingClientRect().width / shell.getBoundingClientRect().width) * 100),
      leftBorderRight: getComputedStyle(left).borderRightWidth,
      leftGap: getComputedStyle(left).gap,
      leftPadding: getComputedStyle(left).paddingLeft,
      leftOverflow: getComputedStyle(left).overflowY,
      rightOverflow: getComputedStyle(right).overflowY,
      spotlight: spot
        ? {
            opacity: getComputedStyle(spot).opacity,
            coverage: getComputedStyle(spot).getPropertyValue("--sc").trim(),
            sy: getComputedStyle(spot).getPropertyValue("--sy").trim(),
            rgb: getComputedStyle(spot).getPropertyValue("--srgb").trim(),
            zIndex: getComputedStyle(spot).zIndex,
          }
        : null,
      order: ["system-model-settings", "added-models-section", "available-models-section"].map((id) =>
        Array.from(document.querySelectorAll(`[data-testid='${id}']`)).length
          ? document
              .querySelector(`[data-testid='${id}']`)
              .closest("section")
              .className.replace("model-providers-column ", "")
          : null,
      ),
    };
  }, lang);

try {
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const login = await page.evaluate(async (password) => {
    const res = await fetch("/api/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "admin@rayrag.local", password }),
    });
    const data = await res.json();
    if (data.code !== 0) return { ok: false };
    localStorage.setItem("rayrag_token", data.data.access_token);
    return { ok: true };
  }, PASSWORD);
  if (!login.ok) throw new Error("login failed");
  await page.goto(`${ORIGIN}/user-setting/model`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='model-providers-page']", { timeout: 20000 });
  results.en = await measure("en");
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/model-page-layout-20260821.png" });
  await ctx.addCookies([{ name: "lang", value: "zh-CN", domain: "127.0.0.1", path: "/" }]);
  await page.reload({ waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("[data-testid='model-providers-page']", { timeout: 20000 });
  results.zh = await measure("zh");
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/model-page-layout-zh-20260821.png" });
} catch (error) {
  results.errors.push(String(error && error.stack ? error.stack : error));
} finally {
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
}
if (results.errors.length || results.pageErrors.length) process.exit(1);
