// CDP probe for the shared default-model tree's keyboard behaviour
// (`components/tree-select.tsx` on Radix `Tree`, used by
// `components/model-tree-select.tsx`): Arrow Down/Up walk the visible rows, Arrow
// Right expands a collapsed branch and enters an expanded one, Arrow Left
// collapses it again and returns to the parent, a one-second typeahead buffer
// cycles through matches, and the rows keep a roving `tabindex` so the whole tree
// is a single tab stop. Only real key presses are used; nothing is saved.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/model-tree-keyboard-cdp.mjs
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
await page.setViewportSize({ width: 1500, height: 1000 });
const results = { errors: [], pageErrors };
mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });
const check = (ok, label, detail) => {
  results[label] = ok ? "ok" : "FAIL";
  if (!ok) results.errors.push(label + (detail ? ` (${detail})` : ""));
};
const focused = () =>
  page.evaluate(() => {
    const node = document.activeElement;
    return node && node.classList && node.classList.contains("default-model-node")
      ? {
          title: (node.querySelector(".default-model-node-title") || node).textContent.trim(),
          expanded: node.getAttribute("aria-expanded"),
          level: node.getAttribute("aria-level"),
          tabindex: node.getAttribute("tabindex"),
        }
      : null;
  });
const treeState = () =>
  page.evaluate(() => {
    const tree = document.querySelector(".default-model-tree");
    if (!tree) return null;
    const rows = Array.from(tree.querySelectorAll("button.default-model-node"));
    return {
      rows: rows.length,
      tabbable: rows.filter((row) => row.getAttribute("tabindex") === "0").length,
      branches: rows.filter((row) => row.getAttribute("aria-expanded") !== null).length,
      levels: Array.from(new Set(rows.map((row) => row.getAttribute("aria-level")))).sort(),
    };
  });

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
    if (data.code !== 0) return { ok: false, body: data };
    localStorage.setItem("rayrag_token", data.data.access_token);
    document.cookie = "rayrag_token=" + data.data.access_token + ";path=/";
    return { ok: true };
  }, PASSWORD);
  if (!login.ok) throw new Error("login failed: " + JSON.stringify(login.body));

  // Open the first default-model picker on the Model providers page.
  await page.goto(`${ORIGIN}/user-setting/model`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("button.default-model-trigger", { timeout: 20000 });
  await page.locator("button.default-model-trigger").first().click();
  await page.waitForSelector(".default-model-tree button.default-model-node", { timeout: 20000 });

  const state = await treeState();
  check(Boolean(state) && state.rows > 1, "tree_renders_rows", JSON.stringify(state));
  check(state.tabbable === 1, "single_tab_stop", String(state.tabbable));
  check(state.levels.includes("1"), "aria_level_present", state.levels.join(","));

  // Arrow Down walks forward and wraps; Arrow Up walks back.
  await page.locator(".default-model-tree button.default-model-node").first().focus();
  const first = await focused();
  check(first && first.tabindex === "0", "focused_row_is_tabbable", JSON.stringify(first));
  await page.keyboard.press("ArrowDown");
  const second = await focused();
  check(second && second.title !== first.title, "arrow_down_moves_focus", JSON.stringify(second));
  await page.keyboard.press("ArrowUp");
  const back = await focused();
  check(back && back.title === first.title, "arrow_up_returns", JSON.stringify(back));
  await page.keyboard.press("End");
  const last = await focused();
  check(last && last.title !== first.title, "end_moves_to_last_visible_row", JSON.stringify(last));
  await page.keyboard.press("Home");
  const home = await focused();
  check(
    home && home.title === first.title,
    "home_returns_to_first_row",
    `${JSON.stringify(home)} vs ${JSON.stringify(first)}`,
  );

  // Branches start expanded, so collapse one first and then re-expand it with
  // the arrow keys (Radix: Left collapses / returns to the parent, Right expands
  // / enters the first child).
  const branchTitle = await page.evaluate(() => {
    const row = document.querySelector(".default-model-tree button.default-model-node[aria-expanded='true']");
    if (!row) return null;
    row.focus();
    return (row.querySelector(".default-model-node-title") || row).textContent.trim();
  });
  check(Boolean(branchTitle), "expanded_branch_found", String(branchTitle));
  if (branchTitle) {
    const expandedRows = (await treeState()).rows;
    await page.keyboard.press("ArrowLeft");
    await page.waitForTimeout(200);
    const collapsedRows = (await treeState()).rows;
    check(
      collapsedRows < expandedRows,
      "arrow_left_collapses_an_expanded_branch",
      `${expandedRows} -> ${collapsedRows}`,
    );
    await page.keyboard.press("ArrowRight");
    await page.waitForTimeout(200);
    const reExpanded = await treeState();
    check(
      reExpanded.rows === expandedRows,
      "arrow_right_expands_a_collapsed_branch",
      `${collapsedRows} -> ${reExpanded.rows}`,
    );
    const onBranch = await focused();
    check(
      onBranch && onBranch.title === branchTitle && onBranch.expanded === "true",
      "focus_stays_on_the_expanded_branch",
      JSON.stringify(onBranch),
    );
    await page.keyboard.press("ArrowRight");
    await page.waitForTimeout(150);
    const child = await focused();
    check(
      child && Number(child.level) === 2 && child.title !== branchTitle,
      "second_arrow_right_enters_the_first_child",
      JSON.stringify(child),
    );
    // Radix collapses an expanded item before leaving it, so a branch child
    // needs one extra Arrow Left to reach the parent.
    await page.keyboard.press("ArrowLeft");
    await page.waitForTimeout(150);
    const collapsedChild = await focused();
    if (collapsedChild && collapsedChild.expanded === "true") {
      await page.keyboard.press("ArrowLeft");
      await page.waitForTimeout(150);
    }
    const parent = await focused();
    check(
      parent && Number(parent.level) === 1 && parent.title === branchTitle,
      "arrow_left_returns_to_the_parent",
      `${JSON.stringify(parent)} after collapsing ${JSON.stringify(collapsedChild)}`,
    );
  }

  // Typeahead: typing a prefix focuses the first matching row, and the buffer
  // survives rapid keystrokes.
  const prefix = await page.evaluate(() => {
    const row = document.querySelector(".default-model-tree button.default-model-node .default-model-node-title");
    return row ? row.textContent.trim().charAt(0).toLowerCase() : "";
  });
  await page.locator(".default-model-tree button.default-model-node").last().focus();
  await page.keyboard.press(prefix);
  await page.waitForTimeout(120);
  const matched = await focused();
  check(
    matched && matched.title.toLowerCase().startsWith(prefix),
    "typeahead_jumps_to_a_match",
    `${JSON.stringify(matched)} for "${prefix}"`,
  );
  const letters = await page.evaluate(() => {
    const row = document.querySelector(".default-model-tree button.default-model-node .default-model-node-title");
    return row ? row.textContent.trim().slice(1, 2).toLowerCase() : "";
  });
  if (letters) {
    await page.keyboard.press(letters);
    await page.waitForTimeout(120);
    const buffered = await focused();
    check(
      buffered && buffered.title.toLowerCase().startsWith(prefix + letters),
      "typeahead_buffer_accumulates",
      JSON.stringify(buffered),
    );
  }
  await page.keyboard.press("Escape");
  await page.waitForFunction(
    () => {
      const pop = document.querySelector(".default-model-popover");
      return !pop || pop.hidden;
    },
    null,
    { timeout: 10000 },
  );
  check(true, "escape_closes_the_popover");
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/model-tree-keyboard.png" });
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
} finally {
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
