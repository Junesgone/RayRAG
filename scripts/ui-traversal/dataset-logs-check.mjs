import { chromium } from "playwright-core";
const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1000 });
const errs = [];
page.on("pageerror", (e) => errs.push(String(e).slice(0, 120)));
await page.goto((process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390") + "/login", { waitUntil: "domcontentloaded" });
await page.evaluate(async (pw) => {
  const r = await fetch("/api/v1/auth/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ email: "admin@rayrag.local", password: pw }) });
  const d = await r.json();
  if (d.code === 0) localStorage.setItem("rayrag_token", d.data.access_token);
}, process.env.RAYRAG_LOGIN_PASSWORD || process.env.UI_DUMP_PASSWORD);
const kb = await page.evaluate(async () => (await (await fetch("/api/v1/datasets")).json()).data[0].id);
await page.goto(`${process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390"}/dataset/logs/${kb}`, { waitUntil: "domcontentloaded" });
await page.waitForTimeout(2500);
console.log(JSON.stringify(await page.evaluate(() => ({
  logRowCount: document.querySelectorAll("#logBody tr").length,
  logBodyLength: (document.getElementById("logBody")?.innerHTML || "").length,
  emptyText: (document.getElementById("piModelLoading")?.textContent || "").trim(),
})), null, 1));
console.log("pageErrors:", JSON.stringify(errs));
await page.screenshot({ path: "/tmp/rayrag-shots/rayrag-dataset-logs.png" });
await page.close();
process.exit(0);
