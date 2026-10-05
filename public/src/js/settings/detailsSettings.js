// Settings: the Details panel settings menu and its helpers.
"use strict";

Object.assign(DpsApp.prototype, {
  setupDetailsPanelSettings() {
    this.detailsOpacityInput = document.querySelector(".detailsOpacityInput");
    this.detailsOpacityValue = document.querySelector(".detailsOpacityValue");
    this.detailsFontSizeInput = document.querySelector(".detailsFontSizeInput");
    this.detailsFontSizeValue = document.querySelector(".detailsFontSizeValue");
    this.detailsIconSizeInput = document.querySelector(".detailsIconSizeInput");
    this.detailsIconSizeValue = document.querySelector(".detailsIconSizeValue");
    this.detailsSettingsBtn = document.querySelector(".detailsSettingsBtn");
    this.detailsSettingsMenu = document.querySelector(".detailsSettingsMenu");
    this.detailsIncludeMeterCheckbox = document.querySelector(".detailsIncludeMeterCheckbox");
    this.detailsSaveScreenshotCheckbox = document.querySelector(".detailsSaveScreenshotCheckbox");
    this.detailsScreenshotFolderRow = document.querySelector(".detailsSettingsFolder");
    this.detailsScreenshotFolderPath = document.querySelector(".detailsSettingsFolderPath");
    this.detailsScreenshotFolderBtn = document.querySelector(".detailsSettingsFolderBtn");
    this.detailsColumnToggles = document.querySelectorAll(".detailsColumnToggle");

    const storedOpacity = this.safeGetSetting(this.storageKeys.detailsBackgroundOpacity);
    const initialOpacity = this.parseDetailsOpacity(storedOpacity);
    this.setDetailsBackgroundOpacity(initialOpacity, { persist: false });

    const storedFontSize = this.safeGetSetting(this.storageKeys.detailsFontSize);
    const initialFontSize = this.parseDetailsFontSize(storedFontSize);
    this.setDetailsFontSize(initialFontSize, { persist: false });

    const storedIconSize = this.safeGetSetting(this.storageKeys.detailsIconSize);
    const initialIconSize = this.parseDetailsIconSize(storedIconSize);
    this.setDetailsIconSize(initialIconSize, { persist: false });

    const storedIncludeMeter =
      this.safeGetSetting(this.storageKeys.detailsIncludeMeterScreenshot) === "true";
    const storedSaveToFolder =
      this.safeGetSetting(this.storageKeys.detailsSaveScreenshotToFolder) === "true";
    const storedFolder = this.safeGetSetting(this.storageKeys.detailsScreenshotFolder);
    this.includeMainMeterScreenshot = storedIncludeMeter;
    this.saveScreenshotToFolder = storedSaveToFolder;
    this.screenshotFolder = storedFolder || this.getDefaultScreenshotFolder();

    if (this.detailsIncludeMeterCheckbox) {
      this.detailsIncludeMeterCheckbox.checked = this.includeMainMeterScreenshot;
      this.detailsIncludeMeterCheckbox.addEventListener("change", (event) => {
        this.includeMainMeterScreenshot = !!event.target?.checked;
        this.safeSetSetting(
          this.storageKeys.detailsIncludeMeterScreenshot,
          String(this.includeMainMeterScreenshot)
        );
      });
    }
    if (this.detailsSaveScreenshotCheckbox) {
      this.detailsSaveScreenshotCheckbox.checked = this.saveScreenshotToFolder;
      this.detailsSaveScreenshotCheckbox.addEventListener("change", (event) => {
        this.saveScreenshotToFolder = !!event.target?.checked;
        if (this.saveScreenshotToFolder && !this.screenshotFolder) {
          this.screenshotFolder = this.getDefaultScreenshotFolder();
        }
        this.safeSetSetting(
          this.storageKeys.detailsSaveScreenshotToFolder,
          String(this.saveScreenshotToFolder)
        );
        if (this.screenshotFolder) {
          this.safeSetSetting(this.storageKeys.detailsScreenshotFolder, this.screenshotFolder);
        }
        this.updateScreenshotFolderDisplay();
      });
    }
    if (this.detailsScreenshotFolderBtn) {
      this.detailsScreenshotFolderBtn.addEventListener("click", async () => {
        const selected = await window.javaBridge?.chooseScreenshotFolder?.(
          this.screenshotFolder || this.getDefaultScreenshotFolder()
        );
        if (!selected || typeof selected !== "string") return;
        this.screenshotFolder = selected;
        this.safeSetSetting(this.storageKeys.detailsScreenshotFolder, this.screenshotFolder);
        this.updateScreenshotFolderDisplay();
      });
    }
    this.updateScreenshotFolderDisplay();

    const storedHiddenColumns = this.safeGetSetting(this.storageKeys.detailsHiddenColumns);
    const storedSeenColumns = this.safeGetSetting(this.storageKeys.detailsSeenColumns);
    const hiddenColumns = new Set();
    // Columns added after the first release: hidden until switched on.
    const NEW_COLUMNS_DEFAULT_HIDDEN = ["regen", "parry", "block", "perfectblock", "ironwall", "regeneration", "miss", "resist"];
    if (typeof storedHiddenColumns === "string" && storedHiddenColumns.trim()) {
      const parsedHidden = this.safeParseJSON(storedHiddenColumns, []);
      if (Array.isArray(parsedHidden)) {
        parsedHidden.forEach((column) => {
          if (typeof column === "string" && column.trim()) {
            hiddenColumns.add(column);
          }
        });
      }
    }
    // Track which columns the user has seen in their settings UI. Any column
    // not yet seen defaults to hidden, regardless of upgrade state.
    const seenColumns = new Set();
    if (typeof storedSeenColumns === "string" && storedSeenColumns.trim()) {
      const parsedSeen = this.safeParseJSON(storedSeenColumns, []);
      if (Array.isArray(parsedSeen)) {
        parsedSeen.forEach((c) => { if (typeof c === "string") seenColumns.add(c); });
      }
    }
    NEW_COLUMNS_DEFAULT_HIDDEN.forEach((col) => {
      if (!seenColumns.has(col)) hiddenColumns.add(col);
    });
    const applyDetailsColumnVisibility = () => {
      if (!this.detailsPanel) return;
      const columns = ["hit", "dmg", "dmgpct", "mhit", "mdmg", "crit", "parry", "perfect", "double", "back", "block", "perfectblock", "ironwall", "regeneration", "miss", "resist", "regen", "mindmg", "avgdmg", "maxdmg"];
      columns.forEach((column) => {
        this.detailsPanel.classList.toggle(`hide-col-${column}`, hiddenColumns.has(column));
      });
      this.detailsUI?.updateGridColumns?.();
    };
    if (this.detailsColumnToggles && this.detailsColumnToggles.length) {
      this.detailsColumnToggles.forEach((toggle) => {
        const column = toggle.dataset.column;
        if (!column) return;
        toggle.checked = !hiddenColumns.has(column);
        toggle.addEventListener("change", (event) => {
          const isVisible = !!event.target?.checked;
          if (isVisible) {
            hiddenColumns.delete(column);
          } else {
            hiddenColumns.add(column);
          }
          // Mark as seen so the default-hidden behaviour doesn't override the user's choice
          seenColumns.add(column);
          this.safeSetSetting(this.storageKeys.detailsHiddenColumns, JSON.stringify([...hiddenColumns]));
          this.safeSetSetting(this.storageKeys.detailsSeenColumns, JSON.stringify([...seenColumns]));
          applyDetailsColumnVisibility();
        });
      });
    }
    applyDetailsColumnVisibility();

    if (this.detailsOpacityInput) {
      this.detailsOpacityInput.value = String(Math.round(initialOpacity * 100));
      const stopDrag = (event) => event.stopPropagation();
      this.detailsOpacityInput.addEventListener("mousedown", stopDrag);
      this.detailsOpacityInput.addEventListener("touchstart", stopDrag, { passive: true });
      this.detailsOpacityInput.addEventListener("input", (event) => {
        const nextValue = Number(event.target?.value);
        const nextOpacity = Number.isFinite(nextValue) ? nextValue / 100 : 1;
        this.setDetailsBackgroundOpacity(nextOpacity, { persist: true });
      });
    }

    if (this.detailsFontSizeInput) {
      this.detailsFontSizeInput.value = String(Math.round(initialFontSize));
      const stopDrag = (event) => event.stopPropagation();
      this.detailsFontSizeInput.addEventListener("mousedown", stopDrag);
      this.detailsFontSizeInput.addEventListener("touchstart", stopDrag, { passive: true });
      this.detailsFontSizeInput.addEventListener("input", (event) => {
        const nextValue = Number(event.target?.value);
        if (!Number.isFinite(nextValue)) return;
        this.setDetailsFontSize(nextValue, { persist: true });
      });
    }

    if (this.detailsIconSizeInput) {
      this.detailsIconSizeInput.value = String(Math.round(initialIconSize));
      const stopDrag = (event) => event.stopPropagation();
      this.detailsIconSizeInput.addEventListener("mousedown", stopDrag);
      this.detailsIconSizeInput.addEventListener("touchstart", stopDrag, { passive: true });
      this.detailsIconSizeInput.addEventListener("input", (event) => {
        const nextValue = Number(event.target?.value);
        if (!Number.isFinite(nextValue)) return;
        this.setDetailsIconSize(nextValue, { persist: true });
      });
    }

    this.detailsSettingsBtn?.addEventListener("click", (event) => {
      event.stopPropagation();
      this.toggleDetailsSettingsMenu(event);
    });

    this.detailsSettingsMenu?.addEventListener("click", (event) => {
      event.stopPropagation();
    });

    this.detailsClose?.addEventListener("click", () => {
      this.closeDetailsSettingsMenu();
    });

    document.addEventListener("click", (event) => {
      if (!this.detailsSettingsMenu?.classList.contains("isOpen")) {
        return;
      }
      const target = event.target;
      if (
        this.detailsSettingsMenu?.contains(target) ||
        this.detailsSettingsBtn?.contains(target)
      ) {
        return;
      }
      this.closeDetailsSettingsMenu();
    });
  },

  parseDetailsOpacity(value) {
    if (value === null || value === undefined || value === "") {
      return 0.8;
    }
    const parsed = Number(value);
    if (!Number.isFinite(parsed)) {
      return 0.8;
    }
    return Math.min(1, Math.max(0, parsed));
  },

  getDefaultDetailsFontSize() {
    const rootSize = getComputedStyle(document.documentElement).getPropertyValue("--font");
    const parsed = Number.parseFloat(rootSize);
    if (Number.isFinite(parsed)) {
      return parsed;
    }
    return 16;
  },

  parseDetailsFontSize(value) {
    if (value === null || value === undefined || value === "") {
      return this.getDefaultDetailsFontSize();
    }
    const parsed = Number(value);
    if (!Number.isFinite(parsed)) {
      return this.getDefaultDetailsFontSize();
    }
    return Math.min(this.DETAILS_FONT_SIZE_MAX, Math.max(this.DETAILS_FONT_SIZE_MIN, parsed));
  },

  parseDetailsIconSize(value) {
    if (value === null || value === undefined || value === "") {
      return 36;
    }
    const parsed = Number(value);
    if (!Number.isFinite(parsed)) {
      return 36;
    }
    return Math.min(this.DETAILS_ICON_SIZE_MAX, Math.max(this.DETAILS_ICON_SIZE_MIN, parsed));
  },

  setDetailsBackgroundOpacity(opacity, { persist = false } = {}) {
    const clamped = Math.min(1, Math.max(0, opacity));
    if (this.detailsPanel) {
      this.detailsPanel.style.setProperty("--details-bg-opacity", clamped);
    }
    if (this.detailsOpacityValue) {
      this.detailsOpacityValue.textContent = `${Math.round(clamped * 100)}%`;
    }
    if (this.detailsOpacityInput && document.activeElement !== this.detailsOpacityInput) {
      this.detailsOpacityInput.value = String(Math.round(clamped * 100));
    }
    if (persist) {
      this.safeSetSetting(this.storageKeys.detailsBackgroundOpacity, String(clamped));
    }
  },

  setDetailsFontSize(size, { persist = false } = {}) {
    const clamped = Math.min(this.DETAILS_FONT_SIZE_MAX, Math.max(this.DETAILS_FONT_SIZE_MIN, size));
    if (this.detailsPanel) {
      this.detailsPanel.style.setProperty("--details-font-size", `${clamped}px`);
    }
    if (this.detailsFontSizeValue) {
      this.detailsFontSizeValue.textContent = `${Math.round(clamped)}px`;
    }
    if (this.detailsFontSizeInput && document.activeElement !== this.detailsFontSizeInput) {
      this.detailsFontSizeInput.value = String(Math.round(clamped));
    }
    if (persist) {
      this.safeSetSetting(this.storageKeys.detailsFontSize, String(clamped));
    }
  },

  setDetailsIconSize(size, { persist = false } = {}) {
    const clamped = Math.min(this.DETAILS_ICON_SIZE_MAX, Math.max(this.DETAILS_ICON_SIZE_MIN, size));
    if (this.detailsPanel) {
      this.detailsPanel.style.setProperty("--details-skill-icon-size", `${clamped}px`);
    }
    const tooltipSize = Math.round(Math.min(28, Math.max(16, clamped * 0.56)));
    document.documentElement.style.setProperty("--tooltip-skill-icon-size", `${tooltipSize}px`);
    if (this.detailsIconSizeValue) {
      this.detailsIconSizeValue.textContent = `${Math.round(clamped)}px`;
    }
    if (this.detailsIconSizeInput && document.activeElement !== this.detailsIconSizeInput) {
      this.detailsIconSizeInput.value = String(Math.round(clamped));
    }
    if (persist) {
      this.safeSetSetting(this.storageKeys.detailsIconSize, String(clamped));
    }
  },

  getDefaultScreenshotFolder() {
    return (
      window.javaBridge?.getDefaultScreenshotFolder?.() ||
      this.safeGetSetting(this.storageKeys.detailsScreenshotFolder) ||
      ""
    );
  },

  updateScreenshotFolderDisplay() {
    if (!this.detailsScreenshotFolderRow) return;
    this.detailsScreenshotFolderRow.classList.toggle("isHidden", !this.saveScreenshotToFolder);
    if (this.detailsScreenshotFolderPath) {
      this.detailsScreenshotFolderPath.textContent =
        this.screenshotFolder || this.getDefaultScreenshotFolder() || "-";
    }
  },

  toggleDetailsSettingsMenu(event) {
    if (!this.detailsSettingsMenu) return;
    if (this.detailsSettingsMenu.classList.contains("isOpen")) {
      this.closeDetailsSettingsMenu();
      return;
    }
    this.openDetailsSettingsMenu(event);
  },

  openDetailsSettingsMenu(event) {
    if (!this.detailsSettingsMenu) return;
    this.detailsSettingsMenu.classList.add("isOpen");
  },

  closeDetailsSettingsMenu() {
    if (!this.detailsSettingsMenu) return;
    this.detailsSettingsMenu.classList.remove("isOpen");
  },
});
