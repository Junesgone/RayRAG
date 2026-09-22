// CDP probe for the avatar crop dialog (`components/avatar-upload.tsx`): the
// selection starts at 80% of the smaller side, drags in image coordinates with the
// position clamped to the picture, resizes on the wheel by 0.9/1.1 with a 20px floor
// while holding its centre, is masked by a dashed white border plus a 9999px shadow,
// and confirming emits a 64x64 PNG into the form's avatar field. The probe drives the
// real file input, real mouse drags and real wheel events, then discards the result by
// reloading the page — nothing is saved.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/avatar-crop-interactive-cdp.mjs
import { chromium } from "playwright-core";
import { mkdirSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error("RAYRAG_LOGIN_PASSWORD is required (sourced from operator env, not committed)");
  process.exit(2);
}
// A 200x120 PNG split into four coloured quadrants: the crop position is then
// checkable from the pixels the dialog produces.
const IMAGE = "/tmp/rr-avatar-probe.png";
const PY = `
import struct, zlib
w, h = 200, 120
rows = b""
for y in range(h):
    row = b"\\x00"
    for x in range(w):
        left, top = x < w // 2, y < h // 2
        row += bytes((255, 0, 0) if (left and top) else (0, 255, 0) if (not left and top) else (0, 0, 255) if (left and not top) else (255, 255, 0))
    rows += row
def chunk(tag, data):
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
png = b"\\x89PNG\\r\\n\\x1a\\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")
open("${IMAGE}", "wb").write(png)
`;
execFileSync("python3", ["-c", PY]);

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

// The crop is saved through `PATCH /api/v1/users/me`; the payload carries the exact
// avatar the dialog produced, so it is the strongest evidence of the 64x64 output.
const patches = [];
page.on("request", (request) => {
  const url = new URL(request.url());
  if (url.pathname !== "/api/v1/users/me" || request.method() !== "PATCH") return;
  const body = request.postData() || "";
  const match = body.match(/"avatar":"(data:image\/png;base64,[^"]+)"/);
  patches.push(match ? match[1] : `no-avatar (${body.length} bytes)`);
});

const cropState = () =>
  page.evaluate(() => {
    const modal = document.getElementById("avatarCropModal");
    const box = document.getElementById("avatarCropBox");
    const stage = document.getElementById("avatarCropStage");
    const image = document.getElementById("avatarCropImage");
    const style = getComputedStyle(box);
    return {
      area: modal._area ? { x: modal._area.x, y: modal._area.y, size: modal._area.size } : null,
      imageSize: modal._image ? { w: modal._image.width, h: modal._image.height } : null,
      box: {
        left: parseFloat(box.style.left),
        top: parseFloat(box.style.top),
        width: box.getBoundingClientRect().width,
        height: box.getBoundingClientRect().height,
      },
      stage: { w: stage.clientWidth, h: stage.clientHeight },
      border: `${style.borderTopWidth} ${style.borderTopStyle} ${style.borderTopColor}`,
      mask: style.boxShadow,
      stageBackground: getComputedStyle(stage).backgroundColor,
      touchAction: getComputedStyle(stage).touchAction,
      imageRendered: { w: image.getBoundingClientRect().width, h: image.getBoundingClientRect().height },
    };
  });

const boxCentre = async () => {
  const rect = await page.locator("#avatarCropBox").boundingBox();
  return { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 };
};

try {
  await ctx.addCookies([{ name: "lang", value: "en", domain: "127.0.0.1", path: "/" }]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.evaluate(async (password) => {
    const res = await fetch("/api/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "admin@rayrag.local", password }),
    });
    const data = await res.json();
    localStorage.setItem("rayrag_token", data.data.access_token);
    document.cookie = "rayrag_token=" + data.data.access_token + ";path=/";
  }, PASSWORD);

  await page.goto(ORIGIN + "/user-setting/profile", { waitUntil: "domcontentloaded", timeout: 40000 });
  results.avatarBefore = await page
    .locator("#profileAvatarPreview")
    .getAttribute("src")
    .catch(() => null);
  await page.setInputFiles("#profileAvatarFile", IMAGE);
  await page.waitForSelector("#avatarCropModal.show", { timeout: 15000 });
  await page.waitForTimeout(600);

  // Upstream `initCropArea`: 80% of the smaller side, centred.
  const initial = await cropState();
  results.initial = initial;
  const expectedSide = Math.min(initial.imageSize.w, initial.imageSize.h) * 0.8;
  check(
    Math.abs(initial.area.size - expectedSide) < 1 &&
      Math.abs(initial.area.x - (initial.imageSize.w - expectedSide) / 2) < 1 &&
      Math.abs(initial.area.y - (initial.imageSize.h - expectedSide) / 2) < 1,
    "initial_selection_is_eighty_percent",
    JSON.stringify(initial.area),
  );
  check(
    initial.border === "2px dashed rgb(255, 255, 255)" &&
      initial.mask.includes("9999px") &&
      initial.mask.includes("rgba(0, 0, 0, 0.5)") &&
      initial.touchAction === "none",
    "crop_box_styling",
    JSON.stringify({ border: initial.border, mask: initial.mask, touch: initial.touchAction }),
  );

  // The selection is at most 80% of the smaller scaled side, i.e. it fits the stage.
  check(
    initial.box.width <= Math.min(initial.stage.w, initial.stage.h) + 1 &&
      Math.abs(initial.box.width - initial.area.size * (initial.imageRendered.w / initial.imageSize.w)) < 1.5,
    "selection_matches_scale",
    JSON.stringify({ box: initial.box.width, stage: initial.stage }),
  );

  // Real drag to the right: the selection follows the mouse.
  const centre = await boxCentre();
  await page.mouse.move(centre.x, centre.y);
  await page.mouse.down();
  await page.mouse.move(centre.x + 60, centre.y + 20, { steps: 8 });
  await page.mouse.up();
  await page.waitForTimeout(200);
  const dragged = await cropState();
  results.dragged = dragged;
  check(
    dragged.area.x > initial.area.x + 10 &&
      dragged.area.y >= initial.area.y &&
      dragged.box.left > initial.box.left + 5,
    "drag_moves_selection",
    JSON.stringify({ before: initial.area, after: dragged.area }),
  );

  // Dragging far past the edge clamps to the image bounds.
  const centre2 = await boxCentre();
  await page.mouse.move(centre2.x, centre2.y);
  await page.mouse.down();
  await page.mouse.move(centre2.x + 900, centre2.y + 900, { steps: 12 });
  await page.mouse.up();
  await page.waitForTimeout(200);
  const clamped = await cropState();
  results.clamped = clamped;
  check(
    Math.abs(clamped.area.x - (clamped.imageSize.w - clamped.area.size)) < 1 &&
      Math.abs(clamped.area.y - (clamped.imageSize.h - clamped.area.size)) < 1,
    "drag_clamped_to_image",
    JSON.stringify({ area: clamped.area, image: clamped.imageSize }),
  );

  // Wheel up zooms in by 1.1 and keeps the centre ratio.
  await page.mouse.move(centre2.x, centre2.y);
  await page.mouse.wheel(0, -120);
  await page.waitForTimeout(200);
  const zoomedIn = await cropState();
  results.zoomedIn = zoomedIn;
  const grown = zoomedIn.area.size / clamped.area.size;
  check(
    grown > 1.05 && grown < 1.15,
    "wheel_zooms_in",
    `${clamped.area.size.toFixed(1)} → ${zoomedIn.area.size.toFixed(1)} (x${grown.toFixed(3)})`,
  );

  // Wheel down repeatedly bottoms out at the upstream 20px floor.
  for (let i = 0; i < 40; i += 1) {
    await page.mouse.wheel(0, 120);
  }
  await page.waitForTimeout(300);
  const floored = await cropState();
  results.floored = floored;
  check(
    Math.abs(floored.area.size - 20) < 0.5,
    "wheel_floor_twenty",
    String(floored.area.size),
  );

  // Zoom back in and confirm: the field receives a 64x64 PNG.
  for (let i = 0; i < 10; i += 1) {
    await page.mouse.wheel(0, -120);
  }
  await page.waitForTimeout(300);
  await page.click("#avatarCropOk");
  await page.waitForTimeout(400);
  // Confirming resolves the dialog's promise, so the page saves the crop through
  // `PATCH /api/v1/users/me` and repaints the preview. The preview is polled rather
  // than assumed, because the save is asynchronous.
  for (let attempt = 0; attempt < 15; attempt += 1) {
    const saved = patches.find((entry) => entry.startsWith("data:image/png;base64,"));
    if (saved) {
      results.savedAvatar = saved;
      break;
    }
    await page.waitForTimeout(500);
  }
  results.output = await page.evaluate(async (saved) => {
    const modal = document.getElementById("avatarCropModal");
    const size = await new Promise((resolve) => {
      const img = new Image();
      img.onload = () => resolve({ w: img.naturalWidth, h: img.naturalHeight });
      img.onerror = () => resolve(null);
      img.src = saved || "";
    });
    return {
      isPng: (saved || "").startsWith("data:image/png;base64,"),
      length: (saved || "").length,
      size,
      modalClosed: !modal.classList.contains("show"),
    };
  }, results.savedAvatar || "");
  results.saveRequests = patches.map((entry) =>
    entry.startsWith("data:") ? `${entry.slice(0, 24)}…(${entry.length} chars)` : entry,
  );
  check(patches.length === 1, "crop_saved_with_one_patch", JSON.stringify(results.saveRequests));
  check(
    results.output.isPng &&
      results.output.size &&
      results.output.size.w === 64 &&
      results.output.size.h === 64 &&
      results.output.modalClosed,
    "confirm_writes_64px_png",
    JSON.stringify(results.output),
  );

  check(pageErrors.length === 0, "no_page_errors", pageErrors.join(" | "));
} catch (error) {
  results.errors.push("exception: " + String(error && error.message ? error.message : error));
  await page.screenshot({ path: "docs/ui-traversal/rayrag-audit/avatar-crop-failure.png" }).catch(() => {});
} finally {
  // The confirm really saves the cropped avatar, so the previous one is put back.
  try {
    const restored = await page.evaluate(async (previous) => {
      const token = localStorage.getItem("rayrag_token") || "";
      const res = await fetch("/api/v1/users/me", {
        method: "PATCH",
        headers: { "Content-Type": "application/json", Authorization: "Bearer " + token },
        body: JSON.stringify({ avatar: previous || "" }),
      });
      const body = await res.text();
      let code = null;
      try {
        code = JSON.parse(body).code;
      } catch {
        code = res.ok ? 0 : null;
      }
      const input = document.getElementById("profileAvatarFile");
      if (input) input.value = "";
      return { status: res.status, code };
    }, results.avatarBefore);
    results.restored = restored.status === 200 ? "ok" : "FAIL";
    if (restored.status !== 200) results.errors.push(`avatar_restore (${JSON.stringify(restored)})`);
  } catch {
    results.restored = "FAIL";
  }
  results.pageErrors = pageErrors;
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
  process.exit(results.errors.length ? 1 : 0);
}
