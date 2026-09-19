// CDP probe for `/admin/users/{id}` (`pages/admin/user-detail.tsx`): the two
// asset tables render a single visible column — avatar plus the dataset `name` /
// agent `title` — with no header row at all (upstream keeps `<TableHeader>`
// commented out), `TableEmpty` when a tab has nothing to show, bordered pill tabs
// that fill when active, and an outline back button with the arrow icon. The
// probe creates one throwaway dataset so the dataset tab really renders a row,
// then deletes it again.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/admin-user-detail-tables-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}
const THROWAWAY = "detail-probe-dataset";

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

let datasetId = null;
try {
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(ORIGIN + "/admin", { waitUntil: "domcontentloaded", timeout: 40000 });
  if (await page.locator("#adminEmail").count()) {
    await page.fill("#adminEmail", "admin@rayrag.local");
    await page.fill("#adminPassword", PASSWORD);
    await page.click("[data-testid='admin-login-submit']");
    await page.waitForURL("**/admin/services", { timeout: 20000 });
  }

  // One throwaway dataset so the dataset tab has a row to render.
  const seeded = await page.evaluate(async (name) => {
    const token = localStorage.getItem("rayrag_token") || "";
    const res = await fetch("/api/v1/datasets", {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: "Bearer " + token },
      body: JSON.stringify({ name, description: "probe", language: "English" }),
    });
    const data = await res.json().catch(() => ({}));
    return { code: data.code, id: data.data && data.data.id };
  }, THROWAWAY);
  datasetId = seeded.id || null;
  check(seeded.code === 0 && !!datasetId, "throwaway_dataset", JSON.stringify(seeded));

  await page.goto(ORIGIN + "/admin/users/admin@rayrag.local", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForSelector("[data-testid='admin-user-detail-panel']", { timeout: 20000 });
  await page.waitForTimeout(900);

  // Back control: outline button with the arrow icon and `admin.back`.
  results.back = {
    text: await text("[data-testid='admin-user-back']"),
    svg: await page.locator("[data-testid='admin-user-back'] svg").count(),
    height: await page
      .locator("[data-testid='admin-user-back']")
      .evaluate((node) => getComputedStyle(node).height),
    href: await page.locator("[data-testid='admin-user-back']").getAttribute("href"),
  };
  check(
    results.back.text === "Back" &&
      results.back.svg === 1 &&
      results.back.height === "40px" &&
      results.back.href === "/admin/users",
    "back_button",
    JSON.stringify(results.back),
  );
  await page.click("[data-testid='admin-user-back']");
  await page.waitForURL("**/admin/users", { timeout: 15000 });
  results.back_navigation = page.url().replace(ORIGIN, "");
  check(results.back_navigation === "/admin/users", "back_navigates", results.back_navigation);
  await page.goBack({ waitUntil: "domcontentloaded" });
  await page.waitForSelector("[data-testid='admin-user-detail-panel']", { timeout: 20000 });
  await page.waitForTimeout(700);

  // Tabs: bordered pills, the active one filled with the card colour.
  results.tabs = await page.locator(".admin-tab").evaluateAll((nodes) =>
    nodes.map((node) => {
      const style = getComputedStyle(node);
      return {
        label: node.textContent.trim(),
        active: node.classList.contains("active"),
        border: `${style.borderTopWidth} ${style.borderTopStyle}`,
        radius: style.borderTopLeftRadius,
        background: style.backgroundColor,
        font: `${style.fontSize} ${style.fontWeight}`,
      };
    }),
  );
  const activeTab = results.tabs.find((tab) => tab.active);
  const idleTab = results.tabs.find((tab) => !tab.active);
  check(
    results.tabs.length === 2 &&
      results.tabs.map((tab) => tab.label).join("|") === "Dataset|Agent" &&
      activeTab.border === "0.5px solid" &&
      idleTab.border === "0.5px solid" &&
      activeTab.radius === "4px" &&
      activeTab.background !== idleTab.background &&
      activeTab.font === "14px 500",
    "tab_pills",
    JSON.stringify(results.tabs),
  );

  // Dataset tab: one column, no header, avatar + name.
  const datasetTable = page.locator("[data-testid='admin-user-datasets']");
  results.dataset_table = {
    headers: await datasetTable.locator("thead").count(),
    cells: await datasetTable.locator("tbody tr:first-child td").count(),
    name: await text("[data-testid='admin-user-datasets'] tbody tr:first-child td"),
    avatar: await datasetTable.locator("tbody tr:first-child .admin-detail-asset-avatar").count(),
  };
  check(
    results.dataset_table.headers === 0 &&
      results.dataset_table.cells === 1 &&
      results.dataset_table.avatar === 1 &&
      results.dataset_table.name.includes(THROWAWAY),
    "dataset_single_column",
    JSON.stringify(results.dataset_table),
  );

  // Agent tab: same shape, `TableEmpty` because this account has no agents.
  await page.click(".admin-tab[data-tab='flow']");
  await page.waitForTimeout(700);
  const agentTable = page.locator("[data-testid='admin-user-agents']");
  results.agent_table = {
    headers: await agentTable.locator("thead").count(),
    cell: await text("[data-testid='admin-user-agents'] tbody tr:first-child td"),
    colSpan: await agentTable
      .locator("tbody tr:first-child td")
      .getAttribute("colspan")
      .catch(() => null),
    cellHeight: await agentTable
      .locator("tbody tr:first-child td")
      .evaluate((node) => getComputedStyle(node).height),
    cellAlign: await agentTable
      .locator("tbody tr:first-child td")
      .evaluate((node) => getComputedStyle(node).textAlign),
  };
  check(
    results.agent_table.headers === 0 &&
      results.agent_table.cell === "No results found" &&
      results.agent_table.colSpan === "1" &&
      results.agent_table.cellHeight === "96px" &&
      results.agent_table.cellAlign === "center",
    "agent_empty_state",
    JSON.stringify(results.agent_table),
  );

  // Back to the dataset tab and confirm the empty state is not sticky.
  await page.click(".admin-tab[data-tab='dataset']");
  await page.waitForTimeout(500);
  results.dataset_rows_after_return = await datasetTable.locator("tbody tr").count();
  check(results.dataset_rows_after_return >= 1, "dataset_rows_after_return");

  // The account summary keeps the upstream six fields and the two badges.
  results.summary = {
    fields: await page.locator(".admin-detail-field-label").allInnerTexts(),
    email: await text("#adminDetailEmail"),
    status: await text("#adminDetailStatus"),
  };
  check(
    results.summary.fields.join("|") ===
      "Last login time|Create time|Last update time|Language|Is anonymous|Is superuser" &&
      results.summary.email === "admin@rayrag.local" &&
      results.summary.status === "Active",
    "account_summary",
    JSON.stringify(results.summary),
  );

  check(pageErrors.length === 0, "no_page_errors", pageErrors.join(" | "));
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/admin-user-detail-tables-failure.png" }).catch(() => {});
} finally {
  if (datasetId) {
    const cleaned = await page
      .evaluate(async (ids) => {
        const token = localStorage.getItem("rayrag_token") || "";
        const res = await fetch("/api/v1/datasets", {
          method: "DELETE",
          headers: { "Content-Type": "application/json", Authorization: "Bearer " + token },
          body: JSON.stringify({ ids }),
        });
        const body = await res.text();
        if (!body) return res.ok ? ids.length : 0;
        try {
          return JSON.parse(body).data.success_count;
        } catch {
          return res.ok ? ids.length : 0;
        }
      }, [datasetId])
      .catch(() => 0);
    results.cleanup_deleted = cleaned === 1 ? "ok" : "FAIL";
    if (cleaned !== 1) results.errors.push(`cleanup_deleted (${cleaned})`);
  }
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
