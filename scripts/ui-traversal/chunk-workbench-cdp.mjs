// CDP probe for the chunk workbench subtree
// (`web/src/pages/chunk/**`): `/chunk`, `/chunk/parsed/chunks`,
// `/chunk/chunk/:id` and `/chunk/result/:id`. It creates a throwaway
// knowledge base with one document, parses it so real chunks exist, then
// exercises the header, the segmented control, the panel toolbars (copy /
// export), the `Active` switch and `Save` with real clicks, and finally
// removes the probe dataset.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/chunk-workbench-cdp.mjs
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
const KB_NAME = `cdp-chunk-probe-${Date.now()}`;

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
const pageErrors = [];
page.on("pageerror", (error) => pageErrors.push(String(error.message)));
await page.setViewportSize({ width: 1440, height: 1000 });
const results = { steps: [], errors: [], pageErrors };
const shots = "docs/ui-traversal/rayrag-audit";
mkdirSync(shots, { recursive: true });

try {
  await ctx.addCookies([
    { name: "lang", value: "en", domain: "127.0.0.1", path: "/" },
  ]);
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const login = await page.evaluate(async (password) => {
    const res = await fetch("/api/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "admin@rayrag.local", password }),
    });
    const data = await res.json();
    if (data.code !== 0) return { ok: false };
    localStorage.setItem("rayrag_token", data.data.access_token);
    return { ok: true };
  }, PASSWORD);
  if (!login.ok) throw new Error("login failed");

  // Probe dataset + document + chunks, straight through the public API.
  const seeded = await page.evaluate(async (name) => {
    const created = await fetch("/api/v1/datasets", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name }),
    }).then((response) => response.json());
    if (created.code !== 0) return { ok: false, step: "dataset", body: created };
    const kbId = created.data.id;
    const form = new FormData();
    form.append(
      "file",
      new Blob(
        [
          "RayRAG chunk workbench probe.\n\nSecond paragraph with more text so the naive chunker produces at least two chunks in this probe document.\n\nThird paragraph.",
        ],
        { type: "text/plain" },
      ),
      "cdp-chunk-probe.txt",
    );
    const uploaded = await fetch("/api/v1/datasets/" + kbId + "/documents", {
      method: "POST",
      body: form,
    }).then((response) => response.json());
    if (uploaded.code !== 0) return { ok: false, step: "upload", body: uploaded };
    const docId = (uploaded.data || []).length
      ? uploaded.data[0].id
      : uploaded.data && uploaded.data.id;
    await fetch(`/api/v1/datasets/${kbId}/chunks`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ document_ids: [docId] }),
    });
    // Wait for the parser so the panels have real chunks to render.
    for (let attempt = 0; attempt < 40; attempt += 1) {
      const docs = await fetch(`/api/v1/datasets/${kbId}/documents`).then(
        (response) => response.json(),
      );
      const doc = (docs.data || [])[0] || {};
      if (doc.run === "DONE") break;
      await new Promise((resolve) => setTimeout(resolve, 500));
    }
    return { ok: true, kbId, docId };
  }, KB_NAME);
  results.seeded = seeded;
  if (!seeded.ok) throw new Error("probe dataset seeding failed: " + JSON.stringify(seeded.body));

  const query = `knowledgeId=${seeded.kbId}`;
  const readHeader = () =>
    page.evaluate(() => ({
      workbench: !!document.querySelector("[data-testid='chunk-workbench']"),
      breadcrumbAgent: document.querySelector(".chunk-breadcrumb-link")
        ? document.querySelector(".chunk-breadcrumb-link").textContent
        : null,
      breadcrumbPage: document.getElementById("chunkBreadcrumbPage")
        ? document.getElementById("chunkBreadcrumbPage").textContent
        : null,
      segments: Array.from(document.querySelectorAll(".chunk-segment")).map(
        (node) => ({
          label: node.textContent,
          href: node.getAttribute("href"),
          active: node.classList.contains("active"),
          selected: node.getAttribute("aria-selected"),
        }),
      ),
      hasMore: !!document.querySelector("[data-testid='chunk-more']"),
      hasSave: !!document.querySelector("[data-testid='chunk-save']"),
      saveLabel: document.querySelector("[data-testid='chunk-save']")
        ? document.querySelector("[data-testid='chunk-save']").textContent.trim()
        : null,
    }));

  // 1. `/chunk` (header-only shell, like upstream).
  await page.goto(`${ORIGIN}/chunk?${query}`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });
  await page.waitForSelector("[data-testid='chunk-workbench']", { timeout: 10000 });
  results.shell = await readHeader();
  await page.screenshot({ path: `${shots}/chunk-shell-20260919.png` });

  // 2. `/chunk/parsed/chunks` renders the real parsed cards.
  await page.goto(`${ORIGIN}/chunk/parsed/chunks?${query}`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });
  await page.waitForFunction(
    () => document.querySelectorAll("[data-testid='chunk-parsed-card']").length > 0,
    null,
    { timeout: 20000 },
  );
  results.parsed = await page.evaluate(() => ({
    cards: document.querySelectorAll("[data-testid='chunk-parsed-card']").length,
    firstLabel: document.querySelector(".chunk-card-page-label")
      ? document.querySelector(".chunk-card-page-label").textContent
      : null,
    firstContent: document.querySelector(".chunk-card-content")
      ? document.querySelector(".chunk-card-content").textContent.slice(0, 40)
      : null,
    toolbarTitle: document.querySelector(".chunk-toolbar-title")
      ? document.querySelector(".chunk-toolbar-title").textContent
      : null,
    hasCopy: !!document.querySelector("[data-testid='chunk-copy-parsed']"),
    hasExport: !!document.querySelector("[data-testid='chunk-export-parsed']"),
    activeSegment: document.querySelector(".chunk-segment.active")
      ? document.querySelector(".chunk-segment.active").textContent
      : null,
  }));
  await page.screenshot({ path: `${shots}/chunk-parsed-20260919.png` });

  // 3. `/chunk/chunk/:id` renders both panels; toggle the Active switch and
  //    press Save for real.
  await page.goto(`${ORIGIN}/chunk/chunk/${seeded.docId}?${query}`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });
  await page.waitForFunction(
    () => document.querySelectorAll("[data-testid='chunk-result-card']").length > 0,
    null,
    { timeout: 20000 },
  );
  results.chunkResult = await page.evaluate(() => ({
    parsedCards: document.querySelectorAll("[data-testid='chunk-parsed-card']").length,
    chunkedCards: document.querySelectorAll("[data-testid='chunk-result-card']").length,
    toolbars: Array.from(document.querySelectorAll(".chunk-toolbar-title")).map(
      (node) => node.textContent,
    ),
    activeSegment: document.querySelector(".chunk-segment.active").textContent,
    hasIcon: !!document.querySelector(".chunk-card-icon svg"),
    activeLabel: document.querySelector(".chunk-card-active")
      ? document.querySelector(".chunk-card-active").textContent
      : null,
  }));
  const before = await page.evaluate(
    () =>
      document.querySelector(".chunk-card-switch").checked,
  );
  await page.click(".chunk-card-switch");
  await page.waitForTimeout(300);
  await page.click("[data-testid='chunk-save']");
  // Wait for the PATCH round-trip to settle instead of sampling the
  // intermediate "Saving…" label.
  await page.waitForFunction(
    () => {
      const node = document.getElementById("chunkStatus");
      if (!node) return false;
      const text = node.textContent || "";
      return text.length > 0 && !text.includes("Saving");
    },
    null,
    { timeout: 20000 },
  );
  results.save = await page.evaluate(() => ({
    status: document.getElementById("chunkStatus").textContent,
    switchChecked: document.querySelector(".chunk-card-switch").checked,
  }));
  // Reload: the persisted value must survive.
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.waitForFunction(
    () => document.querySelectorAll("[data-testid='chunk-result-card']").length > 0,
    null,
    { timeout: 20000 },
  );
  results.persisted = await page.evaluate(
    () => document.querySelector(".chunk-card-switch").checked,
  );
  results.switchFlipped = before !== results.persisted;
  // Flip it back so the probe leaves the chunk as it found it.
  await page.click(".chunk-card-switch");
  await page.waitForTimeout(200);
  await page.click("[data-testid='chunk-save']");
  await page.waitForTimeout(1200);
  await page.screenshot({ path: `${shots}/chunk-chunk-20260919.png` });

  // 4. `/chunk/result/:id` and the ellipsis menu.
  await page.goto(`${ORIGIN}/chunk/result/${seeded.docId}?${query}`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });
  await page.waitForSelector("[data-testid='chunk-workbench']", { timeout: 10000 });
  await page.click("[data-testid='chunk-more']");
  await page.waitForTimeout(300);
  results.menu = await page.evaluate(() => ({
    hidden: document.getElementById("chunkMenu").hidden,
    items: Array.from(document.querySelectorAll("#chunkMenu button")).map(
      (node) => node.textContent,
    ),
    activeSegment: document.querySelector(".chunk-segment.active").textContent,
    panels: document.querySelectorAll(".chunk-panel").length,
  }));
  await page.screenshot({ path: `${shots}/chunk-result-20260919.png` });

  // 5. Export downloads real JSON.
  await page.goto(`${ORIGIN}/chunk/parsed/chunks?${query}`, {
    waitUntil: "domcontentloaded",
    timeout: 30000,
  });
  await page.waitForFunction(
    () => document.querySelectorAll("[data-testid='chunk-parsed-card']").length > 0,
    null,
    { timeout: 20000 },
  );
  const download = await Promise.all([
    page.waitForEvent("download", { timeout: 15000 }),
    page.click("[data-testid='chunk-export-parsed']"),
  ]).then(([event]) => event);
  results.export = { suggested: download.suggestedFilename() };
} catch (error) {
  results.errors.push(String(error && error.stack ? error.stack : error));
  await page.screenshot({ path: `${shots}/chunk-workbench-error-20260919.png` });
} finally {
  // Always remove the probe dataset, even when a step failed.
  if (results.seeded && results.seeded.ok) {
    const cleanup = await page
      .evaluate(async (kbId) => {
        const response = await fetch("/api/v1/datasets/" + kbId, {
          method: "DELETE",
        });
        const data = await response.json().catch(() => ({}));
        return { ok: response.ok && data.code === 0, message: data.message || null };
      }, results.seeded.kbId)
      .catch((error) => ({ ok: false, message: String(error) }));
    results.cleanup = cleanup;
  }
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
}
if (results.errors.length || results.pageErrors.length) process.exit(1);
