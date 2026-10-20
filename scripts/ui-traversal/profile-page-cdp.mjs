// user-setting Profile page: verify the upstream row composition, the avatar
// upload states (plus button -> cropped preview -> remove), the localized copy
// (en + zh) and the edit dialogs.
//
// Usage: UI_DUMP_PASSWORD=... node scripts/ui-traversal/profile-page-cdp.mjs
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
page.on("response", async (res) => {
  if (res.url().includes("/api/v1/users/me") && res.request().method() === "PATCH") {
    out.patches.push({ status: res.status(), body: String(await res.text().catch(() => "")).slice(0, 160) });
  }
});

const out = { steps: [], dialogs: [], patches: [] };
page.on("dialog", async (dialog) => {
  out.dialogs.push(dialog.message());
  await dialog.dismiss().catch(() => {});
});
const snap = async (label) => {
  const state = await page.evaluate(() => {
    const preview = document.getElementById("profileAvatarPreview");
    const add = document.getElementById("profileAvatarAdd");
    const img = document.getElementById("profileAvatar");
    return {
      title: (document.querySelector(".profile-title")?.textContent || "").trim(),
      description: (document.querySelector(".profile-description")?.textContent || "").trim(),
      rowLabels: Array.from(document.querySelectorAll(".profile-row > label")).map((l) => l.textContent.trim()),
      editButtons: Array.from(document.querySelectorAll(".profile-edit-btn")).map((b) => b.textContent.trim()),
      nickname: (document.getElementById("profileNickname")?.textContent || "").trim(),
      timezone: (document.getElementById("profileTimezone")?.textContent || "").trim(),
      email: (document.getElementById("profileEmail")?.textContent || "").trim(),
      emailHint: (document.querySelector(".profile-value-stacked .user-setting-description")?.textContent || "").trim(),
      avatarTip: (document.querySelector(".avatar-upload-tip")?.textContent || "").trim(),
      addVisible: add ? !add.hidden : null,
      previewVisible: preview ? !preview.hidden : null,
      imgSrcPrefix: img && img.src ? img.src.slice(0, 22) : "",
      removeVisible: !!document.getElementById("profileAvatarRemove"),
      dialogTitle: (document.getElementById("profileEditTitle")?.textContent || "").trim(),
    };
  });
  out.steps.push({ label, ...state });
  await page.screenshot({ path: `${SHOT}/rayrag-profile-${out.steps.length}-${label}.png` });
  return state;
};

try {
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const ok = await page.evaluate(
    async ({ password }) => {
      const res = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email: "admin@rayrag.local", password }),
      });
      const data = await res.json();
      if (data.code === 0 && data.data?.access_token) {
        localStorage.setItem("rayrag_token", data.data.access_token);
        return true;
      }
      return false;
    },
    { password: PASSWORD },
  );
  if (!ok) throw new Error("login failed");
  const cdp = await ctx.newCDPSession(page);
  await cdp.send("Network.clearBrowserCache").catch(() => {});

  await page.goto(`${ORIGIN}/user-setting/profile`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(2000);
  await snap("initial");

  // Upload a 96x64 PNG through the hidden file input (cropped to 64x64 client side).
  await page.setInputFiles(
    "#profileAvatarFile",
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
  // The shared avatar-upload crop dialog opens on pick; confirm the 64x64 crop.
  await page.waitForFunction(
    () => document.getElementById("avatarCropModal")?.classList.contains("show"),
    { timeout: 15000 },
  );
  await page.click("#avatarCropOk");
  await page.waitForTimeout(1200);
  const afterUpload = await snap("uploaded");
  out.uploadedSrc = afterUpload.imgSrcPrefix;
  out.uploadedIsPng = afterUpload.imgSrcPrefix.startsWith("data:image/png");

  // Remove through the circular X button.
  await page.evaluate(() => document.getElementById("profileAvatarRemove").click());
  await page.waitForTimeout(2500);
  out.serverAvatarAfterRemove = await page.evaluate(async () => {
    const res = await fetch("/api/v1/user/info");
    const data = await res.json();
    return String((data.data || {}).avatar || "").slice(0, 24);
  });
  await snap("removed");

  // Reject a non-image extension (upstream /\.(jpg|jpeg|png|webp|bmp)$/i).
  await page.setInputFiles(
    "#profileAvatarFile",
    { name: "notes.txt", mimeType: "text/plain", buffer: Buffer.from("nope") },
    { timeout: 8000 },
  );
  await page.waitForTimeout(600);

  // Edit dialogs still work (name / timezone / password titles).
  for (const [index, expected] of [
    [0, "Edit Name"],
    [1, "Edit Time Zone"],
    [2, "Edit Password"],
  ]) {
    await page.evaluate((i) => document.querySelectorAll(".profile-edit-btn")[i].click(), index);
    await page.waitForTimeout(300);
    await page.waitForTimeout(300);
    const title = await page.evaluate(() => (document.getElementById("profileEditTitle")?.textContent || "").trim());
    out.steps.push({ label: `dialog-${index}`, expected, dialogTitle: title });
    await page.evaluate(() => {
      const btn = document.querySelector("#profileEditCancel, button[onclick='closeProfileEdit()']");
      if (btn) btn.click();
    });
    await page.waitForTimeout(200);
  }

  // The profile name rules (profile/constants.ts) reject the two upstream
  // messages client-side before any PATCH is sent.
  await page.evaluate(() => document.querySelectorAll(".profile-edit-btn")[0].click());
  await page.waitForTimeout(300);
  await page.fill("#profileEditName", "x".repeat(101));
  await page.evaluate(() => document.getElementById("profileEditSave").click());
  await page.waitForTimeout(300);
  out.nameMaxError = await page.evaluate(
    () => document.getElementById("profileEditNameError").textContent,
  );
  await page.fill("#profileEditName", "bad<name>");
  await page.evaluate(() => document.getElementById("profileEditSave").click());
  await page.waitForTimeout(300);
  out.nameCharsetError = await page.evaluate(
    () => document.getElementById("profileEditNameError").textContent,
  );
  await page.evaluate(() => document.getElementById("profileEditDialog").classList.remove("show"));

  // zh copy.
  const cdpZh = await ctx.newCDPSession(page);
  await cdpZh.send("Network.setCookie", { name: "lang", value: "zh-CN", url: ORIGIN }).catch(() => {});
  await page.goto(`${ORIGIN}/user-setting/profile`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(1500);
  await snap("zh");
  await cdpZh.send("Network.setCookie", { name: "lang", value: "en", url: ORIGIN }).catch(() => {});
} catch (error) {
  out.error = String(error && error.message ? error.message : error).slice(0, 300);
}
out.pageErrors = pageErrors;
console.log(JSON.stringify(out, null, 1));
await page.close().catch(() => {});
process.exit(0);
