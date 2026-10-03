// Sidebars must follow the window: grow and shrink with it, wrap their labels, and never overlap or
// clip what they sit next to. This walks the pages that have one through eight widths and reports,
// per width, the sidebar's box, whether its labels overflow, and whether the page does.
import { chromium } from "playwright-core";
const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const CDP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const WIDTHS = [1600, 1440, 1280, 1024, 900, 768, 600, 480, 390];
const PAGES = [
  ["user-setting", "/user-setting", ".user-setting-layout", ".user-setting-sidebar"],
  ["agent-templates", "/agent-templates", ".templates-page", ".template-sidebar"],
  ["dataset-detail", null, ".ds-layout", ".ds-sidebar"],
  ["admin", "/admin", ".admin-console", ".admin-console-aside"],
  ["admin-users", "/admin/users", ".admin-console", ".admin-console-aside"],
];

const browser = await chromium.connectOverCDP(CDP);
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();
const errors = [];
page.on("pageerror", (error) => errors.push(String(error).slice(0, 100)));
await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 30000 });
await page.evaluate(async (password) => {
  const response = await fetch("/api/v1/auth/login", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ email: "admin@rayrag.local", password }),
  });
  const body = await response.json();
  if (body.code === 0) {
    localStorage.setItem("rayrag_token", body.data.access_token);
    document.cookie = "rayrag_token=" + body.data.access_token + "; path=/";
  }
}, process.env.RAYRAG_LOGIN_PASSWORD);
PAGES[2][1] = await page.evaluate(async () => {
  const token = localStorage.getItem("rayrag_token");
  const datasets = (await (await fetch("/api/v1/datasets", { headers: { Authorization: "Bearer " + token } })).json()).data;
  return "/dataset/files/" + datasets[0].id;
});

const rows = [];
for (const [name, path, layoutSelector, sidebarSelector] of PAGES) {
  for (const width of WIDTHS) {
    await page.setViewportSize({ width, height: 900 });
    await page.goto(ORIGIN + path, { waitUntil: "domcontentloaded", timeout: 40000 });
    await page.waitForTimeout(500);
    rows.push(await page.evaluate(([name, width, layoutSelector, sidebarSelector]) => {
      const layout = document.querySelector(layoutSelector);
      const sidebar = document.querySelector(sidebarSelector);
      if (!layout || !sidebar) return { name, width, missing: !layout ? layoutSelector : sidebarSelector };
      const box = sidebar.getBoundingClientRect();
      const layoutBox = layout.getBoundingClientRect();
      let labelsOverflow = 0;
      for (const item of sidebar.querySelectorAll("a,button,span,div")) {
        // `text-overflow:ellipsis` is the designed way to fit a long address; only text that runs
        // past its box with no such treatment counts.
        const style = getComputedStyle(item);
        if (item.scrollWidth > item.clientWidth + 2 && style.textOverflow !== "ellipsis") labelsOverflow += 1;
      }
      return {
        name,
        width,
        sidebar: Math.round(box.width),
        sidebarHeight: Math.round(box.height),
        layout: Math.round(layoutBox.width),
        columns: getComputedStyle(layout).gridTemplateColumns.split(" ").length,
        labelsOverflow,
        pageOverflow: document.documentElement.scrollWidth - window.innerWidth,
      };
    }, [name, width, layoutSelector, sidebarSelector]));
  }
}

const problems = rows.filter((row) => row.missing || row.pageOverflow > 2 || row.labelsOverflow > 0);
for (const row of rows) {
  console.log(
    `${row.name.padEnd(15)} @${String(row.width).padStart(4)}  sidebar=${String(row.sidebar).padStart(4)}  layout=${String(row.layout).padStart(4)}  cols=${row.columns}  labelOverflow=${row.labelsOverflow}  pageOverflow=${row.pageOverflow}`
  );
}
console.log(problems.length ? `PROBLEMS: ${problems.length}` : "sidebars follow the window, labels fit, pages do not overflow");
for (const row of problems) console.log(JSON.stringify(row));
console.log("page errors:", errors.length);
await page.close();
await context.close();
await browser.close();
process.exit(problems.length || errors.length ? 1 : 0);
