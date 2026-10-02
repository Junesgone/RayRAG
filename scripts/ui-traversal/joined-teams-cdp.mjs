// Joined teams table (upstream `setting-team/tenant-table.tsx`): verifies the
// four columns, the tri-state update-date sort, the loading/no-data rows and the
// role-dependent action cell (Agree/Refuse for invitations, icon-only quit for
// joined tenants) against the live deployment.
//
// Usage: UI_DUMP_PASSWORD=... node scripts/ui-traversal/joined-teams-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD || process.env.UI_DUMP_PASSWORD;
const SHOT = process.env.SHOT_DIR || "/tmp";

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 180)));
page.on("dialog", async (d) => await d.accept());
const out = { steps: [], pageErrors };

const snapTable = () =>
  page.evaluate(() => ({
    headers: Array.from(document.querySelectorAll("[data-testid='joined-teams-card'] thead th")).map((th) =>
      (th.textContent || "").trim(),
    ),
    sortIcon: (document.getElementById("teamDateSortIcon")?.textContent || "").trim(),
    sortLabel: document.getElementById("teamDateSort")?.getAttribute("aria-label") || "",
    rows: Array.from(document.querySelectorAll("#teamRows tr")).map((tr) => ({
      tenant: tr.dataset.tenant || "",
      name: (tr.children[0]?.textContent || "").trim(),
      hasAvatar: !!tr.querySelector(".team-avatar"),
      date: (tr.children[1]?.textContent || "").trim(),
      email: (tr.children[2]?.textContent || "").trim(),
      agree: (tr.querySelector(".team-agree")?.textContent || "").trim(),
      refuse: (tr.querySelector(".team-refuse")?.textContent || "").trim(),
      quit: tr.querySelector(".team-quit")
        ? {
            text: (tr.querySelector(".team-quit").textContent || "").trim(),
            label: tr.querySelector(".team-quit").getAttribute("aria-label") || "",
            title: tr.querySelector(".team-quit").getAttribute("title") || "",
          }
        : null,
      loading: !!tr.querySelector(".team-loading .team-spinner"),
      empty: (tr.querySelector(".team-empty")?.textContent || "").trim(),
    })),
  }));

const shot = async (label) =>
  page.screenshot({ path: `${SHOT}/rayrag-joined-teams-${out.steps.length + 1}-${label}.png` });

const FIXTURE = [
  {
    tenant_id: "owner-self",
    user_id: "owner-self",
    role: "owner",
    nickname: "Admin User",
    email: "admin@rayrag.local",
    avatar: "",
    update_date: 3000,
  },
  {
    tenant_id: "tenant-invite",
    user_id: "me",
    role: "invite",
    nickname: "Zoe Inviter",
    email: "zoe@example.com",
    avatar: "",
    update_date: 1000,
  },
  {
    tenant_id: "tenant-normal",
    user_id: "me",
    role: "normal",
    nickname: "Mia Member",
    email: "mia@example.com",
    avatar: "",
    update_date: 2000,
  },
];

try {
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.evaluate(async (pw) => {
    const r = await fetch("/api/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "admin@rayrag.local", password: pw }),
    });
    const d = await r.json();
    if (d.code === 0) localStorage.setItem("rayrag_token", d.data.access_token);
  }, PASSWORD);

  // Live payload: the tenant row must carry the owner profile + update_date.
  await page.goto(ORIGIN + "/user-setting/team", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForFunction(
    () => Array.from(document.querySelectorAll("#teamRows tr")).every((tr) => !tr.querySelector(".team-loading")),
    null,
    { timeout: 20000 },
  );
  out.livePayload = await page.evaluate(async () => {
    const body = await (await fetch("/api/v1/tenants")).json();
    const row = (body.data || [])[0] || {};
    return { keys: Object.keys(row).sort(), role: row.role, nickname: row.nickname, email: row.email };
  });
  out.steps.push({ label: "live", ...(await snapTable()) });
  await shot("live");

  // Deterministic fixture: invite + normal + owner rows with distinct dates.
  const stub = async (route) => {
    await new Promise((resolve) => setTimeout(resolve, 600));
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ code: 0, data: FIXTURE }),
    });
  };
  await page.route("**/api/v1/tenants", stub);
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.waitForSelector("#teamRows .team-loading", { timeout: 8000 });
  out.steps.push({ label: "loading", ...(await snapTable()) });
  await shot("loading");
  await page.waitForSelector("#teamRows tr[data-testid='joined-team-row']", { timeout: 8000 });
  out.steps.push({ label: "unsorted", ...(await snapTable()) });
  await shot("unsorted");

  for (const [index, expected] of [
    ["asc", ["tenant-invite", "tenant-normal", "owner-self"]],
    ["desc", ["owner-self", "tenant-normal", "tenant-invite"]],
    ["none", ["owner-self", "tenant-invite", "tenant-normal"]],
  ]) {
    await page.click("#teamDateSort");
    await page.waitForTimeout(300);
    const state = await snapTable();
    out.steps.push({ label: `sort-${index}`, ...state });
    out.steps[out.steps.length - 1].order = state.rows.map((r) => r.tenant);
    out.steps[out.steps.length - 1].expected = expected;
    await shot(`sort-${index}`);
  }

  // Search filter narrows the visible rows, then the empty fixture shows `No data`.
  await page.fill("#teamSearch", "mia");
  await page.waitForTimeout(250);
  out.filtered = await snapTable();
  out.filtered.rows = out.filtered.rows.map((r) => r.tenant);
  await shot("filtered");
  await page.fill("#teamSearch", "");
  await page.unroute("**/api/v1/tenants", stub);
  await page.route("**/api/v1/tenants", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ code: 0, data: [] }) }),
  );
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.waitForSelector("#teamRows .team-empty", { timeout: 8000 });
  out.empty = await snapTable();
  await shot("empty");

  await page.unroute("**/api/v1/tenants");
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.waitForSelector("#teamRows tr[data-testid='joined-team-row']", { timeout: 8000 });
  out.restored = (await snapTable()).rows.map((r) => r.tenant);
} catch (error) {
  out.fatal = String(error && error.stack ? error.stack : error).slice(0, 400);
} finally {
  await page.unroute("**/api/v1/tenants").catch(() => {});
  console.log(JSON.stringify(out, null, 2));
  await page.close();
  await browser.close();
}
