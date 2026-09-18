// CDP probe for the Add-custom-model dialog and the List-models picker of the
// provider modal (upstream `use-list-models-options.tsx`,
// `add-custom-model-dialog.tsx`, `use-custom-model-fields.tsx` and
// `components/ui/multi-select.tsx`). Everything is driven by real clicks and
// typing; the provider instance is never saved, so no server state changes.
//
// Usage: RAYRAG_CDP_ORIGIN=http://127.0.0.1:9390 \
//        RAYRAG_LOGIN_PASSWORD=... node scripts/ui-traversal/provider-custom-model-cdp.mjs
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
const PROBE_MODEL = `cdp-probe-model-${Date.now()}`;

const b = await chromium.connectOverCDP("http://127.0.0.1:16002");
const ctx = b.contexts()[0];
const page = await ctx.newPage();
await page.setViewportSize({ width: 1440, height: 1000 });
const results = { steps: [], errors: [] };
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

  await page.goto(ORIGIN + "/user-setting/model", {
    waitUntil: "domcontentloaded",
    timeout: 40000,
  });
  await page.waitForSelector("[data-testid='available-model-card']", {
    timeout: 15000,
  });

  // Open the provider modal from a card that owns the model picker.
  const opened = await page.evaluate(() => {
    const card = Array.from(
      document.querySelectorAll("[data-testid='available-model-card']"),
    ).find((node) => node.dataset.modelPicker === "true");
    if (!card) return null;
    card.click();
    return { provider: card.dataset.provider, picker: card.dataset.modelPicker };
  });
  results.provider = opened;
  if (!opened) throw new Error("no picker provider card available");
  await page.waitForSelector("#providerInstanceModal.show", { timeout: 10000 });

  // 1. List models: real click on the trigger, then wait for the catalog to
  //    settle before inspecting rows (the discovery request can be slow).
  await page.click("#piListModels");
  await page.waitForFunction(
    () => !document.getElementById("piModelLoading").textContent.includes("Loading"),
    null,
    { timeout: 30000 },
  );
  await page.waitForTimeout(800);
  results.picker = await page.evaluate(() => {
    const rows = Array.from(document.querySelectorAll(".provider-model-row"));
    const edit = document.querySelector(".provider-model-edit");
    const svg = edit ? edit.querySelector("svg") : null;
    const chips = Array.from(document.querySelectorAll(".provider-model-chip"));
    const badgeChips = Array.from(
      document.querySelectorAll(".provider-model-type"),
    );
    const add = document.querySelector("[data-testid='pi-add-custom-model']");
    return {
      rowCount: rows.length,
      firstRowChips: rows[0]
        ? Array.from(rows[0].querySelectorAll(".provider-model-chip")).map(
            (chip) => chip.textContent,
          )
        : null,
      chipTexts: [...new Set(chips.map((chip) => chip.textContent))],
      legacyBadgeChips: badgeChips.length,
      editAria: edit ? edit.getAttribute("aria-label") : null,
      editTitle: edit ? edit.getAttribute("title") : null,
      editIconWidth: svg ? svg.getAttribute("width") : null,
      addRole: add ? add.getAttribute("role") : null,
      addTabIndex: add ? add.getAttribute("tabindex") : null,
      addAria: add ? add.getAttribute("aria-label") : null,
      addHasPlus: add ? !!add.querySelector("svg") : null,
      allRowLabel: document.querySelector(".provider-model-all-label")
        ? document.querySelector(".provider-model-all-label").textContent
        : null,
      searchPlaceholder: document.getElementById("piModelSearch").placeholder,
    };
  });
  await page.screenshot({ path: `${shots}/provider-picker-20260919.png` });

  // 2. Open the Add-custom-model dialog through a real click on the footer row.
  await page.click("[data-testid='pi-add-custom-model']");
  await page.waitForTimeout(600);
  results.dialog = await page.evaluate(() => ({
    open: document
      .getElementById("providerCustomModelModal")
      .classList.contains("show"),
    title: document.getElementById("providerCustomModelTitle").textContent,
    namePlaceholder: document.getElementById("pcmName").placeholder,
    typesPlaceholder: document.getElementById("pcmTypesPlaceholder")
      ? document.getElementById("pcmTypesPlaceholder").textContent
      : null,
    submit: document.getElementById("pcmSubmit").textContent,
    cancel: document.getElementById("pcmCancel").textContent,
    popoverHidden: document.getElementById("pcmTypesPopover").hidden,
  }));

  // 3. Real click on the combobox trigger: 7 options in upstream order.
  await page.click("[data-testid='pcm-types-trigger']");
  await page.waitForTimeout(400);
  results.multiselect = await page.evaluate(() => ({
    popoverHidden: document.getElementById("pcmTypesPopover").hidden,
    expanded: document
      .getElementById("pcmTypesTrigger")
      .getAttribute("aria-expanded"),
    options: Array.from(
      document.querySelectorAll("#pcmTypesOptions .pi-multiselect-option"),
    ).map((row) => row.textContent.trim()),
    searchPlaceholder: document.getElementById("pcmTypesSearch").placeholder,
    footer: [
      document.getElementById("pcmTypesClearAll").textContent,
      document.getElementById("pcmTypesClose").textContent,
    ],
  }));
  await page.screenshot({ path: `${shots}/provider-multiselect-open-20260919.png` });

  // 4. Select two types, rewrite the search to filter, then clear.
  await page
    .locator("#pcmTypesOptions .pi-multiselect-option", { hasText: "Chat" })
    .first()
    .click();
  await page
    .locator("#pcmTypesOptions .pi-multiselect-option", { hasText: "Embedding" })
    .first()
    .click();
  await page.waitForTimeout(300);
  results.selected = await page.evaluate(() => ({
    badges: Array.from(document.querySelectorAll(".pi-multiselect-badge")).map(
      (badge) => badge.textContent.trim(),
    ),
    checked: document.querySelectorAll(".pi-multiselect-check.on").length,
    clearHidden: document.getElementById("pcmTypesClear").hidden,
  }));
  await page.fill("#pcmTypesSearch", "rer");
  await page.waitForTimeout(300);
  results.filtered = await page.evaluate(() => ({
    options: Array.from(
      document.querySelectorAll("#pcmTypesOptions .pi-multiselect-option"),
    ).map((row) => row.textContent.trim()),
  }));
  await page.fill("#pcmTypesSearch", "");
  await page.waitForTimeout(200);
  // Remove one badge through its XCircle.
  await page.locator(".pi-multiselect-badge-remove").first().click();
  await page.waitForTimeout(250);
  results.afterBadgeRemove = await page.evaluate(() =>
    Array.from(document.querySelectorAll(".pi-multiselect-badge")).map((badge) =>
      badge.textContent.trim(),
    ),
  );
  // Clear + Close buttons of the footer.
  await page.click("#pcmTypesClearAll");
  await page.waitForTimeout(250);
  results.cleared = await page.evaluate(() => ({
    badges: document.querySelectorAll(".pi-multiselect-badge").length,
    placeholder: document.getElementById("pcmTypesPlaceholder")
      ? document.getElementById("pcmTypesPlaceholder").textContent
      : null,
  }));
  await page.click("#pcmTypesClose");
  await page.waitForTimeout(250);
  results.closed = await page.evaluate(
    () => document.getElementById("pcmTypesPopover").hidden,
  );

  // 5. Validation: max-tokens below the upstream minimum, then a clean submit.
  await page.fill("#pcmName", PROBE_MODEL);
  await page.click("[data-testid='pcm-types-trigger']");
  await page.waitForTimeout(300);
  await page
    .locator("#pcmTypesOptions .pi-multiselect-option", { hasText: "Chat" })
    .first()
    .click();
  await page.click("#pcmTypesClose");
  await page.fill("#pcmMaxTokens", "-1");
  await page.click("#pcmSubmit");
  await page.waitForTimeout(400);
  results.maxTokensError = await page
    .locator("#pcmError")
    .textContent();
  await page.fill("#pcmMaxTokens", "64");
  await page.click("#pcmSubmit");
  await page.waitForTimeout(600);
  results.added = await page.evaluate((name) => {
    const row = Array.from(document.querySelectorAll(".provider-model-row")).find(
      (candidate) => candidate.textContent.includes(name),
    );
    return {
      dialogClosed: !document
        .getElementById("providerCustomModelModal")
        .classList.contains("show"),
      inList: !!row,
      chips: row
        ? Array.from(row.querySelectorAll(".provider-model-chip")).map(
            (chip) => chip.textContent,
          )
        : null,
      checked: row ? row.querySelector("input[type=checkbox]").checked : null,
    };
  }, PROBE_MODEL);

  // 6. Duplicate name is refused with the upstream message.
  await page.click("[data-testid='pi-add-custom-model']");
  await page.waitForTimeout(400);
  await page.fill("#pcmName", PROBE_MODEL);
  await page.fill("#pcmMaxTokens", "64");
  await page.click("[data-testid='pcm-types-trigger']");
  await page.waitForTimeout(250);
  await page
    .locator("#pcmTypesOptions .pi-multiselect-option", { hasText: "Chat" })
    .first()
    .click();
  await page.click("#pcmTypesClose");
  await page.click("#pcmSubmit");
  await page.waitForTimeout(400);
  results.duplicateError = await page.locator("#pcmError").textContent();
  await page.click("#pcmCancel");
  await page.waitForTimeout(300);

  // 7. Edit mode: the pencil opens the dialog with the type preselected.
  await page.evaluate((name) => {
    const row = Array.from(document.querySelectorAll(".provider-model-row")).find(
      (candidate) => candidate.textContent.includes(name),
    );
    row.querySelector(".provider-model-edit").click();
  }, PROBE_MODEL);
  await page.waitForTimeout(500);
  results.editMode = await page.evaluate(() => ({
    title: document.getElementById("providerCustomModelTitle").textContent,
    nameDisabled: document.getElementById("pcmName").disabled,
    name: document.getElementById("pcmName").value,
    badges: Array.from(document.querySelectorAll(".pi-multiselect-badge")).map(
      (badge) => badge.textContent.trim(),
    ),
    maxTokens: document.getElementById("pcmMaxTokens").value,
  }));
  await page.screenshot({ path: `${shots}/provider-custom-model-edit-20260919.png` });
  await page.click("#pcmCancel");
  await page.waitForTimeout(300);

  // 7a. Make sure one provider instance exists so the added-models section is
  //     populated. The instance is created through the same API the provider
  //     modal posts to (the isolated probe host has no Ollama to verify
  //     against), then removed again through the row trash button so nothing is
  //     left behind.
  await page.click("#providerInstanceModal button:has-text('Cancel')");
  await page.waitForTimeout(400);
  const created = await page.evaluate(async () => {
    const response = await fetch("/api/v1/providers/Ollama/instances", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        instance_name: "cdp-probe-instance",
        api_key: "",
        base_url: "http://127.0.0.1:11434",
        region: "default",
        group_id: null,
        model_info: [
          {
            model_name: "cdp-probe-llama",
            model_type: ["chat"],
            max_tokens: 4096,
            extra: { is_tools: true },
          },
        ],
      }),
    });
    const data = await response.json().catch(() => ({}));
    return { ok: response.ok && data.code === 0, message: data.message || null };
  });
  results.instanceCreated = created;
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.waitForSelector("[data-testid='added-model-card']", {
    timeout: 15000,
  });
  results.instanceRows = await page.evaluate(() => ({
    groups: document.querySelectorAll("[data-testid='added-model-card']").length,
    names: Array.from(document.querySelectorAll(".added-model-instance-name")).map(
      (node) => node.textContent,
    ),
  }));

  // 7b. Instance model editor (`used-model.tsx`): expand an instance of the
  //     added provider, then open the row pencil and check the shared
  //     MultiSelect with the name / max-tokens fields disabled.
  const instanceProbe = await page.evaluate(async () => {
    const group = document.querySelector("[data-testid='added-model-card']");
    if (!group) return { skipped: "no added provider" };
    const toggle = group.querySelector(".added-model-toggle");
    if (!toggle) return { skipped: "no instance toggle" };
    toggle.click();
    return { provider: group.dataset.provider, text: toggle.textContent.trim() };
  });
  results.instanceToggle = instanceProbe;
  await page.waitForTimeout(2500);
  results.instanceToggleOpen = await page.evaluate(() => {
    const toggle = document.querySelector(".added-model-toggle");
    const first = document.querySelector(".added-model-row");
    return {
      text: toggle ? toggle.textContent.trim() : null,
      expanded: toggle ? toggle.getAttribute("aria-expanded") : null,
      hasChevron: !!(toggle && toggle.querySelector(".added-model-toggle-icon svg")),
      rows: document.querySelectorAll(".added-model-row").length,
      chips: first
        ? Array.from(first.querySelectorAll(".added-model-type")).map(
            (chip) => chip.textContent,
          )
        : null,
      editSvg: !!(first && first.querySelector(".added-model-edit svg")),
      editAria: first
        ? first.querySelector(".added-model-edit").getAttribute("aria-label")
        : null,
      deleteSvg: !!(document.querySelector(".added-model-delete svg")),
    };
  });
  const editor = await page.evaluate(() => {
    const row = document.querySelector(".added-model-row");
    const pencil = row ? row.querySelector(".added-model-edit") : null;
    if (!pencil) return { skipped: "no model row" };
    pencil.click();
    return { opened: true };
  });
  results.instanceEditorOpen = editor;
  if (!editor.skipped) {
    await page.waitForTimeout(600);
    results.instanceEditor = await page.evaluate(() => ({
      title: document.getElementById("editInstanceModelTitle").textContent,
      nameDisabled: document.getElementById("editInstanceModelName").disabled,
      maxTokensDisabled: document.getElementById("editInstanceModelMaxTokens").disabled,
      confirm: document.getElementById("editInstanceModelOk").textContent,
      cancel: document.getElementById("editInstanceModelCancel").textContent,
      badges: Array.from(document.querySelectorAll("#eimTypesValues .pi-multiselect-badge")).map(
        (badge) => badge.textContent.trim(),
      ),
    }));
    // Real click on the shared combobox: the option list uses the same order.
    await page.click("[data-testid='eim-types-trigger']");
    await page.waitForTimeout(400);
    results.instanceEditorOptions = await page.evaluate(() =>
      Array.from(document.querySelectorAll("#eimTypesOptions .pi-multiselect-option")).map(
        (row) => row.textContent.trim(),
      ),
    );
    await page.click("#eimTypesClose");
    await page.waitForTimeout(200);
    await page.click("#editInstanceModelCancel");
    await page.waitForTimeout(300);
    results.instanceEditorClosed = await page.evaluate(
      () =>
        !document
          .getElementById("editInstanceModelModal")
          .classList.contains("show"),
    );
  }

  // 7c. Delete the probe instance through the row trash button.
  if (created.ok) {
    const opened = await page.evaluate(() => {
      const rows = Array.from(document.querySelectorAll(".added-model-instance"));
      const row = rows.find(
        (node) =>
          (node.querySelector(".added-model-instance-name") || {}).textContent ===
          "cdp-probe-instance",
      );
      if (!row) return false;
      row.querySelector(".added-model-delete").click();
      return true;
    });
    results.deleteDialogOpened = opened;
    if (opened) {
      await page.waitForTimeout(400);
      await page.click("#deleteModelInstanceOk");
      // Wait for the row to disappear instead of sampling once: the delete
      // round-trip reloads the instance list asynchronously.
      await page
        .waitForFunction(
          () =>
            !Array.from(
              document.querySelectorAll(".added-model-instance-name"),
            ).some((node) => node.textContent === "cdp-probe-instance"),
          null,
          { timeout: 15000 },
        )
        .catch(() => {});
      results.instanceDeleted = await page.evaluate(() => ({
        remaining: Array.from(
          document.querySelectorAll(".added-model-instance-name"),
        ).map((node) => node.textContent),
        stillThere: Array.from(
          document.querySelectorAll(".added-model-instance-name"),
        ).some((node) => node.textContent === "cdp-probe-instance"),
      }));
    }
  }

  // 8. The provider modal stays closed: nothing was persisted.
  results.providerModalClosed = !(await page
    .locator("#providerInstanceModal")
    .evaluate((node) => node.classList.contains("show")));
  results.instanceCountUnchanged = await page.evaluate(async () => {
    const response = await fetch("/api/v1/tenant/models");
    const data = await response.json();
    return Array.isArray(data.data) ? data.data.length : -1;
  });
} catch (error) {
  results.errors.push(String(error && error.stack ? error.stack : error));
  await page.screenshot({ path: `${shots}/provider-custom-model-error-20260919.png` });
} finally {
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  await b.close();
}
if (results.errors.length) process.exit(1);
