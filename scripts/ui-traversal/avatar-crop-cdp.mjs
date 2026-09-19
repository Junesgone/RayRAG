// CDP probe for the draggable avatar crop dialog
// (`web/src/components/avatar-upload.tsx`): the dialog seeds an 80% selection,
// dragging it moves the crop box within the image bounds and the confirm button
// writes the chosen region back into the avatar preview. Real mouse drags; the
// probe discards the change afterwards (no save request is sent).
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/avatar-crop-cdp.mjs
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
await page.setViewportSize({ width: 1500, height: 950 });
const results = { errors: [], pageErrors };
mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });

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
  await page.goto(`${ORIGIN}/user-setting/profile`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("#avatarCropModal", { state: "attached", timeout: 20000 });

  // Feed a synthetic file through the shared opener (same code path as the file input).
  await page.evaluate(async () => {
    const canvas = document.createElement("canvas");
    canvas.width = 400;
    canvas.height = 260;
    const ctx = canvas.getContext("2d");
    ctx.fillStyle = "#123456";
    ctx.fillRect(0, 0, 400, 260);
    ctx.fillStyle = "#ffcc00";
    ctx.fillRect(300, 0, 100, 100);
    const blob = await new Promise((resolve) => canvas.toBlob(resolve, "image/png"));
    const file = new File([blob], "probe.png", { type: "image/png" });
    window.__probeCrop = await openAvatarCropModal(file, "", "profileAvatarPreview", "");
  });
  await page.waitForFunction(
    () => document.getElementById("avatarCropModal").classList.contains("show"),
    null,
    { timeout: 10000 },
  );
  const before = await page.evaluate(() => {
    const box = document.getElementById("avatarCropBox");
    const stage = document.getElementById("avatarCropStage");
    const image = document.getElementById("avatarCropImage");
    const area = document.getElementById("avatarCropModal")._area;
    return {
      box: { width: box.style.width, height: box.style.height, left: box.style.left, top: box.style.top },
      area: { size: Math.round(area.size), x: Math.round(area.x), y: Math.round(area.y) },
      stage: { width: stage.clientWidth, height: stage.clientHeight },
      imageNatural: { width: image.naturalWidth, height: image.naturalHeight },
      selectionIs80Percent: Math.abs(area.size - Math.min(image.naturalWidth, image.naturalHeight) * 0.8) < 1,
    };
  });
  // Real drag: press inside the box and move 60px right.
  const handle = await page.$("[data-testid='avatar-crop-box']");
  const boxInfo = await handle.boundingBox();
  await page.mouse.move(boxInfo.x + boxInfo.width / 2, boxInfo.y + boxInfo.height / 2);
  await page.mouse.down();
  await page.mouse.move(boxInfo.x + boxInfo.width / 2 + 60, boxInfo.y + boxInfo.height / 2, { steps: 8 });
  await page.mouse.up();
  await page.waitForTimeout(200);
  const after = await page.evaluate(() => {
    const modal = document.getElementById("avatarCropModal");
    return { area: { x: Math.round(modal._area.x), y: Math.round(modal._area.y) }, left: document.getElementById("avatarCropBox").style.left };
  });
  results.drag = { before, after, movedRight: after.area.x > before.area.x };
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/avatar-crop-20260920.png" });
  // Drag far beyond the right edge: the selection must clamp to the image bounds.
  await page.mouse.move(boxInfo.x + 40, boxInfo.y + 40);
  await page.mouse.down();
  await page.mouse.move(boxInfo.x + 3000, boxInfo.y + 3000, { steps: 10 });
  await page.mouse.up();
  await page.waitForTimeout(200);
  results.clamped = await page.evaluate(() => {
    const modal = document.getElementById("avatarCropModal");
    const img = modal._image;
    const area = modal._area;
    return {
      x: Math.round(area.x),
      y: Math.round(area.y),
      maxX: Math.round(img.width - area.size),
      maxY: Math.round(img.height - area.size),
      withinBounds: area.x <= img.width - area.size + 1 && area.y <= img.height - area.size + 1,
    };
  });
  // Confirm writes a 64x64 PNG into the avatar preview.
  await page.click("[data-testid='avatar-crop-confirm']");
  await page.waitForTimeout(300);
  results.enButtons = await page.evaluate(() => ({
    ok: document.getElementById("avatarCropOk").textContent.trim(),
    cancel: document.getElementById("avatarCropCancel").textContent.trim(),
  }));
  results.confirmed = await page.evaluate(() => {
    const preview = document.getElementById("profileAvatarPreview");
    const src = preview ? preview.src || preview.getAttribute("src") || "" : "";
    return {
      dialogClosed: !document.getElementById("avatarCropModal").classList.contains("show"),
      previewIsPng: src.startsWith("data:image/png"),
      previewLength: src.length,
    };
  });
  // Chinese locale: the dialog footer localises too.
  await ctx.addCookies([{ name: "lang", value: "zh-CN", domain: "127.0.0.1", path: "/" }]);
  await page.goto(`${ORIGIN}/user-setting/profile`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForSelector("#avatarCropModal", { state: "attached", timeout: 20000 });
  results.zhButtons = await page.evaluate(() => ({
    ok: document.getElementById("avatarCropOk").textContent.trim(),
    cancel: document.getElementById("avatarCropCancel").textContent.trim(),
  }));
} catch (error) {
  results.errors.push(String(error && error.stack ? error.stack : error));
} finally {
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
}
if (results.errors.length || results.pageErrors.length) process.exit(1);
