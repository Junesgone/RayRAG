// Compilation-template editor traversal (internal verification script, not published).
// Drives the RAGFlow v0.27.2 edit-next deep form like a user: kind-driven blocks,
// section field add/edit/delete, blueprint presets, JSON preview and save/reload.
//
// Usage:
//   RAYRAG_UI_ORIGIN=http://127.0.0.1:9490 RAYRAG_CDP_ENDPOINT=http://127.0.0.1:16002 //   node scripts/ui-traversal/compilation-template-editor-cdp.mjs
import { chromium } from "playwright-core";

const CDP_EP = process.env.RAYRAG_CDP_ENDPOINT || "http://127.0.0.1:16002";
const ORIGIN = process.env.RAYRAG_UI_ORIGIN || "http://127.0.0.1:9390";
const EMAIL = process.env.RAYRAG_LOGIN_EMAIL || "ui-probe-47@example.com";
const PASSWORD = process.env.RAYRAG_LOGIN_PASSWORD || "Probe-pass-123";
const SHOT_DIR = process.env.SHOT_DIR || "/tmp";

const checks = [];
function check(name, condition, detail) {
  checks.push({ name, ok: !!condition, detail: detail ?? "" });
  if (!condition) console.error("FAIL", name, detail ?? "");
}

const browser = await chromium.connectOverCDP(CDP_EP);
const ctx = browser.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1500, height: 1000 });
const pageErrors = [];
page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 300)));

const api = async (path, init) =>
  page.evaluate(
    async ({ path, init }) => {
      const token = localStorage.getItem("rayrag_token") || "";
      const response = await fetch(path, {
        ...(init || {}),
        headers: {
          "Content-Type": "application/json",
          Authorization: "Bearer " + token,
          ...((init && init.headers) || {}),
        },
      });
      let body = null;
      try {
        body = await response.json();
      } catch (e) {
        body = null;
      }
      return { status: response.status, body };
    },
    { path, init },
  );

const visible = async (selector) =>
  page.locator(selector).first().isVisible().catch(() => false);
const count = async (selector) => page.locator(selector).count();

let created = [];
try {
  await page.goto(ORIGIN + "/login", { waitUntil: "domcontentloaded", timeout: 40000 });
  const loggedIn = await page.evaluate(
    async ({ email, password }) => {
      const response = await fetch("/api/v1/auth/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email, password }),
      });
      const data = await response.json();
      if (data.code === 0 && data.data && data.data.access_token) {
        localStorage.setItem("rayrag_token", data.data.access_token);
        return true;
      }
      return false;
    },
    { email: EMAIL, password: PASSWORD },
  );
  check("login", loggedIn);
  if (!loggedIn) throw new Error("login failed");

  // The palette/presets the editor consumes.
  const builtins = await api("/api/v1/compilation-templates/builtins");
  check("builtins code", builtins.body && builtins.body.code === 0, JSON.stringify(builtins.body).slice(0, 200));
  const members = (builtins.body && builtins.body.data) || [];
  check("builtins count is 8", members.length === 8, String(members.length));
  check(
    "builtin order is upstream",
    members.map((item) => item.id).join(",") ===
      "knowledge_graph,timeline,mind_map,page_index,session_essence,session_graph,tree,wiki",
    members.map((item) => item.id).join(","),
  );
  const wikiBuiltin = members.find((item) => item.id === "wiki") || {};
  check("wiki kind renamed", wikiBuiltin.kind === "wiki", String(wikiBuiltin.kind));
  check("wiki mode seeded", wikiBuiltin.config && wikiBuiltin.config.mode === "entity");
  const presets = await api("/api/v1/compilation-templates/wiki-presets");
  const presetRows = (presets.body && presets.body.data) || [];
  check("wiki presets count is 6", presetRows.length === 6, String(presetRows.length));
  check(
    "wiki presets carry example",
    presetRows.every((row) => typeof row.example === "string" && row.example.length > 0),
  );

  // Create-mode page.
  await page.goto(ORIGIN + "/compilation-templates/edit-next", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(1200);
  await page.waitForFunction(
    () => !!document.querySelector("#ctKind option[value='wiki']"),
    null,
    { timeout: 15000 },
  );
  const kindValues = await page.$$eval("#ctKind option", (options) =>
    options.map((option) => option.value).filter(Boolean),
  );
  check(
    "kind options exclude session kinds",
    kindValues.join(",") === "knowledge_graph,mind_map,page_index,timeline,tree,wiki",
    kindValues.join(","),
  );
  check("tree block hidden by default", !(await visible("[data-testid='compilation-template-raptor']")));
  check("blueprint hidden by default", !(await visible("[data-testid='compilation-template-blueprint']")));
  check("mode hidden by default", !(await visible("[data-testid='compilation-template-wiki-mode']")));

  await page.selectOption("#ctKind", "wiki");
  await page.waitForTimeout(400);
  check("wiki mode visible", await visible("[data-testid='compilation-template-wiki-mode']"));
  check("blueprint visible for wiki", await visible("[data-testid='compilation-template-blueprint']"));
  const tabs = await page.$$eval("[data-testid='compilation-template-section-tab']", (nodes) =>
    nodes.map((node) => node.textContent.trim()),
  );
  check(
    "wiki section tabs",
    tabs.join("|") === "Entity specification|Relation specification|Claim specification|Concept specification",
    tabs.join("|"),
  );
  const entityCards = await count("[data-testid='compilation-template-field-card']");
  check("wiki entity field cards", entityCards === 8, String(entityCards));
  await page.screenshot({ path: SHOT_DIR + "/rayrag-ct-wiki-form.png", fullPage: true });

  // Relation tab has its own card set.
  await page.click("[data-testid='compilation-template-section-tab'][data-section='relation']");
  await page.waitForTimeout(250);
  const relationCards = await count("[data-testid='compilation-template-field-card']");
  check("relation field cards", relationCards === 7, String(relationCards));

  // Add a custom field, edit it, then delete it.
  await page.click("[data-testid='compilation-template-add-field']");
  await page.waitForTimeout(250);
  check("field modal opens", await visible("[data-testid='compilation-template-field-modal']"));
  check("modal has type input", (await count("#ctField_type")) === 1);
  await page.fill("#ctField_type", "custom_relation");
  await page.fill("#ctField_description", "Custom relation description");
  await page.fill("#ctField_rule", "Custom relation rule");
  await page.click("[data-testid='compilation-template-field-confirm']");
  await page.waitForTimeout(300);
  check("field added", (await count("[data-testid='compilation-template-field-card']")) === 8, String(await count("[data-testid='compilation-template-field-card']")));

  await page.click("[data-testid='compilation-template-field-card'][data-index='7'] [data-testid='compilation-template-field-edit']");
  await page.waitForTimeout(250);
  const editedDescription = await page.inputValue("#ctField_description");
  check("edit modal prefills", editedDescription.includes("Custom relation"), editedDescription);
  await page.fill("#ctField_description", "Custom relation description (edited)");
  await page.click("[data-testid='compilation-template-field-confirm']");
  await page.waitForTimeout(250);
  const editedCard = await page.textContent(
    "[data-testid='compilation-template-field-card'][data-index='7']",
  );
  check("edited card text", editedCard.includes("Custom relation description (edited)"), editedCard);

  await page.click("[data-testid='compilation-template-field-card'][data-index='7'] [data-testid='compilation-template-field-delete']");
  await page.waitForTimeout(250);
  check("field deleted", (await count("[data-testid='compilation-template-field-card']")) === 7);

  // Blueprint presets.
  const presetValues = await page.$$eval("#ctBlueprintPreset option", (options) =>
    options.map((option) => option.value),
  );
  check(
    "blueprint preset options",
    presetValues.join(",") === "brand,engineering,general,market,product,user_interview,__custom__",
    presetValues.join(","),
  );
  await page.selectOption("#ctBlueprintPreset", "brand");
  await page.waitForTimeout(250);
  const brandInstruction = await page.inputValue("#ctInstruction");
  const brandExample = await page.inputValue("#ctExample");
  check("brand preset fills instruction", brandInstruction.includes("encyclopedic article"), brandInstruction.slice(0, 60));
  check("brand preset fills example", brandExample.length > 100, String(brandExample.length));
  const customInstruction = wikiBuiltin.config.instruction;
  await page.selectOption("#ctBlueprintPreset", "__custom__");
  await page.waitForTimeout(250);
  check(
    "custom preset restores builtin instruction",
    (await page.inputValue("#ctInstruction")) === customInstruction,
  );
  await page.selectOption("#ctBlueprintPreset", "brand");
  await page.waitForTimeout(200);

  // JSON preview renders the form state.
  await page.click("[data-testid='compilation-template-json-preview']");
  await page.waitForTimeout(300);
  check("json sheet opens", await visible("[data-testid='compilation-template-json-sheet']"));
  const previewText = await page.textContent("#ctJsonBody");
  let preview = null;
  try {
    preview = JSON.parse(previewText);
  } catch (e) {
    preview = null;
  }
  check("json preview parses", preview !== null);
  check(
    "json preview carries wiki kind",
    preview && preview.templates && preview.templates[0].kind === "wiki",
    previewText.slice(0, 120),
  );
  check(
    "json preview carries entity fields",
    preview && preview.templates[0].config.entity.fields.length === 8,
  );
  await page.screenshot({ path: SHOT_DIR + "/rayrag-ct-json-preview.png" });
  await page.keyboard.press("Escape");
  await page.waitForTimeout(200);
  check("json sheet closes on Escape", !(await visible("[data-testid='compilation-template-json-sheet']")));

  // Save the wiki template (topic mode) and read it back.
  const wikiName = "CDP wiki template " + Date.now();
  await page.fill("#ctName", wikiName);
  await page.selectOption("#ctMode", "topic");
  await page.click("[data-testid='compilation-template-save']");
  await page.waitForURL("**/agents**", { timeout: 20000 });
  check("save redirects to agents", page.url().includes("/agents"), page.url());
  const groups = await api("/api/v1/compilation-template-groups?page=1&page_size=100");
  const groupRows = (groups.body && groups.body.data && groups.body.data.groups) || [];
  const savedWiki = groupRows.find((group) => group.name === wikiName);
  check("saved group exists", !!savedWiki);
  if (savedWiki) {
    created.push(savedWiki.id);
    check("saved wiki scope", savedWiki.scope === "dataset", String(savedWiki.scope));
    const template = (savedWiki.templates && savedWiki.templates[0]) || {};
    check("saved wiki kind", template.kind === "wiki", String(template.kind));
    check("saved wiki mode", template.config && template.config.mode === "topic");
    check(
      "saved wiki blueprint instruction",
      template.config && typeof template.config.instruction === "string" && template.config.instruction.length > 100,
    );
    check(
      "saved wiki sections",
      template.config && template.config.entity.fields.length === 8 && template.config.relation.fields.length === 7,
    );

    // Edit page回填.
    await page.goto(ORIGIN + "/compilation-templates/edit-next/" + savedWiki.id, {
      waitUntil: "domcontentloaded",
      timeout: 40000,
    });
    await page.waitForTimeout(1400);
    check("edit page prefills name", (await page.inputValue("#ctName")) === wikiName);
    check("edit page prefills kind", (await page.inputValue("#ctKind")) === "wiki");
    check("edit page prefills mode", (await page.inputValue("#ctMode")) === "topic");
    check("edit page shows sections", (await count("[data-testid='compilation-template-section-tab']")) === 4);
    const blueprintSelected = await page.inputValue("#ctBlueprintPreset");
    check("edit page matches preset", blueprintSelected === "brand", blueprintSelected);
  }

  // Tree kind: RAPTOR block instead of sections.
  await page.goto(ORIGIN + "/compilation-templates/edit-next", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(1200);
  await page.waitForFunction(
    () => !!document.querySelector("#ctKind option[value='tree']"),
    null,
    { timeout: 15000 },
  );
  await page.selectOption("#ctKind", "tree");
  await page.waitForTimeout(300);
  check("raptor visible for tree", await visible("[data-testid='compilation-template-raptor']"));
  check("sections hidden for tree", !(await visible("[data-testid='compilation-template-fields']")));
  check("blueprint hidden for tree", !(await visible("[data-testid='compilation-template-blueprint']")));
  check("rechunk hidden for tree", !(await visible("#ctRechunkField")));
  check("raptor max token default", (await page.inputValue("#ctRaptorMaxToken")) === "512");
  await page.fill("#ctRaptorMaxToken", "1024");
  await page.fill("#ctRaptorPrompt", "Summarize these paragraphs.");
  await page.check("#ctRaptorRechunk");
  const treeName = "CDP tree template " + Date.now();
  await page.fill("#ctName", treeName);
  await page.click("[data-testid='compilation-template-save']");
  await page.waitForURL("**/agents**", { timeout: 20000 });
  const groups2 = await api("/api/v1/compilation-template-groups?page=1&page_size=100");
  const treeRows = (groups2.body && groups2.body.data && groups2.body.data.groups) || [];
  const savedTree = treeRows.find((group) => group.name === treeName);
  check("saved tree group exists", !!savedTree);
  if (savedTree) {
    created.push(savedTree.id);
    const template = (savedTree.templates && savedTree.templates[0]) || {};
    check("saved tree scope is file", savedTree.scope === "file", String(savedTree.scope));
    check("saved raptor max_token", template.config && template.config.raptor.max_token === 1024, JSON.stringify(template.config && template.config.raptor));
    check("saved raptor rechunk", template.config && template.config.raptor.rechunk === true);
    check("saved tree has no sections", template.config && !template.config.entity);
  }

  // Validation surface: duplicate field type is refused before submit.
  await page.goto(ORIGIN + "/compilation-templates/edit-next", { waitUntil: "domcontentloaded", timeout: 40000 });
  await page.waitForTimeout(1200);
  await page.waitForFunction(
    () => !!document.querySelector("#ctKind option[value='knowledge_graph']"),
    null,
    { timeout: 15000 },
  );
  await page.selectOption("#ctKind", "knowledge_graph");
  await page.waitForTimeout(400);
  check(
    "non-wiki/non-tree kind shows re-chunk",
    await visible("#ctRechunkField"),
  );
  const kgTabs = await page.$$eval("[data-testid='compilation-template-section-tab']", (nodes) =>
    nodes.map((node) => node.textContent.trim()),
  );
  check("knowledge_graph sections", kgTabs.length === 2, kgTabs.join("|"));
  await page.click("[data-testid='compilation-template-add-field']");
  await page.waitForTimeout(200);
  await page.click("[data-testid='compilation-template-field-confirm']");
  await page.waitForTimeout(200);
  check("empty field is refused", await visible("[data-testid='compilation-template-field-modal']"));

  // Cleanup the probe groups.
  for (const id of created) {
    await api("/api/v1/compilation-template-groups/" + encodeURIComponent(id), { method: "DELETE" });
  }
  check("cleanup", true);
  check("no page errors", pageErrors.length === 0, pageErrors.join(" | "));
} catch (error) {
  checks.push({ name: "traversal crashed", ok: false, detail: String(error).slice(0, 400) });
  console.error(error);
} finally {
  await page.close().catch(() => {});
  await browser.close().catch(() => {});
  const failed = checks.filter((entry) => !entry.ok);
  console.log(JSON.stringify({ total: checks.length, failed: failed.length, checks }, null, 2));
  if (failed.length > 0) process.exitCode = 1;
}
