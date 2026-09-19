// CDP probe for `/admin/users/{id}` (`pages/admin/user-detail.tsx`): the page
// fetches the detail, dataset and agent endpoints in parallel, renders the
// upstream account summary (Active/Inactive badge, role badge, last-login /
// create / update / language / anonymous / superuser fields) and paginates both
// asset tabs. The probe creates eleven throwaway datasets so the pager really
// has a second page, then deletes them again.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/admin-user-detail-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}
const EMAIL = process.env.RAYRAG_ADMIN_EMAIL || "admin@rayrag.local";
const EXTRA = 11;

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

const created = [];
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

  const before = await page.evaluate(async (email) => {
    const res = await fetch(`/api/v1/admin/users/${encodeURIComponent(email)}/datasets`);
    const data = await res.json();
    return { code: data.code, count: (data.data || []).length, first: (data.data || [])[0] || null };
  }, EMAIL);
  check(before.code === 0, "datasets_endpoint_ok");
  check(
    before.first && "language" in before.first && "chunk_num" in before.first,
    "dataset_rows_carry_the_upstream_fields",
  );
  const agents = await page.evaluate(async (email) => {
    const res = await fetch(`/api/v1/admin/users/${encodeURIComponent(email)}/agents`);
    const data = await res.json();
    return { code: data.code, rows: data.data || [] };
  }, EMAIL);
  check(agents.code === 0, "agents_endpoint_ok");
  const detail = await page.evaluate(async (email) => {
    const res = await fetch(`/api/v1/admin/users/${encodeURIComponent(email)}`);
    const data = await res.json();
    return { code: data.code, rows: data.data, first: (data.data || [])[0] || null };
  }, EMAIL);
  check(
    detail.code === 0 && Array.isArray(detail.rows) && detail.rows.length === 1,
    "detail_payload_is_a_list",
  );
  check(detail.first && detail.first.email === EMAIL, "detail_row_is_the_account");
  check(detail.first && "is_superuser" in detail.first && "login_channel" in detail.first, "detail_fields");

  for (let index = 0; index < EXTRA; index += 1) {
    const id = await page.evaluate(async (name) => {
      const res = await fetch("/api/v1/datasets", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name }),
      });
      const data = await res.json();
      return data.data && data.data.id ? data.data.id : "";
    }, `probe-detail-${Date.now()}-${index}`);
    if (id) created.push(id);
  }
  check(created.length === EXTRA, "throwaway_datasets_created", String(created.length));

  await page.goto(`${ORIGIN}/admin/users/${encodeURIComponent(EMAIL)}`, {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForSelector("[data-testid='admin-user-detail-panel']", { timeout: 20000 });
  await page.waitForSelector("#adminUserDatasetRows tr", { timeout: 20000 });
  check((await text("#adminDetailEmail")) === EMAIL, "header_email");
  check((await text("#adminDetailStatus")) === "Active", "header_status_badge");
  check((await text("#adminDetailRole")) === "admin", "header_role_badge");
  const fields = await page.locator("#adminDetailFields .admin-detail-field").allInnerTexts();
  const labels = fields.map((entry) => entry.split("\n")[0].trim());
  check(
    ["Last login time", "Create time", "Last update time", "Language", "Is anonymous", "Is superuser"].every(
      (label) => labels.includes(label),
    ),
    "upstream_summary_fields",
    labels.join("|"),
  );
  check(
    fields.some((entry) => /Is superuser\nYes/.test(entry)),
    "superuser_is_yes_for_the_admin",
  );
  check((await page.locator(".admin-tab", { hasText: "Dataset" }).count()) === 1, "dataset_tab");
  check((await page.locator(".admin-tab", { hasText: "Agent" }).count()) === 1, "agent_tab");

  const total = before.count + EXTRA;
  const pager = "#adminUserDatasetPager";
  check((await text(`${pager} .admin-pager-total`)) === `Total ${total}`, "pager_total", await text(`${pager} .admin-pager-total`));
  const pages = Math.ceil(total / 10);
  check((await text(`${pager} .admin-pager-now`)) === `1 / ${pages}`, "pager_first_page", await text(`${pager} .admin-pager-now`));
  const firstPageRows = await page.locator("#adminUserDatasetRows tr").count();
  check(firstPageRows === Math.min(10, total), "first_page_row_count", String(firstPageRows));
  await page.locator(`${pager} .admin-pager-step`, { hasText: "Next" }).click();
  await page.waitForFunction(
    (expected) => document.querySelector("#adminUserDatasetPager .admin-pager-now").textContent === expected,
    `2 / ${pages}`,
    { timeout: 10000 },
  );
  check(true, "pager_advances");
  const secondPageRows = await page.locator("#adminUserDatasetRows tr").count();
  check(secondPageRows === total - 10, "second_page_row_count", String(secondPageRows));
  await page.locator(`${pager} .admin-pager-size`).selectOption("20");
  await page.waitForFunction(
    () => document.querySelector("#adminUserDatasetPager .admin-pager-now").textContent === "1 / 1",
    null,
    { timeout: 10000 },
  );
  check(true, "page_size_change_resets_to_the_first_page");

  await page.locator(".admin-tab", { hasText: "Agent" }).click();
  await page.waitForFunction(
    () => !document.getElementById("adminDetailFlowPanel").hidden,
    null,
    { timeout: 10000 },
  );
  check(true, "agent_tab_switches");
  if (agents.rows.length) {
    const rowCount = await page.locator("#adminUserAgentRows tr").count();
    check(rowCount === agents.rows.length, "agent_rows_render", String(rowCount));
    check(
      (await text("#adminUserAgentRows")).includes(agents.rows[0].title),
      "agent_title_rendered",
    );
  } else {
    check(
      (await text("#adminUserAgentRows")) === "No agent",
      "agent_empty_state",
      await text("#adminUserAgentRows"),
    );
  }
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/admin-user-detail.png" });
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
} finally {
  // Always remove the throwaway datasets.
  let cleaned = 0;
  if (created.length) {
    cleaned = await page
      .evaluate(async (ids) => {
        const res = await fetch("/api/v1/datasets", {
          method: "DELETE",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ ids }),
        });
        const text = await res.text();
        if (!text) return res.ok ? ids.length : 0;
        try {
          return JSON.parse(text).data.success_count;
        } catch {
          return res.ok ? ids.length : 0;
        }
      }, created)
      .catch(() => 0);
  }
  results.cleanup_deleted = cleaned === created.length ? "ok" : "FAIL";
  if (cleaned !== created.length) {
    results.errors.push(`cleanup_deleted (${cleaned}/${created.length})`);
  }
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
