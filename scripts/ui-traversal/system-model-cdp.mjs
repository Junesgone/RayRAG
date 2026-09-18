// CDP probe for the SystemSetting section of `/user-setting/model`
// (upstream `web/src/pages/user-setting/setting-model/components/system-setting.tsx`).
//
// It drives a real Chrome through the DevTools Protocol, logs in, opens the
// page in both languages and verifies, by measuring the live DOM:
//   * the `article > header.py-5` composition and the bordered, internally
//     scrollable `max-h-[70vh]` rows container;
//   * the six `flex gap-3` rows with a 1/4 label column and a 3/4 control column;
//   * the upstream `setting.selectModelPlaceholder` text on the trigger;
//   * the lucide `CircleQuestionMark` trigger and its hover popover (the
//     ::after box that CSS only generates while the pointer is over the icon);
//   * a real click that opens the model tree and Escape that closes it.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9494 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/system-model-cdp.mjs
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
const shots = "docs/ui-traversal/rayrag-audit";
mkdirSync(shots, { recursive: true });

try {
  await ctx.addCookies([
    { name: "lang", value: "zh-CN", domain: "127.0.0.1", path: "/" },
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

  const measure = async (lang) => {
    await ctx.addCookies([
      { name: "lang", value: lang === "zh" ? "zh-CN" : "en", domain: "127.0.0.1", path: "/" },
    ]);
    await page.goto(ORIGIN + "/user-setting/model", {
      waitUntil: "domcontentloaded",
      timeout: 40000,
    });
    await page.waitForSelector("[data-testid='system-model-settings']", {
      timeout: 10000,
    });
    return page.evaluate(() => {
      const section = document.querySelector(
        "[data-testid='system-model-settings']",
      );
      const header = section.querySelector("header");
      const heading = header.querySelector("h2");
      const desc = header.querySelector("p");
      const rows = Array.from(
        section.querySelectorAll(".provider-default-field"),
      );
      const grid = section.querySelector(".provider-default-grid");
      const gridStyle = getComputedStyle(grid);
      const firstRow = rows[0];
      const label = firstRow.querySelector("label");
      const control = firstRow.querySelector(".default-model-control");
      const tip = label.querySelector(".tip-icon");
      const svg = tip ? tip.querySelector("svg") : null;
      const placeholder = firstRow.querySelector(
        ".default-model-placeholder",
      );
      const required = label.querySelector("span[style*='--d']");
      return {
        heading: heading.textContent,
        headingSize: getComputedStyle(heading).fontSize,
        headingWeight: getComputedStyle(heading).fontWeight,
        description: desc.textContent,
        descriptionSize: getComputedStyle(desc).fontSize,
        rowCount: rows.length,
        rowDisplay: getComputedStyle(firstRow).display,
        rowGap: getComputedStyle(firstRow).columnGap,
        gridDisplay: gridStyle.display,
        gridDirection: gridStyle.flexDirection,
        gridGap: gridStyle.rowGap,
        gridMaxHeight: gridStyle.maxHeight,
        gridOverflowY: gridStyle.overflowY,
        gridBorder: gridStyle.borderTopWidth + " " + gridStyle.borderTopStyle,
        gridPadding: gridStyle.paddingTop + " " + gridStyle.paddingLeft,
        labelWidthRatio:
          Math.round((label.getBoundingClientRect().width / control.parentElement.getBoundingClientRect().width) * 1000) / 1000,
        controlWidthRatio:
          Math.round((control.getBoundingClientRect().width / control.parentElement.getBoundingClientRect().width) * 1000) / 1000,
        requiredMarker: required ? required.textContent : null,
        placeholder: placeholder ? placeholder.textContent : null,
        tipCount: section.querySelectorAll(".tip-icon").length,
        legacyTipCount: section.querySelectorAll(".tip").length,
        tipSvg: svg
          ? {
              viewBox: svg.getAttribute("viewBox"),
              width: getComputedStyle(svg).width,
              stroke: getComputedStyle(svg).strokeWidth,
              circle: !!svg.querySelector("circle"),
              paths: svg.querySelectorAll("path").length,
            }
          : null,
        tipDataTip: tip ? tip.getAttribute("data-tip") : null,
        tipAria: tip ? tip.getAttribute("aria-label") : null,
      };
    });
  };

  results.zh = await measure("zh");
  await page.screenshot({ path: `${shots}/system-model-zh-20260918.png` });
  results.en = await measure("en");
  await page.screenshot({ path: `${shots}/system-model-en-20260918.png` });

  // Hover popover: the CSS `::after` box only exists while :hover matches.
  const hoverIcon = page.locator(".provider-default-field .tip-icon").first();
  const beforeHover = await page.evaluate(() => {
    const tip = document.querySelector(".provider-default-field .tip-icon");
    const after = getComputedStyle(tip, "::after");
    return { content: after.content, width: after.width, top: after.top };
  });
  await hoverIcon.hover({ timeout: 8000 });
  await page.waitForTimeout(400);
  const afterHover = await page.evaluate(() => {
    const tip = document.querySelector(".provider-default-field .tip-icon");
    const after = getComputedStyle(tip, "::after");
    return {
      content: after.content,
      width: after.width,
      zIndex: after.zIndex,
      background: after.backgroundColor,
    };
  });
  results.hover = { beforeHover, afterHover };
  await page.screenshot({ path: `${shots}/system-model-tooltip-zh-20260918.png` });

  // Real click: open the LLM tree, then Escape.
  await page
    .locator("[data-testid='default-model-llm_id']")
    .click({ timeout: 8000 });
  await page.waitForTimeout(600);
  results.opened = await page.evaluate(() => {
    const trigger = document.querySelector(
      "[data-testid='default-model-llm_id']",
    );
    const popover = document.getElementById("dmLlmPopover");
    return {
      expanded: trigger.getAttribute("aria-expanded"),
      state: trigger.getAttribute("data-state"),
      popoverHidden: popover ? popover.hidden : null,
      treeRole: popover
        ? popover.querySelector(".default-model-tree").getAttribute("role")
        : null,
      searchPlaceholder: popover
        ? popover.querySelector("input").getAttribute("placeholder")
        : null,
    };
  });
  await page.screenshot({ path: `${shots}/system-model-tree-zh-20260918.png` });
  await page.keyboard.press("Escape");
  await page.waitForTimeout(400);
  results.closed = await page.evaluate(() => {
    const trigger = document.querySelector(
      "[data-testid='default-model-llm_id']",
    );
    const popover = document.getElementById("dmLlmPopover");
    return {
      expanded: trigger.getAttribute("aria-expanded"),
      popoverHidden: popover ? popover.hidden : null,
    };
  });

  // All six rows expose the same shape. Rows that already hold a default model
  // show the selected value instead of the placeholder, so `placeholder` is
  // nullable here.
  results.rows = await page.evaluate(() =>
    Array.from(document.querySelectorAll(".provider-default-field")).map(
      (row) => {
        const ph = row.querySelector(".default-model-placeholder");
        return {
          domId: row.getAttribute("data-default-model-root"),
          label: row
            .querySelector("label")
            .textContent.trim()
            .replace(/\s+/g, " "),
          placeholder: ph ? ph.textContent : null,
          value: row.querySelector("[data-testid]").dataset.value || "",
          hasTip: !!row.querySelector(".tip-icon"),
          required: !!row.querySelector("label span[style*='--d']"),
          testId: row.querySelector("[data-testid]").getAttribute("data-testid"),
        };
      },
    ),
  );
  results.placeholderRows = results.rows.filter((row) => row.placeholder);
} catch (error) {
  results.errors.push(String(error && error.stack ? error.stack : error));
} finally {
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
}
if (results.errors.length) process.exit(1);
