// Settings: "Report a problem". Opens an issue form on GitHub with the report
// info filled in, and prepares packet logs for a bug report. Nothing is sent
// from here: the player sees the info first and submits the form themselves.
"use strict";

const REPORT_REPO = "https://github.com/Seralth/Daevalog";

// The issue forms in .github/ISSUE_TEMPLATE/. Each has a "report-info" field,
// which GitHub fills from the query parameter of the same id.
const REPORT_FORMS = {
  "wrong-numbers": "wrong-numbers.yml",
  crash: "crash.yml",
  other: "something-else.yml",
};

const REPORT_HINTS = {
  "wrong-numbers": ["settings.report.hint.wrongNumbers",
    "Attach a prepared packet log of the fight, and the game's Combat Analysis record (Ctrl+X) if you have one."],
  crash: ["settings.report.hint.crash",
    "Attach debug.log from Open log folder. If Enable debug logging is off, turn it on and make the problem happen again first."],
  other: ["settings.report.hint.other", "Anything else, ideas for new features too."],
  security: ["settings.report.hint.security",
    "For anything that could hurt other players or users if posted in public. Only the developer sees this report."],
};

function reportFormUrl(kind, info) {
  if (kind === "security") return `${REPORT_REPO}/security/advisories/new`;
  const form = REPORT_FORMS[kind];
  if (!form) return null;
  return `${REPORT_REPO}/issues/new?template=${form}&report-info=${encodeURIComponent(info || "")}`;
}

Object.assign(DpsApp.prototype, {
  initReportProblem() {
    const modal = document.querySelector(".reportModal");
    const openBtn = document.querySelector(".reportProblemBtn");
    if (!modal || !openBtn || openBtn.dataset.wired) return;
    openBtn.dataset.wired = "1";
    const q = (selector) => modal.querySelector(selector);
    const t = (key, fallback, vars) =>
      vars ? (window.i18n?.format?.(key, vars, fallback) ?? fallback)
           : (window.i18n?.t?.(key, fallback) ?? fallback);
    const ui = {
      kinds: [...modal.querySelectorAll(".reportKind")],
      hint: q(".reportKindHint"),
      prepare: q(".reportPrepare"),
      select: q(".reportLogSelect"),
      prepareBtn: q(".reportPrepareBtn"),
      prepareStatus: q(".reportPrepareStatus"),
      recordBtn: q(".reportRecordFolderBtn"),
      info: q(".reportInfo"),
      copyBtn: q(".reportCopyBtn"),
      formBtn: q(".reportOpenBtn"),
    };
    let kind = null;
    let infoText = "";

    const choose = (next) => {
      kind = next;
      for (const btn of ui.kinds) {
        const on = btn.dataset.kind === kind;
        btn.classList.toggle("isActive", on);
        btn.setAttribute("aria-pressed", on ? "true" : "false");
      }
      const hint = REPORT_HINTS[kind];
      ui.hint.textContent = hint ? t(hint[0], hint[1]) : "";
      ui.prepare.hidden = kind !== "wrong-numbers";
      ui.formBtn.disabled = !kind;
    };

    const fillLogs = (logs) => {
      ui.select.textContent = "";
      for (const log of logs || []) {
        const option = document.createElement("option");
        option.value = log.name;
        option.textContent = `${log.name} · ${(log.bytes / 1048576).toFixed(1)} MB`;
        ui.select.appendChild(option);
      }
      const none = !logs || logs.length === 0;
      ui.select.disabled = none;
      ui.prepareBtn.disabled = none;
      ui.prepareStatus.textContent = none
        ? t("settings.report.prepare.none",
          "No packet logs yet. Turn on Enable packet logging, play the fight again, then prepare a log.")
        : "";
    };

    const open = async () => {
      choose(null);
      ui.info.textContent = "";
      ui.prepareStatus.textContent = "";
      modal.classList.add("isOpen");
      modal.setAttribute("aria-hidden", "false");
      ui.kinds[0]?.focus?.();
      try {
        const result = await window.javaBridge?.reportInfo?.();
        infoText = result?.text || "";
        ui.info.textContent = infoText;
        fillLogs(result?.logs);
        ui.recordBtn.hidden = !result?.recordFolder;
      } catch {
        infoText = "";
        fillLogs([]);
      }
    };

    const close = () => {
      modal.classList.remove("isOpen");
      modal.setAttribute("aria-hidden", "true");
      openBtn.focus?.();
    };

    openBtn.addEventListener("click", open);
    q(".reportClose")?.addEventListener("click", close);
    modal.addEventListener("click", (event) => {
      if (event.target === modal) close();
    });
    modal.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        close();
      }
    });
    for (const btn of ui.kinds) {
      btn.addEventListener("click", () => choose(btn.dataset.kind));
    }

    ui.formBtn.addEventListener("click", () => {
      const url = reportFormUrl(kind, infoText);
      if (url) window.javaBridge?.openBrowser?.(url);
    });

    ui.copyBtn.addEventListener("click", async () => {
      const label = t("settings.report.copy", "Copy");
      let ok = false;
      try {
        await navigator.clipboard.writeText(infoText);
        ok = true;
      } catch {
        try {
          const area = document.createElement("textarea");
          area.value = infoText;
          area.style.position = "fixed";
          area.style.opacity = "0";
          document.body.appendChild(area);
          area.select();
          ok = document.execCommand("copy");
          area.remove();
        } catch {
          ok = false;
        }
      }
      ui.copyBtn.textContent = ok ? t("settings.report.copied", "Copied") : t("settings.report.copyFailed", "Could not copy");
      setTimeout(() => { ui.copyBtn.textContent = label; }, 1500);
    });

    ui.prepareBtn.addEventListener("click", async () => {
      const name = ui.select.value || null;
      ui.prepareBtn.disabled = true;
      ui.prepareStatus.textContent = t("settings.report.prepare.working", "Preparing...");
      try {
        const result = await window.javaBridge?.prepareReportLog?.(name);
        ui.prepareStatus.textContent = t("settings.report.prepare.done",
          `Saved ${result?.name}. The log folder is open.`, { name: result?.name || "" });
      } catch (err) {
        const error = String(err?.message || err || "");
        ui.prepareStatus.textContent = t("settings.report.prepare.failed",
          `Could not prepare the log: ${error}`, { error });
      } finally {
        ui.prepareBtn.disabled = false;
      }
    });

    ui.recordBtn.addEventListener("click", () => window.javaBridge?.openGameRecordFolder?.());
  },
});
