// CDP probe for the RAGFlow admin console subtree
// (`web/src/pages/admin/**`): `/admin` is the console login, `/admin/services`
// the component table, `/admin/users` the user management table,
// `/admin/users/:id` the account detail and `/admin/sandbox-settings` the
// sandbox surface. Every step below is a real click / typing sequence in
// Chrome driven through the DevTools Protocol.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/admin-console-cdp.mjs
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
const PROBE_USER = `cdp-admin-probe-${Date.now()}@example.com`;

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1440, height: 900 });
const results = { steps: [], errors: [] };
const shots = "docs/ui-traversal/rayrag-audit";
mkdirSync(shots, { recursive: true });

const text = (selector) =>
  page.locator(selector).first().textContent({ timeout: 8000 });

try {
  await ctx.addCookies([
    { name: "lang", value: "en", domain: "127.0.0.1", path: "/" },
  ]);

  // 1. Console login card: wrong password first (the card must stay and show
  //    the API message), then the real administrator sign-in. The session is
  //    cleared first so a stale token cannot skip the card.
  await page.goto(ORIGIN + "/login", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.evaluate(() => {
    localStorage.removeItem("rayrag_token");
    document.cookie = "rayrag_token=;path=/;max-age=0";
  });
  await page.goto(ORIGIN + "/admin", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  results.loginPage = {
    title: await text(".admin-login-title"),
    brand: await text(".admin-login-brand-name"),
    hasSubmit: (await page.locator("[data-testid='admin-login-submit']").count()) > 0,
    emailPlaceholder: await page
      .locator("#adminEmail")
      .getAttribute("placeholder"),
    passwordPlaceholder: await page
      .locator("#adminPassword")
      .getAttribute("placeholder"),
  };
  await page.screenshot({ path: `${shots}/admin-login-20260919.png` });

  await page.fill("#adminEmail", "admin@rayrag.local");
  await page.fill("#adminPassword", "definitely-wrong-password");
  await page.click("[data-testid='admin-login-submit']");
  await page.waitForTimeout(1200);
  results.badLogin = {
    error: await text("[data-testid='admin-login-error']"),
    stillOnLogin: page.url().includes("/admin") && !page.url().includes("/services"),
  };

  await page.fill("#adminPassword", PASSWORD);
  await page.click("[data-testid='admin-login-submit']");
  await page.waitForURL("**/admin/services", { timeout: 20000 });
  await page.waitForSelector("[data-testid='admin-services-table']", {
    timeout: 10000,
  });
  results.goodLogin = { url: page.url() };

  // 2. Sidebar nav: brand, three links, active state, version, logout button.
  results.shell = await page.evaluate(() => ({
    brand: document.querySelector(".admin-console-title").textContent,
    logo: !!document.querySelector(".admin-console-logo"),
    links: Array.from(
      document.querySelectorAll(".admin-console-nav-link"),
    ).map((link) => ({
      href: link.getAttribute("href"),
      label: link.querySelector(".admin-console-nav-label").textContent,
      active: link.classList.contains("active"),
      ariaCurrent: link.getAttribute("aria-current"),
      hasIcon: !!link.querySelector("svg"),
    })),
    version: document.querySelector(".admin-console-version").textContent,
    hasThemeSwitch: !!document.querySelector(".theme-switch"),
    logoutLabel: document.querySelector(".admin-console-logout").textContent,
    section: document
      .querySelector("[data-testid='admin-console']")
      .getAttribute("data-section"),
  }));

  // 3. Service table: rows, statuses, the type filter and the details dialog.
  results.services = await page.evaluate(() => {
    const rows = Array.from(
      document.querySelectorAll("[data-testid='admin-service-row']"),
    );
    return {
      rowCount: rows.length,
      first: rows[0]
        ? Array.from(rows[0].querySelectorAll("td")).map((cell) =>
            cell.textContent.trim(),
          )
        : null,
      types: Array.from(document.querySelectorAll("#adminServiceType option")).map(
        (option) => option.value,
      ),
      statuses: rows.map((row) => row.querySelector(".badge").textContent),
    };
  });
  await page.selectOption("#adminServiceType", "meta_data");
  await page.waitForTimeout(300);
  results.serviceFilter = await page.evaluate(() => ({
    rows: document.querySelectorAll("[data-testid='admin-service-row']").length,
    name: document.querySelector("[data-testid='admin-service-row'] td:nth-child(2)")
      .textContent,
  }));
  await page.click("button:has-text('Reset')");
  await page.waitForTimeout(300);
  await page.screenshot({ path: `${shots}/admin-services-20260919.png` });
  await page
    .locator("[data-testid='admin-service-row']")
    .first()
    .locator("button")
    .click();
  await page.waitForTimeout(600);
  results.serviceDialog = {
    visible: await page.locator("#adminServiceDialog").isVisible(),
    name: await text("#adminServiceName"),
    detailHasKeys: (await text("#adminServiceDetail")).includes("{"),
  };
  await page.click("#adminServiceDialog button:has-text('Close')");
  await page.waitForTimeout(300);
  results.serviceDialogClosed = !(await page
    .locator("#adminServiceDialog")
    .isVisible());

  // 4. Nav click: User management.
  await page.click("a[href='/admin/users']");
  await page.waitForURL("**/admin/users", { timeout: 15000 });
  await page.waitForSelector("[data-testid='admin-users-table']", {
    timeout: 10000,
  });
  await page.waitForTimeout(500);
  results.users = await page.evaluate(() => ({
    section: document
      .querySelector("[data-testid='admin-console']")
      .getAttribute("data-section"),
    activeHref: document
      .querySelector(".admin-console-nav-link.active")
      .getAttribute("href"),
    rowCount: document.querySelectorAll("[data-testid='admin-user-row']").length,
    headers: Array.from(document.querySelectorAll(".admin-table thead th")).map(
      (th) => th.textContent,
    ),
    firstRow: Array.from(
      document.querySelectorAll("[data-testid='admin-user-row']")[0].querySelectorAll("td"),
    ).map((cell) => cell.textContent.trim()),
  }));
  await page.screenshot({ path: `${shots}/admin-users-20260919.png` });

  // 5. Create a probe user through the dialog, then delete it again.
  await page.click("[data-testid='admin-new-user']");
  await page.waitForTimeout(300);
  await page.fill("#adminNewUserEmail", PROBE_USER);
  await page.fill("#adminNewUserPassword", "probe-password-12345");
  await page.selectOption("#adminNewUserRole", "normal");
  await page.click("[data-testid='admin-create-user-submit']");
  await page.waitForTimeout(1200);
  results.created = await page.evaluate(
    (email) => ({
      present: Array.from(
        document.querySelectorAll("[data-testid='admin-user-row']"),
      ).some((row) => row.textContent.includes(email)),
      error: document.getElementById("adminCreateUserError").textContent,
    }),
    PROBE_USER,
  );
  // Role filter narrows the table to that one account.
  await page.selectOption("#adminUserRole", "normal");
  await page.waitForTimeout(300);
  const filtered = await page.evaluate(
    (email) => ({
      rows: Array.from(
        document.querySelectorAll("[data-testid='admin-user-row']"),
      ).map((row) => row.textContent).filter((value) => value.includes(email)).length,
    }),
    PROBE_USER,
  );
  results.roleFilter = filtered;
  await page.click("button:has-text('Reset')");
  await page.waitForTimeout(300);

  // Delete the probe account through the confirmation dialog.
  await page.evaluate((email) => {
    const row = Array.from(
      document.querySelectorAll("[data-testid='admin-user-row']"),
    ).find((candidate) => candidate.textContent.includes(email));
    Array.from(row.querySelectorAll("button"))
      .find((button) => button.textContent === "Delete")
      .click();
  }, PROBE_USER);
  await page.waitForTimeout(300);
  results.deleteDialog = { target: await text("#adminDeleteTarget") };
  await page.click("[data-testid='admin-delete-user-submit']");
  await page.waitForTimeout(1200);
  results.deleted = await page.evaluate(
    (email) => ({
      present: Array.from(
        document.querySelectorAll("[data-testid='admin-user-row']"),
      ).some((row) => row.textContent.includes(email)),
    }),
    PROBE_USER,
  );

  // 6. User detail: follow the email link, then back.
  const detailHref = await page.evaluate(() => {
    const row = document.querySelector("[data-testid='admin-user-row']");
    return row.querySelector("a").getAttribute("href");
  });
  await page.goto(ORIGIN + detailHref, {
    waitUntil: "domcontentloaded",
    timeout: 20000,
  });
  await page.waitForSelector("[data-testid='admin-user-detail-panel']", {
    timeout: 10000,
  });
  await page.waitForTimeout(900);
  results.userDetail = await page.evaluate(() => ({
    email: document.querySelector("#adminDetailEmail").textContent,
    avatar: document.querySelector("#adminDetailAvatar").textContent,
    status: document.querySelector("#adminDetailStatus").textContent,
    role: document.querySelector("#adminDetailRole").textContent,
    fields: Array.from(
      document.querySelectorAll(".admin-detail-field"),
    ).map((box) => [
      box.querySelector(".admin-detail-field-label").textContent,
      box.textContent.replace(box.querySelector(".admin-detail-field-label").textContent, ""),
    ]),
    tabs: Array.from(document.querySelectorAll(".admin-tab")).map((tab) => ({
      label: tab.textContent,
      active: tab.classList.contains("active"),
      selected: tab.getAttribute("aria-selected"),
    })),
    datasetRows: Array.from(
      document.querySelectorAll("#adminUserDatasetRows tr"),
    ).map((row) => row.textContent),
    agentRows: Array.from(
      document.querySelectorAll("#adminUserAgentRows tr"),
    ).map((row) => row.textContent),
    datasetPanelHidden: document.querySelector("#adminDetailDatasetPanel").hidden,
    flowPanelHidden: document.querySelector("#adminDetailFlowPanel").hidden,
  }));
  // Real click on the Agent tab switches the panels.
  await page.click(".admin-tab[data-tab='flow']");
  await page.waitForTimeout(300);
  results.userDetailTabs = await page.evaluate(() => ({
    datasetPanelHidden: document.querySelector("#adminDetailDatasetPanel").hidden,
    flowPanelHidden: document.querySelector("#adminDetailFlowPanel").hidden,
    activeTab: document.querySelector(".admin-tab.active").textContent,
  }));
  await page.screenshot({ path: `${shots}/admin-user-detail-20260919.png` });

  // 7. Sandbox settings: the variables editor loads through the protected API.
  await page.goto(ORIGIN + "/admin/sandbox-settings", {
    waitUntil: "domcontentloaded",
    timeout: 20000,
  });
  await page.waitForSelector("[data-testid='admin-sandbox-table']", {
    timeout: 10000,
  });
  await page.waitForTimeout(1000);
  results.sandbox = await page.evaluate(() => ({
    title: document.querySelector(".admin-panel-title").textContent,
    description: document.querySelector(".admin-panel-description").textContent,
    rowCount: document.querySelectorAll("#adminSandboxRows tr").length,
    firstCell: (document.querySelector("#adminSandboxRows td") || {}).textContent,
  }));
  await page.screenshot({ path: `${shots}/admin-sandbox-20260919.png` });

  // 8. Logout through the sidebar revokes the console session.
  await page.click(".admin-console-logout");
  await page.waitForURL("**/admin", { timeout: 20000 });
  await page.waitForTimeout(600);
  results.logout = {
    url: page.url(),
    backToLogin: (await page.locator("[data-testid='admin-login-submit']").count()) > 0,
  };
  await page.goto(ORIGIN + "/admin/services", {
    waitUntil: "domcontentloaded",
    timeout: 20000,
  });
  results.afterLogout = {
    url: page.url(),
    onLogin: (await page.locator("[data-testid='admin-login-submit']").count()) > 0,
  };
} catch (error) {
  results.errors.push(String(error && error.stack ? error.stack : error));
  await page.screenshot({ path: `${shots}/admin-console-error-20260919.png` });
} finally {
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
}
if (results.errors.length) process.exit(1);
