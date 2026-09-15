// Available-models pills + LocalLlmConfigs modals: check the count pills render
// like upstream (label and count adjacent), then open the non-picker local
// provider modals (ModelScope/HuggingFace/GPUStack) and the OpenRouter
// provider_order field, plus the picker factories hiding the generic Vision.
//
// Usage: UI_DUMP_PASSWORD=... node scripts/ui-traversal/local-provider-modal-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
const PASSWORD = process.env.UI_DUMP_PASSWORD;
const SHOT = process.env.SHOT_DIR || "/tmp";

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1600, height: 1100 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 180)));

const out = { providers: [], steps: [] };

const inspectModal = () =>
  page.evaluate(() => {
    const visible = (id) => {
      const el = document.getElementById(id);
      return el ? !el.hidden : null;
    };
    const value = (id) => {
      const el = document.getElementById(id);
      return el ? el.value : null;
    };
    const baseField = document.getElementById("piBaseField");
    return {
      title: (document.getElementById("providerInstanceTitle")?.textContent || "").trim(),
      baseLabel: baseField && baseField.firstChild ? String(baseField.firstChild.nodeValue || "").trim() : "",
      baseVisible: visible("piBaseField"),
      keyVisible: visible("piKeyField"),
      visionVisible: visible("piVisionField"),
      providerOrderVisible: visible("piProviderOrderField"),
      pickerVisible: visible("piModelPicker"),
      typeChecks: Array.from(document.querySelectorAll("#piTypeFields input[type=checkbox]")).map((i) => i.value),
      typeChecked: Array.from(document.querySelectorAll("#piTypeFields input[type=checkbox]:checked")).map((i) => i.value),
      modelName: value("pf_model_name"),
      maxTokens: value("pf_max_tokens"),
      toolsVisible: visible("pf_is_tools"),
      toolsChecked: document.getElementById("pf_is_tools")?.checked ?? null,
      visionFieldVisible: visible("pf_vision"),
      payload:
        typeof providerSpecialPayload === "function" && document.getElementById("piExtraFields")
          ? (() => { try { return providerSpecialPayload(); } catch (e) { return { error: String(e) }; } })()
          : null,
    };
  });

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

  await page.goto(`${ORIGIN}/user-setting/model`, { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(2200);

  out.pills = await page.evaluate(() => {
    const all = document.querySelector(".provider-chip");
    const counts = Array.from(document.querySelectorAll(".provider-chip")).map((chip) => (chip.textContent || "").trim());
    return {
      allText: (all?.textContent || "").trim(),
      allCount: (document.getElementById("providerCountAll")?.textContent || "").trim(),
      counts: counts.slice(0, 8),
    };
  });
  await page.screenshot({ path: `${SHOT}/rayrag-local-providers-pills.png` });

  for (const provider of ["ModelScope", "HuggingFace", "GPUStack", "Ollama", "OpenRouter"]) {
    const card = page.locator(`[data-testid='available-model-card'][data-provider='${provider}']`).first();
    if (!(await card.count())) {
      out.providers.push({ provider, missing: true });
      continue;
    }
    await card.scrollIntoViewIfNeeded();
    await card.locator(".available-provider-add").first().click({ timeout: 8000 });
    await page.waitForTimeout(700);
    const state = await inspectModal();
    out.providers.push({ provider, ...state });
    await page.screenshot({ path: `${SHOT}/rayrag-local-provider-${provider}.png` });
    if (provider === "ModelScope") {
      // `onTypes` gating: the vision/tools switches only appear once a matching
      // model type is selected (upstream `shouldRender` dependencies).
      await page.locator("#piTypeFields input[value='chat']").check({ timeout: 8000 });
      await page.waitForTimeout(250);
      out.providers.push({ provider: `${provider}-chat-on`, ...(await inspectModal()) });
      await page.locator("#pf_vision").click({ timeout: 8000 });
      await page.locator("#pf_model_name").fill("modelscope-model");
      out.providers.push({ provider: `${provider}-vision-on`, ...(await inspectModal()) });
      const tools = page.locator("#pf_is_tools");
      if (await tools.count()) {
        await tools.click({ timeout: 8000 });
        out.providers.push({ provider: `${provider}-tools-on`, ...(await inspectModal()) });
      }
    }
    await page.locator("button[onclick='closeProviderInstanceModal()']").first().click({ timeout: 8000 }).catch(async () => {
      await page.keyboard.press("Escape");
    });
    await page.waitForTimeout(300);
  }
  out.steps = out.providers.map((p) => p.provider);
} catch (error) {
  out.error = String(error && error.message ? error.message : error).slice(0, 300);
}
out.pageErrors = pageErrors;
console.log(JSON.stringify(out, null, 1));
await page.close().catch(() => {});
process.exit(0);
