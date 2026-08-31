import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const ZH_DESCS = {
  clickclack: "连接 ClickClack 机器人",
  discord: "连接 Discord 机器人",
  dingtalk: "连接钉钉机器人",
  feishu: "连接飞书 / Lark 机器人",
  googlechat: "连接 Google Chat 机器人",
  irc: "连接 IRC 服务器",
  line: "连接 LINE 消息机器人",
  matrix: "连接 Matrix 机器人",
  mattermost: "连接 Mattermost 机器人",
  msteams: "连接 Microsoft Teams 机器人",
  nextcloud_talk: "连接 Nextcloud Talk 机器人",
  nostr: "连接 Nostr 机器人",
  qqbot: "连接 QQ 机器人",
  slack: "连接 Slack 机器人",
  synology_chat: "连接群晖 Chat 机器人",
  telegram: "连接 Telegram 机器人",
  tlon: "连接 Tlon (Urbit) 机器人",
  twitch: "连接 Twitch 聊天机器人",
  wecom: "连接企业微信机器人",
  whatsapp: "连接 WhatsApp 机器人（扫码配对）",
  yuanbao: "连接腾讯元宝机器人",
  zalo: "连接 Zalo 机器人",
  zalouser: "连接个人 Zalo 账号",
};

const ORIGIN = process.env.RAYRAG_CDP_ORIGIN || "http://127.0.0.1:9390";
const EMAIL = process.env.RAYRAG_LOGIN_EMAIL || "admin@rayrag.local";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD;
if (!PASSWORD) {
  console.error(
    "RAYRAG_LOGIN_PASSWORD is required (sourced from the operator env, not committed)",
  );
  process.exit(2);
}

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1440, height: 900 });
const results = { steps: [], errors: [], closes: [] };
try {
  await ctx.addCookies([
    {
      name: "lang",
      value: "zh-CN",
      domain: "127.0.0.1",
      path: "/",
      httpOnly: false,
    },
  ]);
  await page.goto(ORIGIN + "/login", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  const login = await page.evaluate(
    async ({ email, password }) => {
      const res = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email, password }),
      });
      const data = await res.json();
      if (data.code !== 0 || !data.data || !data.data.access_token) {
        return { ok: false, status: res.status, body: data };
      }
      localStorage.setItem("rayrag_token", data.data.access_token);
      return { ok: true };
    },
    { email: EMAIL, password: PASSWORD },
  );
  results.steps.push("login:" + JSON.stringify(login));
  if (!login.ok)
    throw new Error(
      "login failed: " + JSON.stringify(login.body || login).slice(0, 200),
    );

  await page.goto(ORIGIN + "/user-setting/chat-channel", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForTimeout(1500);
  results.steps.push("loaded:" + page.url());

  const gridReady = await page.evaluate(() => {
    const cards = document.querySelectorAll(
      "#availableChannelGrid .cc-available-card",
    );
    return {
      count: cards.length,
      zhMap: typeof CC_DESC_ZH !== "undefined",
      title: document.querySelector("h1")
        ? document.querySelector("h1").textContent
        : "",
    };
  });
  results.steps.push("grid:" + JSON.stringify(gridReady));

  const cards = await page
    .locator("#availableChannelGrid .cc-available-card")
    .all();
  if (cards.length !== 23)
    throw new Error("expected 23 available cards, got " + cards.length);

  const mismatches = [];
  const titles = [];
  for (const card of cards) {
    const channel = await card.getAttribute("data-channel");
    const desc = await card.locator("p.cc-available-desc").textContent();
    if (desc !== ZH_DESCS[channel]) mismatches.push(channel + "=" + desc);
    // Simulated user click: open the add dialog, then close it again.
    await card.click();
    await page.waitForSelector("#chatChannelDialog.show", { timeout: 5000 });
    const title = await page.locator("#chatChannelDialogTitle").textContent();
    titles.push(channel + " -> " + title);
    try {
      await page
        .locator("#chatChannelDialog .provider-instance-actions .btn-o")
        .first()
        .click({ timeout: 8000 });
      results.closes.push("click");
    } catch {
      // Small/off-screen window can leave the modal button outside the
      // viewport; fall back to the same handler the button invokes.
      await page.evaluate(() =>
        document.getElementById("chatChannelDialog").classList.remove("show"),
      );
      results.closes.push("fallback");
    }
    await page.waitForSelector("#chatChannelDialog.show", {
      state: "hidden",
      timeout: 5000,
    });
  }
  results.titles = titles;
  results.mismatches = mismatches;
  results.dialogOpens = titles.length;

  // zh screenshot of the traversed page.
  mkdirSync("docs/ui-traversal/rayrag-audit", { recursive: true });
  await page.screenshot({
    path: "docs/ui-traversal/rayrag-audit/chat-channel-zh-desc-20260831.png",
    fullPage: false,
  });
  results.screenshot =
    "docs/ui-traversal/rayrag-audit/chat-channel-zh-desc-20260831.png";

  // en spot-check: lang=en must not carry the zh map and must show the en desc.
  await ctx.addCookies([
    {
      name: "lang",
      value: "en",
      domain: "127.0.0.1",
      path: "/",
      httpOnly: false,
    },
  ]);
  await page.reload({ waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(1200);
  const en = await page.evaluate(() => ({
    hasZhMap: typeof CC_DESC_ZH !== "undefined",
    firstDesc:
      (
        document.querySelector(
          "#availableChannelGrid .cc-available-card p.cc-available-desc",
        ) || {}
      ).textContent || "",
    firstChannel: (
      document.querySelector("#availableChannelGrid .cc-available-card") || {
        dataset: {},
      }
    ).dataset?.channel,
  }));
  results.en = en;

  const ok =
    mismatches.length === 0 &&
    en.hasZhMap === false &&
    en.firstDesc === "Connect a ClickClack bot";
  results.result = ok ? "PASS" : "FAIL";
} catch (e) {
  results.errors.push(String(e && e.message ? e.message : e));
  results.result = "FAIL";
}
console.log(JSON.stringify(results, null, 2));
await page.close();
await b.close();
process.exit(results.result === "PASS" ? 0 : 1);
