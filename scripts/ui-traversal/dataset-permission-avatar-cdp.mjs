// Dataset Configuration tab: permission searchable select (upstream
// PermissionFormField) + avatar AvatarUpload states, with save round-trip.
//
// Usage: UI_DUMP_PASSWORD=... node scripts/ui-traversal/dataset-permission-avatar-cdp.mjs
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
const out = { steps: [] };

const snap = async (label) => {
  const state = await page.evaluate(() => {
    const input = document.getElementById("cfgPerm");
    return {
      permValue: input ? input.value : null,
      permLabel: (document.getElementById("cfgPermSelectedLabel")?.textContent || "").trim(),
      popoverOpen: !document.getElementById("cfgPermPopover").hidden,
      optionRows: Array.from(document.querySelectorAll("#cfgPermList .input-select-option")).map((r) => r.textContent.trim()),
      avatarAddVisible: !document.getElementById("cfgAvatarAdd").hidden,
      avatarPreviewVisible: !document.getElementById("cfgAvatarPreviewWrap").hidden,
      avatarSrcPrefix: (document.getElementById("cfgAvatarPreview")?.src || "").slice(0, 22),
      avatarTestId: !!document.querySelector("[data-testid='ds-settings-basic-avatar-upload']"),
      permTestId: !!document.querySelector("[data-testid='ds-settings-basic-permissions-select']"),
    };
  });
  out.steps.push({ label, ...state });
  await page.screenshot({ path: `${SHOT}/rayrag-ds-perm-${out.steps.length}-${label}.png` });
  return state;
};

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
  const kb = await page.evaluate(async () => {
  const list = await (await fetch("/api/v1/datasets")).json();
  const rows = list.data || [];
  if (rows.length) return rows[0].id;
  const created = await (await fetch("/api/v1/datasets", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ name: "probe-" + Date.now() }) })).json();
  return created.data && created.data.id;
});
  await page.goto(`${ORIGIN}/dataset/configuration/${kb}`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(2200);
  await snap("initial");

  // Searchable select: open, filter, pick Team.
  await page.locator("#cfgPermTrigger").click({ timeout: 8000 });
  await page.waitForTimeout(300);
  await snap("opened");
  await page.locator("#cfgPerm").fill("tea");
  await page.waitForTimeout(250);
  await snap("filtered");
  await page.locator("#cfgPermList .input-select-option").first().click({ timeout: 8000 });
  await page.waitForTimeout(300);
  await snap("picked-team");

  // Avatar: upload a 1x1 PNG (cropped to 64x64) then remove it.
  await page.setInputFiles(
    "#cfgAvatar",
    {
      name: "avatar.png",
      mimeType: "image/png",
      buffer: Buffer.from(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8AAAwAB/AGtE0m9AAAAAElFTkSuQmCC",
        "base64",
      ),
    },
    { timeout: 8000 },
  );
  await page.waitForTimeout(900);
  await snap("avatar-uploaded");
  await page.locator("#cfgAvatarRemove").click({ timeout: 8000 });
  await page.waitForTimeout(400);
  await snap("avatar-removed");

  // Save round-trip through the configuration PUT.
  const responses = [];
  page.on("response", (res) => {
    if (res.url().includes("/api/v1/datasets/") && res.request().method() === "PUT") {
      responses.push(res.status());
    }
  });
  await page.locator("#cfgPerm").evaluate((el) => { el.dataset.force = "1"; });
  await page.evaluate(() => {
    const input = document.getElementById("cfgPerm");
    input.value = "team";
    if (typeof cfgPermissionSet === "function") cfgPermissionSet("team", true);
  });
  await page.locator("[data-testid='ds-settings-basic-save-btn']").click({ timeout: 8000 });
  await page.waitForTimeout(1500);
  out.saveStatus = responses;
  out.persisted = await page.evaluate(async (id) => {
    const res = await fetch(`/api/v1/datasets/${id}`);
    const data = await res.json();
    return (data.data || {}).permission;
  }, kb);
} catch (error) {
  out.error = String(error && error.message ? error.message : error).slice(0, 300);
}
out.pageErrors = pageErrors;
console.log(JSON.stringify(out, null, 1));
await page.close().catch(() => {});
process.exit(0);
