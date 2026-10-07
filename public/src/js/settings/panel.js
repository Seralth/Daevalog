// Settings: the panel, its dropdowns, the setters and the sync from the Settings window.
"use strict";

Object.assign(DpsApp.prototype, {
  setupSettingsPanel() {
    this.settingsPanel = document.querySelector(".settingsPanel");
    this.settingsClose = document.querySelector(".settingsClose");
    this.settingsBtn = document.querySelector(".settingsBtn");
    this.lockedIp = document.querySelector(".lockedIp");
    this.lockedPort = document.querySelector(".lockedPort");
    this.localActorIdInput = document.querySelector(".localActorIdInput");
    this.allTargetsWindowDropdownBtn = document.querySelector(".allTargetsWindowDropdownBtn");
    this.allTargetsWindowDropdownMenu = document.querySelector(".allTargetsWindowDropdownMenu");
    this.encounterTimeoutInput = document.querySelector(".encounterTimeoutInput");
    this.targetWindowDropdownBtn = document.querySelector(".targetWindowDropdownBtn");
    this.targetWindowDropdownMenu = document.querySelector(".targetWindowDropdownMenu");
    this.trainSelectionModeDropdownBtn = document.querySelector(".trainSelectionModeDropdownBtn");
    this.trainSelectionModeDropdownMenu = document.querySelector(".trainSelectionModeDropdownMenu");
    this.defaultMeterModeDropdownBtn = document.querySelector(".defaultMeterModeDropdownBtn");
    this.defaultMeterModeDropdownMenu = document.querySelector(".defaultMeterModeDropdownMenu");
    this.resetDetectBtn = document.querySelector(".resetDetectBtn");
    this.autoDetectDeviceCheckbox = document.querySelector(".autoDetectDeviceCheckbox");
    this.deviceDropdownBtn = document.querySelector(".deviceDropdownBtn");
    this.deviceDropdownMenu = document.querySelector(".deviceDropdownMenu");
    this.characterNameInput = document.querySelector(".characterNameInput");
    this.showSuspendBtnCheckbox = document.querySelector(".showSuspendBtnCheckbox");
    this.bossLogsCheckbox = document.querySelector(".bossLogsCheckbox");
    this.debugLoggingCheckbox = document.querySelector(".debugLoggingCheckbox");
    this.showPingCheckbox = document.querySelector(".showPingCheckbox");
    this.saveRawPacketsCheckbox = document.querySelector(".saveRawPacketsCheckbox");
    this.sendLogsBtn = document.querySelector(".sendLogsBtn");
    this.openLogFolderBtn = document.querySelector(".openLogFolderBtn");
    this.sendLogsStatus = document.querySelector(".sendLogsStatus");
    this.pinMeToTopCheckbox = document.querySelector(".pinMeToTopCheckbox");
    this.detailsMonitorDropdownBtn = document.querySelector(".detailsMonitorDropdownBtn");
    this.detailsMonitorDropdownMenu = document.querySelector(".detailsMonitorDropdownMenu");
    this.detailsMonitorHint = document.querySelector(".detailsMonitorHint");
    this.meterLayoutDropdownBtn = document.querySelector(".meterLayoutDropdownBtn");
    this.meterLayoutDropdownMenu = document.querySelector(".meterLayoutDropdownMenu");
    this.playerNamesBoldCheckbox = document.querySelector(".playerNamesBoldCheckbox");
    this.hideOtherNamesCheckbox = document.querySelector(".hideOtherNamesCheckbox");
    this.autoUploadCheckbox = document.querySelector(".autoUploadCheckbox");
    this.accountStateEl = document.querySelector(".accountState");
    this.accountHintEl = document.querySelector(".accountHint");
    this.accountConnectBtn = document.querySelector(".accountConnectBtn");
    this.accountSignOutBtn = document.querySelector(".accountSignOutBtn");
    this.accountCodeBox = document.querySelector(".accountCodeBox");
    this.accountCodeEl = document.querySelector(".accountCode");
    this.playerDpsBoldCheckbox = document.querySelector(".playerDpsBoldCheckbox");
    this.meterOpacityInput = document.querySelector(".meterOpacityInput");
    this.meterOpacityValue = document.querySelector(".meterOpacityValue");
    this.bossNameSizeInput = document.querySelector(".bossNameSizeInput");
    this.bossNameSizeValue = document.querySelector(".bossNameSizeValue");
    this.windowOpacityInput = document.querySelector(".windowOpacityInput");
    this.windowOpacityValue = document.querySelector(".windowOpacityValue");
    this.supportWidget = document.querySelector(".supportWidget");
    this.supportButton = document.querySelector(".supportButton");
    this.supportModal = document.querySelector("#supportModal");
    this.supportModalTitle = document.querySelector("#supportModalTitle");
    this.supportModalClose = document.querySelector(".supportModalClose");
    this.supportQrImage = document.querySelector(".supportQrImage");
    this.supportPrimaryButton = document.querySelector(".supportPrimaryButton");
    this.supportCopyStatus = document.querySelector(".supportCopyStatus");
    this.supportActionButtons = Array.from(document.querySelectorAll(".supportIconButton"));
    this.kofiWidget = document.querySelector(".kofiWidget");
    this.quitButton = document.querySelector(".quitButton");
    this.settingsVersionValue = document.querySelector(".settingsVersionValue");
    this.settingsVersionLink = document.querySelector(".settingsVersionLink");
    this.languageDropdownBtn = document.querySelector(".languageDropdownBtn");
    this.languageDropdownMenu = document.querySelector(".languageDropdownMenu");
    this.themeDropdownBtn = document.querySelector(".themeDropdownBtn");
    this.themeDropdownMenu = document.querySelector(".themeDropdownMenu");
    this.settingsSelections = {
      language: "en",
      theme: this.theme,
      defaultMeterMode: "bossTargets",
      allTargetsWindowMs: "0",
      trainSelectionMode: "all",
      targetSelectionWindowMs: "5000",
    };

    const storedName = this.safeGetStorage(this.storageKeys.userName) || "";
    const storedAllTargetsWindowMs = this.safeGetSetting(this.storageKeys.allTargetsWindowMs) ||
      this.safeGetStorage(this.storageKeys.allTargetsWindowMs) ||
      "0";
    const storedTrainSelectionMode = this.safeGetSetting(this.storageKeys.trainSelectionMode) ||
      this.safeGetStorage(this.storageKeys.trainSelectionMode) ||
      "all";
    const storedTargetSelectionWindowMs = this.safeGetSetting(this.storageKeys.targetSelectionWindowMs) ||
      this.safeGetStorage(this.storageKeys.targetSelectionWindowMs) ||
      "5000";
    let storedMeterOpacity = this.safeGetSetting(this.storageKeys.meterFillOpacity) ||
      this.safeGetStorage(this.storageKeys.meterFillOpacity);
    if (this.safeGetSetting("dpsMeter.migration.opacityReset1") !== "done") {
      storedMeterOpacity = "80";
      this.safeSetSetting(this.storageKeys.meterFillOpacity, "80");
      this.safeSetSetting("dpsMeter.migration.opacityReset1", "done");
    }
    const storedDebugLogging = this.safeGetSetting(this.storageKeys.debugLogging) === "true";
    const storedPinMeToTop = this.safeGetSetting(this.storageKeys.pinMeToTop) === "true";
    const mainPlayerNamesBoldSetting = this.safeGetSetting(this.storageKeys.mainPlayerNamesBold);
    const storedMainPlayerNamesBold = mainPlayerNamesBoldSetting !== "false";
    const mainPlayerDpsBoldSetting = this.safeGetSetting(this.storageKeys.mainPlayerDpsBold);
    const storedMainPlayerDpsBold = mainPlayerDpsBoldSetting !== "false";
    const storedDefaultMeterMode = this.safeGetSetting(this.storageKeys.defaultMeterMode) || "bossTargets";
    const storedTargetSelection = this.safeGetStorage(this.storageKeys.targetSelection);
    const storedLanguage = this.safeGetStorage(this.storageKeys.language);
    const storedTheme = this.safeGetSetting(this.storageKeys.theme);

    // Only the overlay tells the backend things at startup. Every window runs
    // this, and opening History or a fight used to put the default meter mode
    // back while the overlay's button still showed the old one.
    const isOverlay = window.A2_VIEW === "main";
    this.setUserName(storedName, { persist: false, syncBackend: isOverlay });
    this.setOnlyShowUser(false, { persist: false });
    this.setDebugLogging(storedDebugLogging, { persist: false, syncBackend: true });
    this.setPinMeToTop(storedPinMeToTop, { persist: false });
    this.setHideOtherNames(this.safeGetSetting(this.storageKeys.hideOtherNames) === "true", { persist: false });
    this.setBetaUi(this.safeGetSetting(this.storageKeys.betaUi) !== "false", { persist: false });
    const storedSlimMode = this.safeGetSetting(this.storageKeys.slimMode) === "true";
    this.setSlimMode(storedSlimMode, { persist: false });
    this.setMainPlayerNamesBold(storedMainPlayerNamesBold, { persist: false });
    this.setMainPlayerDpsBold(storedMainPlayerDpsBold, { persist: false });
    if (mainPlayerNamesBoldSetting === null || mainPlayerNamesBoldSetting === undefined || mainPlayerNamesBoldSetting === "") {
      this.safeSetSetting(this.storageKeys.mainPlayerNamesBold, "true");
    }
    if (mainPlayerDpsBoldSetting === null || mainPlayerDpsBoldSetting === undefined || mainPlayerDpsBoldSetting === "") {
      this.safeSetSetting(this.storageKeys.mainPlayerDpsBold, "true");
    }
    const validModes = TARGET_MODES;
    const normalizedDefaultMode = validModes.includes(storedDefaultMeterMode)
      ? storedDefaultMeterMode : "bossTargets";
    this.settingsSelections.defaultMeterMode = normalizedDefaultMode;
    this.setTargetSelection(normalizedDefaultMode, {
      persist: false,
      syncBackend: isOverlay,
      reason: "default meter mode setting",
    });
    this.applyTheme(storedTheme || this.theme, { persist: false });
    if (storedLanguage) {
      this.i18n?.setLanguage?.(storedLanguage, { persist: false });
    }

    if (this.characterNameInput) {
      this.characterNameInput.value = this.USER_NAME;
      // The player's own word on their name: saved when they leave the field
      // or press Enter, sent as a manual change (the backend takes it even
      // after the game has named a character, unlike a remembered name), and
      // broadcast so the meter window adopts it too. The game's own record of
      // who is playing still replaces it on the next zone change if it differs.
      this.characterNameInput.addEventListener("keydown", (event) => {
        if (event.key === "Enter") event.target.blur();
      });
      // Saved after a pause in typing, on Enter or leaving the field, and when
      // the window closes: saving on "change" alone lost a name typed just
      // before closing Settings (issue #13). Compared with what was last
      // saved, not with the detected name, so typing the detected name still
      // saves it as the player's own.
      let savedName = String(this.safeGetSetting(this.storageKeys.userName) || "").trim();
      let saveTimer = null;
      const saveTypedName = () => {
        clearTimeout(saveTimer);
        saveTimer = null;
        const name = String(this.characterNameInput.value || "").trim();
        if (name === savedName) return;
        savedName = name;
        this.setUserName(name, { persist: true, syncBackend: true, manual: true });
        this.safeSetSetting(this.storageKeys.userName, name);
      };
      this.characterNameInput.addEventListener("input", () => {
        clearTimeout(saveTimer);
        saveTimer = setTimeout(saveTypedName, 800);
      });
      this.characterNameInput.addEventListener("change", (event) => {
        event.target.value = String(event.target?.value || "").trim();
        saveTypedName();
      });
      window.addEventListener("pagehide", () => {
        if (saveTimer) saveTypedName();
      });
      window.addEventListener("beforeunload", () => {
        if (saveTimer) saveTypedName();
      });
    }
    if (this.localActorIdInput) {
      this.localActorIdInput.value = this.localPlayerId ? String(this.localPlayerId) : "";
      this.localActorIdInput.addEventListener("input", (event) => {
        const digits = String(event.target?.value || "").replace(/[^0-9]/g, "");
        event.target.value = digits;
      });
      this.localActorIdInput.addEventListener("change", (event) => {
        const value = String(event.target?.value || "").trim();
        // The only id the UI sends: one the player typed. The backend's
        // answer comes back in the next dps update.
        if (!value) {
          // Cleared: the backend unbinds and finds you again.
          window.javaBridge?.bindLocalActorId?.(0, true);
          return;
        }
        window.javaBridge?.bindLocalActorId?.(value, true);
        if (this.USER_NAME) {
          window.javaBridge?.bindLocalNickname?.(value, this.USER_NAME, true);
        }
      });
    }

    const allowedWindows = ["0", "30000", "60000", "120000", "180000", "300000"];
    const selectedWindow = allowedWindows.includes(String(storedAllTargetsWindowMs))
      ? String(storedAllTargetsWindowMs)
      : "0";
    this.settingsSelections.allTargetsWindowMs = selectedWindow;
    this.safeSetSetting(this.storageKeys.allTargetsWindowMs, selectedWindow);
    window.javaBridge?.setAllTargetsWindowMs?.(selectedWindow);

    // Encounter timeout in whole seconds, 5-300; the backend reads the setting.
    if (this.encounterTimeoutInput) {
      const storedTimeout = Number(this.safeGetSetting(this.storageKeys.encounterTimeoutSec));
      this.encounterTimeoutInput.value = String(
        Number.isInteger(storedTimeout) && storedTimeout >= 5 && storedTimeout <= 300 ? storedTimeout : 15);
      this.encounterTimeoutInput.addEventListener("change", () => {
        const value = Math.round(Number(this.encounterTimeoutInput.value));
        const secs = Number.isFinite(value) ? Math.min(300, Math.max(5, value)) : 15;
        this.encounterTimeoutInput.value = String(secs);
        this.safeSetSetting(this.storageKeys.encounterTimeoutSec, String(secs));
      });
    }

    const allowedTargetWindows = ["5000", "10000", "15000", "20000", "30000"];
    const selectedTargetWindow = allowedTargetWindows.includes(String(storedTargetSelectionWindowMs))
      ? String(storedTargetSelectionWindowMs)
      : "5000";
    this.settingsSelections.targetSelectionWindowMs = selectedTargetWindow;
    this.safeSetSetting(this.storageKeys.targetSelectionWindowMs, selectedTargetWindow);
    window.javaBridge?.setTargetSelectionWindowMs?.(selectedTargetWindow);

    const allowedModes = ["all", "highestDamage"];
    const selectedMode = allowedModes.includes(String(storedTrainSelectionMode))
      ? String(storedTrainSelectionMode)
      : "all";
    this.trainSelectionMode = selectedMode;
    this.settingsSelections.trainSelectionMode = selectedMode;
    this.safeSetSetting(this.storageKeys.trainSelectionMode, selectedMode);
    window.javaBridge?.setTrainSelectionMode?.(selectedMode);

    if (this.bossLogsCheckbox) {
      const storedBossLogs = this.safeGetSetting(this.storageKeys.bossLogs) === "true";
      this.bossLogsCheckbox.checked = storedBossLogs;
      this.bossLogsCheckbox.addEventListener("change", (event) => {
        const isChecked = !!event.target?.checked;
        this.safeSetSetting(this.storageKeys.bossLogs, String(isChecked));
        window.javaBridge?.setBossLogsEnabled?.(isChecked);
      });
    }
    if (this.debugLoggingCheckbox) {
      this.debugLoggingCheckbox.checked = this.debugLoggingEnabled;
      this.debugLoggingCheckbox.addEventListener("change", (event) => {
        const isChecked = !!event.target?.checked;
        this.setDebugLogging(isChecked, { persist: true, syncBackend: true });
      });
    }
    if (this.showPingCheckbox) {
      this.showPingCheckbox.checked = this.showPing;
      this.showPingCheckbox.addEventListener("change", (event) => {
        this.showPing = !!event.target?.checked;
        this.safeSetSetting(this.storageKeys.showPing, String(this.showPing));
      });
    }
    this.showTotalDpsCheckbox = document.querySelector(".showTotalDpsCheckbox");
    if (this.showTotalDpsCheckbox) {
      this.showTotalDpsCheckbox.checked = this.showTotalDps;
      this.showTotalDpsCheckbox.addEventListener("change", (event) => {
        this.showTotalDps = !!event.target?.checked;
        this.safeSetSetting(this.storageKeys.showTotalDps, String(this.showTotalDps));
        this.renderCurrentRows();
      });
    }
    this.roundDpsCheckbox = document.querySelector(".roundDpsCheckbox");
    if (this.roundDpsCheckbox) {
      this.roundDpsCheckbox.checked = this.roundDps;
      this.roundDpsCheckbox.addEventListener("change", (event) => {
        this.roundDps = !!event.target?.checked;
        this.safeSetSetting(this.storageKeys.roundDps, String(this.roundDps));
        this.renderCurrentRows();
      });
    }
    this.autoHideMeterCheckbox = document.querySelector(".autoHideMeterCheckbox");
    if (this.autoHideMeterCheckbox) {
      const storedAutoHide = this.safeGetSetting(this.storageKeys.autoHideMeter) !== "false";
      this.autoHideMeterCheckbox.checked = storedAutoHide;
      this.autoHideMeterCheckbox.addEventListener("change", (event) => {
        const isChecked = !!event.target?.checked;
        this.safeSetSetting(this.storageKeys.autoHideMeter, String(isChecked));
        window.javaBridge?.setAutoHideMeter?.(isChecked);
      });
    }
    // Suspend button setting
    if (this.showSuspendBtnCheckbox) {
      const storedShow = this.safeGetSetting(this.storageKeys.showSuspendBtn) === "true";
      this.showSuspendBtnCheckbox.checked = storedShow;
      this._applySuspendBtnVisibility(storedShow);
      this.showSuspendBtnCheckbox.addEventListener("change", (event) => {
        const isChecked = !!event.target?.checked;
        this.safeSetSetting(this.storageKeys.showSuspendBtn, String(isChecked));
        this._applySuspendBtnVisibility(isChecked);
        if (!isChecked) {
          // When hiding the button, also resume if suspended
          this._setCaptureSuspended(false);
        }
      });
    }
    this.initOverlayLock();
    // Restore suspend state from backend on load
    this._captureSuspended = !!window.javaBridge?.isCaptureSuspended?.();
    this._updateSuspendBtnIcon();
    this._updateSuspendStatusMessage();

    this.initPlayerLimitDropdown();
    this.initEncColumnsSettings();
    if (this.saveRawPacketsCheckbox) {
      const storedSaveRaw = this.safeGetSetting(this.storageKeys.saveRawPackets) === "true";
      this.saveRawPacketsCheckbox.checked = storedSaveRaw;
      this.saveRawPacketsCheckbox.addEventListener("change", (event) => {
        const isChecked = !!event.target?.checked;
        this.safeSetSetting(this.storageKeys.saveRawPackets, String(isChecked));
        window.javaBridge?.setSaveRawPackets?.(isChecked);
      });
    }
    this.initSendLogs();
    this.initReportProblem?.();
    if (this.pinMeToTopCheckbox) {
      this.pinMeToTopCheckbox.checked = this.pinMeToTop;
      this.pinMeToTopCheckbox.addEventListener("change", (event) => {
        const isChecked = !!event.target?.checked;
        this.setPinMeToTop(isChecked, { persist: true });
      });
    }
    if (this.hideOtherNamesCheckbox) {
      this.hideOtherNamesCheckbox.checked = playerNames.hideOthers;
      this.hideOtherNamesCheckbox.addEventListener("change", (event) => {
        this.setHideOtherNames(!!event.target?.checked, { persist: true });
      });
    }
    if (this.accountConnectBtn) {
      this.accountConnectBtn.addEventListener("click", async () => {
        this.accountConnectBtn.disabled = true;
        try {
          const prompt = await window.javaBridge?.accountBeginLink?.();
          if (prompt?.userCode) {
            // Shown, not hidden: the browser may have failed to open, and the
            // code is the only way back into the flow if it did.
            if (this.accountCodeEl) this.accountCodeEl.textContent = prompt.userCode;
            if (this.accountCodeBox) this.accountCodeBox.style.display = "block";
            this.setAccountState(
              window.i18n?.t?.("settings.account.working", "Waiting for approval…")
            );
          }
        } catch (err) {
          const msg = typeof err === "string" ? err : err?.message || String(err);
          this.setAccountState(
            window.i18n?.format?.("settings.account.failed", { error: msg }, `Sign-in failed: ${msg}`)
          );
          this.accountConnectBtn.disabled = false;
        }
      });
    }
    if (this.accountSignOutBtn) {
      this.accountSignOutBtn.addEventListener("click", async () => {
        await window.javaBridge?.accountSignOut?.();
        this.refreshAccountPanel();
      });
    }
    this.refreshAccountPanel();

    // Tray settings, off unless turned on. The backend applies them: the
    // taskbar one at once, the start one at the next launch.
    for (const [selector, key] of [
      [".startInTrayCheckbox", "dpsMeter.startInTray"],
      [".hideFromTaskbarCheckbox", "dpsMeter.hideFromTaskbar"],
    ]) {
      const checkbox = document.querySelector(selector);
      if (!checkbox) continue;
      checkbox.checked = this.safeGetSetting(key) === "true";
      checkbox.addEventListener("change", (event) => {
        this.safeSetSetting(key, String(!!event.target?.checked));
      });
    }

    // Shown where the session has layer-shell, or while it is on, so it can
    // always be turned off again. Unset, it follows the desktop's default
    // (on for KDE Plasma, Hyprland and Sway on Wayland).
    const waylandLayerCheckbox = document.querySelector(".waylandLayerCheckbox");
    if (waylandLayerCheckbox && /Linux/.test(navigator.userAgent)) {
      const savedLayer = this.safeGetSetting(this.storageKeys.waylandLayer);
      waylandLayerCheckbox.checked = savedLayer === "true";
      waylandLayerCheckbox.addEventListener("change", (event) => {
        this.safeSetSetting(this.storageKeys.waylandLayer, String(!!event.target?.checked));
      });
      Promise.resolve(window.__TAURI__?.core?.invoke?.("wayland_layer_state"))
        .then((layer) => {
          if (savedLayer !== "true" && savedLayer !== "false") {
            waylandLayerCheckbox.checked = !!layer?.byDefault;
          }
          if (layer?.supported || waylandLayerCheckbox.checked) {
            document.querySelector(".waylandLayerSetting")?.style.removeProperty("display");
          }
        })
        .catch(() => {});
    }
    if (this.autoUploadCheckbox) {
      // Off unless turned on: an upload publishes a fight.
      this.autoUploadCheckbox.checked =
        this.safeGetSetting(this.storageKeys.autoUpload) === "true";
      this.autoUploadCheckbox.addEventListener("change", (event) => {
        this.safeSetSetting(this.storageKeys.autoUpload, String(!!event.target?.checked));
      });
    }

    if (this.playerNamesBoldCheckbox) {
      this.playerNamesBoldCheckbox.checked = this.mainPlayerNamesBold;
      this.playerNamesBoldCheckbox.addEventListener("change", (event) => {
        const isChecked = !!event.target?.checked;
        this.setMainPlayerNamesBold(isChecked, { persist: true });
      });
    }
    if (this.playerDpsBoldCheckbox) {
      this.playerDpsBoldCheckbox.checked = this.mainPlayerDpsBold;
      this.playerDpsBoldCheckbox.addEventListener("change", (event) => {
        const isChecked = !!event.target?.checked;
        this.setMainPlayerDpsBold(isChecked, { persist: true });
      });
    }
    if (this.meterOpacityInput && this.meterOpacityValue) {
      const defaultOpacity = this.getDefaultMeterFillOpacity();
      const hasStoredOpacity =
        storedMeterOpacity !== null &&
        storedMeterOpacity !== undefined &&
        String(storedMeterOpacity).trim() !== "";
      const resolvedOpacity = hasStoredOpacity
        ? this.normalizeMeterOpacity(storedMeterOpacity, defaultOpacity)
        : defaultOpacity;
      this.applyMeterFillOpacity(resolvedOpacity, { persist: false });
      this.meterOpacityInput.value = String(resolvedOpacity);
      this.meterOpacityValue.textContent = `${resolvedOpacity}%`;
      const stopDrag = (event) => event.stopPropagation();
      this.meterOpacityInput.addEventListener("mousedown", stopDrag);
      this.meterOpacityInput.addEventListener("touchstart", stopDrag, { passive: true });
      this.meterOpacityInput.addEventListener("input", (event) => {
        const value = Number(event.target?.value);
        const next = this.normalizeMeterOpacity(value, defaultOpacity);
        this.meterOpacityValue.textContent = `${next}%`;
        this.applyMeterFillOpacity(next, { persist: true });
      });
    }

    // Target name size
    if (this.bossNameSizeInput && this.bossNameSizeValue) {
      const defaultBossNameSize = this.getDefaultBossNameSize();
      const storedBossNameSize = this.safeGetSetting(this.storageKeys.bossNameSize);
      const resolvedBossNameSize =
        storedBossNameSize !== null && String(storedBossNameSize).trim() !== ""
          ? this.normalizeBossNameSize(storedBossNameSize, defaultBossNameSize)
          : defaultBossNameSize;
      this.applyBossNameSize(resolvedBossNameSize, { persist: false });
      this.bossNameSizeInput.value = String(resolvedBossNameSize);
      this.bossNameSizeValue.textContent = `${resolvedBossNameSize}px`;
      const stopBossNameDrag = (event) => event.stopPropagation();
      this.bossNameSizeInput.addEventListener("mousedown", stopBossNameDrag);
      this.bossNameSizeInput.addEventListener("touchstart", stopBossNameDrag, { passive: true });
      this.bossNameSizeInput.addEventListener("input", (event) => {
        const next = this.normalizeBossNameSize(event.target?.value, defaultBossNameSize);
        this.bossNameSizeValue.textContent = `${next}px`;
        this.applyBossNameSize(next, { persist: true });
      });
    }

    // Window opacity
    if (this.windowOpacityInput && this.windowOpacityValue) {
      const storedWindowOpacity = this.safeGetStorage(this.storageKeys.windowOpacity);
      const resolvedWindowOpacity = storedWindowOpacity !== null && String(storedWindowOpacity).trim() !== ""
        ? Math.max(0, Math.min(100, Math.round(Number(storedWindowOpacity))))
        : 40;
      this.applyWindowOpacity(resolvedWindowOpacity, { persist: false });
      this.windowOpacityInput.value = String(resolvedWindowOpacity);
      this.windowOpacityValue.textContent = `${resolvedWindowOpacity}%`;
      const stopDrag = (event) => event.stopPropagation();
      this.windowOpacityInput.addEventListener("mousedown", stopDrag);
      this.windowOpacityInput.addEventListener("touchstart", stopDrag, { passive: true });
      this.windowOpacityInput.addEventListener("input", (event) => {
        const value = Number(event.target?.value);
        const next = Math.max(0, Math.min(100, Math.round(value)));
        this.windowOpacityValue.textContent = `${next}%`;
        this.applyWindowOpacity(next, { persist: true });
      });
    }

    // Mirror each slider's position into a CSS custom property so the track
    // can paint a filled portion. Presentation only — no setting reads it.
    const syncRangeFill = (input) => {
      if (!input) return;
      const min = Number(input.min);
      const max = Number(input.max);
      const value = Number(input.value);
      if (!Number.isFinite(min) || !Number.isFinite(max) || !Number.isFinite(value) || max <= min) {
        return;
      }
      const pct = Math.max(0, Math.min(100, ((value - min) / (max - min)) * 100));
      input.style.setProperty("--range-pct", `${pct}%`);
    };
    // Both panels: the Details settings menu uses the same slider geometry, so
    // its tracks read the same property.
    const isRange = (el) => el?.tagName === "INPUT" && el.type === "range";
    [this.settingsPanel, this.detailsPanel].forEach((root) => {
      if (!root) return;
      root.querySelectorAll('input[type="range"]').forEach(syncRangeFill);
      root.addEventListener("input", (event) => {
        if (isRange(event.target)) syncRangeFill(event.target);
      });
    });

    this.setupKeybindButtons();

    const currentLanguage = this.i18n?.getLanguage?.() || storedLanguage || "en";
    this.settingsSelections.language = currentLanguage;
    this.settingsSelections.theme = this.theme;

    this.detailsMonitor = this.getDetailsMonitorSetting();
    this.refreshMonitorList().then(() => {
      this.initializeSettingsDropdowns();
      // Re-open the Details window if it was left enabled last session.
      if (this.detailsMonitor !== "off" && window.A2_VIEW !== "details") {
        this.applyDetailsMonitor(this.detailsMonitor, { persist: false });
      }
    });

    this.initializeSettingsDropdowns();

    this.settingsBtn?.addEventListener("click", () => {
      // Settings is its own window now, so the overlay no longer grows to
      // ~820px to contain it.
      if (window.A2_VIEW === "main") {
        window.javaBridge?.openSettingsWindow?.();
        return;
      }
      this.refreshMonitorList().then(() => this.initializeSettingsDropdowns());
      this.toggleSettingsPanel();
    });

    this.settingsClose?.addEventListener("click", () => this.closeSettingsPanel());

    const advancedToggle = document.querySelector(".settingsAdvancedToggle");
    const advancedBody = document.querySelector(".settingsAdvancedBody");
    advancedToggle?.addEventListener("click", () => {
      const isOpen = advancedToggle.classList.toggle("isOpen");
      if (advancedBody) advancedBody.style.display = isOpen ? "" : "none";
    });

    this.resetDetectBtn?.addEventListener("click", () => {
      window.javaBridge?.resetAutoDetection?.();
      this.refreshConnectionInfo();
    });

    // Device selection: auto-detect checkbox + manual device dropdown
    this._autoDetectDevice = !this.safeGetSetting("dpsMeter.manualDevice");
    if (this.autoDetectDeviceCheckbox) {
      this.autoDetectDeviceCheckbox.checked = this._autoDetectDevice;
      this._updateDeviceDropdownState();
      this.autoDetectDeviceCheckbox.addEventListener("change", () => {
        this._autoDetectDevice = this.autoDetectDeviceCheckbox.checked;
        this._updateDeviceDropdownState();
        if (this._autoDetectDevice) {
          window.javaBridge?.setManualDevice?.("");
          this.refreshConnectionInfo();
        } else {
          this._loadDeviceDropdown();
        }
      });
    }

    document.querySelector(".resetAllSettingsBtn")?.addEventListener("click", () => {
      this.resetAllSettings();
    });

    this.supportButton?.addEventListener("click", () => {
      this.openSupportModal();
    });
    this.supportModalClose?.addEventListener("click", () => this.closeSupportModal());
    this.supportModal?.addEventListener("click", (event) => {
      if (event.target === this.supportModal) {
        this.closeSupportModal();
      }
    });
    document.addEventListener("keydown", (event) => {
      if (event.key !== "Escape") return;
      if (this.supportModal?.classList.contains("isOpen")) {
        this.closeSupportModal();
      }
    });
    this.supportActionButtons?.forEach((button) => {
      button.addEventListener("click", () => this.handleSupportAction(button));
    });

    this.settingsVersionLink?.addEventListener("click", () => {
      window.javaBridge?.openBrowser?.("https://github.com/Seralth/Daevalog/releases");
    });

    // Quit is wired in startApp, before anything that can be slow.

    this.updateSettingsVersion();
    this.updateSupportVisibility(currentLanguage);
    this.updateSupportPrimaryAction(currentLanguage);
    this.updateSupportQrImage(this.supportPrimaryButton?.dataset.support || "afdian");
  },

  initializeSettingsDropdowns() {
    const previewThemeVars = (themeId) => {
      const root = document.documentElement;
      const previous = root.dataset.theme;
      root.dataset.theme = themeId;
      const computed = getComputedStyle(root);
      const textColor = computed.getPropertyValue("--text-color").trim() || "#ffffff";
      const nameShadow = computed.getPropertyValue("--player-name-shadow").trim() || "none";
      const rowFill = computed.getPropertyValue("--row-fill").trim() || "#2f2f2f";
      root.dataset.theme = previous || "aion2";
      return { textColor, nameShadow, rowFill };
    };

    const closeAll = () => {
      document.querySelectorAll(".settingsDropdownMenu.isOpen").forEach((menu) => {
        menu.classList.remove("isOpen");
      });
    };

    const setupDropdown = (
      button,
      menu,
      options,
      currentValue,
      onSelect,
      { decorateItem = null, decorateButton = null } = {}
    ) => {
      if (!button || !menu) return;
      const optionList = Array.isArray(options) ? options : [];
      menu.innerHTML = "";

      optionList.forEach((opt) => {
        const item = document.createElement("button");
        item.type = "button";
        item.className = "settingsDropdownItem";
        item.dataset.value = opt.value;
        item.textContent = opt.label;
        if (String(opt.value) === String(currentValue)) {
          item.classList.add("isActive");
        }
        decorateItem?.(item, opt.value);
        item.addEventListener("click", () => {
          onSelect?.(opt.value);
          menu.classList.remove("isOpen");
          this.initializeSettingsDropdowns();
        });
        menu.appendChild(item);
      });

      const selected = optionList.find((opt) => String(opt.value) === String(currentValue)) || optionList[0];
      const textEl = button.querySelector(".settingsDropdownText");
      if (textEl) {
        textEl.textContent = selected?.label || "-";
      }
      button.style.background = "";
      button.style.color = "";
      decorateButton?.(button, selected?.value);
      button.onclick = (event) => {
        event.stopPropagation();
        const wasOpen = menu.classList.contains("isOpen");
        closeAll();
        menu.classList.toggle("isOpen", !wasOpen);
      };
    };

    if (!this._settingsDropdownOutsideBound) {
      document.addEventListener("click", (event) => {
        if (!event.target?.closest?.(".settingsDropdownWrapper")) {
          closeAll();
        }
      });
      this._settingsDropdownOutsideBound = true;
    }

    const languageOptions = [
      { value: "en", label: "English" },
      { value: "de", label: "Deutsch" },
      { value: "es", label: "Español" },
      { value: "fr", label: "Français" },
      { value: "ja", label: "日本語" },
      { value: "ko", label: "한국어" },
      { value: "pt", label: "Português" },
      { value: "ru", label: "Русский" },
      { value: "zh-Hant", label: "繁體中文" },
      { value: "zh-Hans", label: "简体中文" },
    ];

    const themeOptions = [
      { value: "aion2", label: this.i18n?.t("settings.theme.options.aion2", "AION2") },
      { value: "asmodian", label: this.i18n?.t("settings.theme.options.asmodian", "Asmodian") },
      { value: "cogni", label: this.i18n?.t("settings.theme.options.cogni", "Cogni") },
      { value: "elyos", label: this.i18n?.t("settings.theme.options.elyos", "Elyos") },
      { value: "ember", label: this.i18n?.t("settings.theme.options.ember", "Ember") },
      { value: "fera", label: this.i18n?.t("settings.theme.options.fera", "Fera") },
      { value: "frost", label: this.i18n?.t("settings.theme.options.frost", "Frost") },
      { value: "natura", label: this.i18n?.t("settings.theme.options.natura", "Natura") },
      { value: "obsidian", label: this.i18n?.t("settings.theme.options.obsidian", "Obsidian") },
      { value: "varian", label: this.i18n?.t("settings.theme.options.varian", "Varian") },
    ];

    themeOptions.sort((a, b) => a.label.localeCompare(b.label));

    const targetWindowOptions = [
      { value: "5000", label: this.i18n?.t("settings.targetWindow.options.5s", "5 seconds") },
      { value: "10000", label: this.i18n?.t("settings.targetWindow.options.10s", "10 seconds") },
      { value: "15000", label: this.i18n?.t("settings.targetWindow.options.15s", "15 seconds") },
      { value: "20000", label: this.i18n?.t("settings.targetWindow.options.20s", "20 seconds") },
      { value: "30000", label: this.i18n?.t("settings.targetWindow.options.30s", "30 seconds") },
    ];

    const allTargetsWindowOptions = [
      { value: "0", label: this.i18n?.t("settings.allTargetsWindow.options.off", "Off (since zone change)") },
      { value: "30000", label: this.i18n?.t("settings.allTargetsWindow.options.30s", "30 seconds") },
      { value: "60000", label: this.i18n?.t("settings.allTargetsWindow.options.1m", "1 minute") },
      { value: "120000", label: this.i18n?.t("settings.allTargetsWindow.options.2m", "2 minutes") },
      { value: "180000", label: this.i18n?.t("settings.allTargetsWindow.options.3m", "3 minutes") },
      { value: "300000", label: this.i18n?.t("settings.allTargetsWindow.options.5m", "5 minutes") },
    ];

    const defaultMeterModeOptions = TARGET_MODE_ORDER.map((value) => ({ value, label: TARGET_MODE_LABELS[value] }));

    const trainModeOptions = [
      { value: "all", label: this.i18n?.t("settings.trainingMode.options.all", "All") },
      {
        value: "highestDamage",
        label: this.i18n?.t("settings.trainingMode.options.highestDamage", "Highest Damage"),
      },
    ];

    setupDropdown(
      this.languageDropdownBtn,
      this.languageDropdownMenu,
      languageOptions,
      this.settingsSelections.language,
      (value) => {
        if (!value) return;
        this.settingsSelections.language = value;
        this.safeSetStorage(this.storageKeys.language, value);
        this.i18n?.setLanguage?.(value, { persist: true });
      }
    );

    setupDropdown(
      this.themeDropdownBtn,
      this.themeDropdownMenu,
      themeOptions,
      this.settingsSelections.theme,
      (value) => {
        if (!value) return;
        this.settingsSelections.theme = value;
        this.applyTheme(value, { persist: true });
      },
      {
        // Only the colours are set here; size and weight come from the
        // stylesheet so the theme control matches every other dropdown.
        decorateItem: (item, value) => {
          const colors = previewThemeVars(value);
          item.style.background = colors.rowFill;
          item.style.opacity = "1";
          item.style.color = colors.textColor;
          item.style.textShadow = colors.nameShadow;
        },
        decorateButton: (button, value) => {
          const colors = previewThemeVars(value);
          button.style.background = colors.rowFill;
          button.style.opacity = "1";
          button.style.color = colors.textColor;
          button.style.textShadow = colors.nameShadow;
          const textEl = button.querySelector(".settingsDropdownText");
          if (textEl) {
            textEl.style.textShadow = colors.nameShadow;
          }
        },
      }
    );

    setupDropdown(
      this.targetWindowDropdownBtn,
      this.targetWindowDropdownMenu,
      targetWindowOptions,
      this.settingsSelections.targetSelectionWindowMs,
      (value) => {
        if (!value) return;
        this.settingsSelections.targetSelectionWindowMs = value;
        this.safeSetSetting(this.storageKeys.targetSelectionWindowMs, value);
        window.javaBridge?.setTargetSelectionWindowMs?.(value);
        if (!this.isCollapse) this.fetchDps();
      }
    );

    setupDropdown(
      this.allTargetsWindowDropdownBtn,
      this.allTargetsWindowDropdownMenu,
      allTargetsWindowOptions,
      this.settingsSelections.allTargetsWindowMs,
      (value) => {
        if (!value) return;
        this.settingsSelections.allTargetsWindowMs = value;
        this.safeSetSetting(this.storageKeys.allTargetsWindowMs, value);
        window.javaBridge?.setAllTargetsWindowMs?.(value);
        if (!this.isCollapse) this.fetchDps();
      }
    );

    setupDropdown(
      this.trainSelectionModeDropdownBtn,
      this.trainSelectionModeDropdownMenu,
      trainModeOptions,
      this.settingsSelections.trainSelectionMode,
      (value) => {
        if (!value) return;
        this.settingsSelections.trainSelectionMode = value;
        this.trainSelectionMode = value;
        this.safeSetSetting(this.storageKeys.trainSelectionMode, value);
        window.javaBridge?.setTrainSelectionMode?.(value);
        if (!this.isCollapse) this.fetchDps();
      }
    );

    // Monitor picker. Rebuilt every time the settings panel opens so hot-plugged
    // displays appear without a restart.
    {
      const monitors = Array.isArray(this.monitorList) ? this.monitorList : [];
      const offLabel = this.i18n?.t?.("settings.detailsMonitor.off", "Off") ?? "Off";
      const detailsMonitorOptions = [{ value: "off", label: offLabel }].concat(
        monitors.map((m, i) => ({ value: String(m.index), label: this.monitorLabel(m, i) }))
      );
      const current = this.detailsMonitor ?? this.getDetailsMonitorSetting();
      setupDropdown(
        this.detailsMonitorDropdownBtn,
        this.detailsMonitorDropdownMenu,
        detailsMonitorOptions,
        detailsMonitorOptions.some((o) => o.value === current) ? current : "off",
        (value) => {
          this.applyDetailsMonitor(value, { persist: true });
        }
      );
      if (monitors.length <= 1) {
        this.setDetailsMonitorHint(
          this.i18n?.t?.(
            "settings.detailsMonitor.single",
            "Only one display detected — connect a second screen to use this."
          ) ?? "Only one display detected — connect a second screen to use this."
        );
      }
    }

    // One control for both axes of the main window: which skin, and how dense.
    // They were two separate toggles, which made four states the user had to
    // assemble themselves.
    const meterLayoutOptions = [
      { value: "beta", label: this.i18n?.t?.("settings.meterLayout.beta", "Beta UI") ?? "Beta UI" },
      { value: "betaSlim", label: this.i18n?.t?.("settings.meterLayout.betaSlim", "Beta Slim") ?? "Beta Slim" },
      { value: "classic", label: this.i18n?.t?.("settings.meterLayout.classic", "Classic UI") ?? "Classic UI" },
      { value: "classicSlim", label: this.i18n?.t?.("settings.meterLayout.classicSlim", "Classic Slim") ?? "Classic Slim" },
    ];
    setupDropdown(
      this.meterLayoutDropdownBtn,
      this.meterLayoutDropdownMenu,
      meterLayoutOptions,
      this.getMeterLayout(),
      (value) => {
        if (!value) return;
        this.setMeterLayout(value, { persist: true });
      }
    );

    setupDropdown(
      this.defaultMeterModeDropdownBtn,
      this.defaultMeterModeDropdownMenu,
      defaultMeterModeOptions,
      this.settingsSelections.defaultMeterMode,
      (value) => {
        if (!value) return;
        // Saved only. The backend tells every window, and the overlay
        // switches to it (applyRemoteSettingChange); it is the one window that
        // sets the backend's mode.
        this.settingsSelections.defaultMeterMode = value;
        this.safeSetSetting(this.storageKeys.defaultMeterMode, value);
      }
    );
  },

  toggleSettingsPanel() {
    if (!this.settingsPanel) return;
    const isOpen = this.settingsPanel.classList.toggle("isOpen");
    if (isOpen) {
      this.pinnedDetailsRowId = null;
      this.hoveredDetailsRowId = null;
      this.detailsUI?.close?.({ keepPinned: false });
      this.refreshConnectionInfo();
      this.refreshKeybindLabels?.();
      this._loadDeviceDropdown();
    }
  },

  closeSettingsPanel() {
    this.settingsPanel?.classList.remove("isOpen");
  },

  // `manual` marks a name the player typed, which the backend accepts even
  // after the game has named the character (see the name field's handler).
  setUserName(name, { persist = false, syncBackend = false, manual = false } = {}) {
    const previousName = this.USER_NAME;
    const trimmed = String(name ?? "").trim();
    this.USER_NAME = trimmed;
    if (this.characterNameInput && document.activeElement !== this.characterNameInput) {
      this.characterNameInput.value = trimmed;
    }
    if (persist) {
      localStorage.setItem(this.storageKeys.userName, trimmed);
    }
    if (syncBackend) {
      window.javaBridge?.setCharacterName?.(trimmed, manual);
    }
    // A typed correction is not a character switch: the backend has already
    // put the name on the entity that is you, so keep the fight on screen
    // rather than resetting it.
    if (previousName && previousName !== trimmed && !manual) {
      this.refreshDamageData({ reason: "local name update" });
      this.reinitTargetSelection("local name update");
    }
    if (!this.isCollapse) {
      this.fetchDps();
    }
    this.refreshSettingsPanelIfOpen();
  },

  setOnlyShowUser(enabled, { persist = false } = {}) {
    this.onlyShowUser = !!enabled;
    if (persist) {
      localStorage.setItem(this.storageKeys.onlyShowUser, String(this.onlyShowUser));
    }
    if (!this.isCollapse) {
      this.fetchDps();
    }
  },

  setDebugLogging(enabled, { persist = false, syncBackend = false } = {}) {
    this.debugLoggingEnabled = !!enabled;
    if (this.debugLoggingCheckbox && document.activeElement !== this.debugLoggingCheckbox) {
      this.debugLoggingCheckbox.checked = this.debugLoggingEnabled;
    }
    if (persist) {
      this.safeSetSetting(this.storageKeys.debugLogging, String(this.debugLoggingEnabled));
    }
    if (syncBackend) {
      window.javaBridge?.setDebugLoggingEnabled?.(this.debugLoggingEnabled);
    }
  },

  setPinMeToTop(enabled, { persist = false } = {}) {
    this.pinMeToTop = !!enabled;
    if (this.pinMeToTopCheckbox && document.activeElement !== this.pinMeToTopCheckbox) {
      this.pinMeToTopCheckbox.checked = this.pinMeToTop;
    }
    if (persist) {
      this.safeSetSetting(this.storageKeys.pinMeToTop, String(this.pinMeToTop));
    }
    this.renderCurrentRows();
  },

  // Other players by class and number, in every view of this window.
  setHideOtherNames(enabled, { persist = false } = {}) {
    playerNames.hideOthers = !!enabled;
    if (this.hideOtherNamesCheckbox && document.activeElement !== this.hideOtherNamesCheckbox) {
      this.hideOtherNamesCheckbox.checked = playerNames.hideOthers;
    }
    if (persist) {
      this.safeSetSetting(this.storageKeys.hideOtherNames, String(playerNames.hideOthers));
    }
    this.renderCurrentRows();
    this.hideHoverTooltip();
    this.detailsUI?.refresh?.();
    this.detailsUI?.relabelHistoryFight?.();
  },

  getMeterLayout() {
    const base = this.betaUi ? "beta" : "classic";
    return this.slimMode ? `${base}Slim` : base;
  },

  setMeterLayout(value, { persist = false } = {}) {
    const beta = String(value).startsWith("beta");
    const slim = String(value).endsWith("Slim");
    this.setBetaUi(beta, { persist });
    this.setSlimMode(slim, { persist });
  },

  // Beta UI is the redesigned main window and is the default. Switching it off
  // adds body.legacyUi, which activates the pre-redesign skin in styles.css.
  setBetaUi(enabled, { persist = false } = {}) {
    this.betaUi = !!enabled;
    document.body.classList.toggle("legacyUi", !this.betaUi);
    // Both skins show the same placeholder text; only the type scale and the
    // uppercase transform differ, so the fitted size has to be recomputed.
    this.fitBossName();
    if (persist) {
      this.safeSetSetting(this.storageKeys.betaUi, String(this.betaUi));
    }
  },

  setSlimMode(enabled, { persist = false } = {}) {
    this.slimMode = !!enabled;
    document.querySelector(".meter")?.classList.toggle("slim", this.slimMode);
    if (persist) {
      this.safeSetSetting(this.storageKeys.slimMode, String(this.slimMode));
    }
  },

  setMainPlayerNamesBold(enabled, { persist = false } = {}) {
    this.mainPlayerNamesBold = !!enabled;
    if (this.playerNamesBoldCheckbox && document.activeElement !== this.playerNamesBoldCheckbox) {
      this.playerNamesBoldCheckbox.checked = this.mainPlayerNamesBold;
    }
    document.body?.classList.toggle("mainPlayerNamesBold", this.mainPlayerNamesBold);
    if (persist) {
      this.safeSetSetting(this.storageKeys.mainPlayerNamesBold, String(this.mainPlayerNamesBold));
    }
  },

  setMainPlayerDpsBold(enabled, { persist = false } = {}) {
    this.mainPlayerDpsBold = !!enabled;
    if (this.playerDpsBoldCheckbox && document.activeElement !== this.playerDpsBoldCheckbox) {
      this.playerDpsBoldCheckbox.checked = this.mainPlayerDpsBold;
    }
    document.body?.classList.toggle("mainPlayerDpsBold", this.mainPlayerDpsBold);
    if (persist) {
      this.safeSetSetting(this.storageKeys.mainPlayerDpsBold, String(this.mainPlayerDpsBold));
    }
  },

  setTargetSelection(mode, { persist = false, syncBackend = false, reason = "update" } = {}) {
    const previousSelection = this.targetSelection;
    this.targetSelection = TARGET_MODES.includes(mode)
      ? mode
       : "lastHitByMe";
    if (persist) {
      this.safeSetStorage(this.storageKeys.targetSelection, String(this.targetSelection));
    }
    if (syncBackend) {
      window.javaBridge?.setTargetSelection?.(this.targetSelection);
    }
    if (previousSelection !== this.targetSelection) {
      this.logDebug(
        `Target selection changed: "${previousSelection}" -> "${this.targetSelection}" (reason: ${reason}).`
      );
      this._lastTargetSelection = this.targetSelection;
    }
    // Leaving Boss mode drops "Waiting for a boss" at once, rather than on
    // the next update from the backend.
    if (this.targetSelection !== "bossTargets" && this._waitingForBoss) {
      this._waitingForBoss = false;
      this.updateConnectionStatusUi();
    }
    this.updateTargetModeButton();
  },

  applyTheme(themeId, { persist = false } = {}) {
    const normalized = this.availableThemes.includes(themeId) ? themeId : this.availableThemes[0];
    this.theme = normalized;
    document.documentElement.dataset.theme = normalized;
    if (this.settingsSelections) {
      this.settingsSelections.theme = normalized;
    }
    if (persist) {
      this.safeSetSetting(this.storageKeys.theme, normalized);
    }
  },

  setDisplayMode(mode, { persist = false } = {}) {
    this.displayMode = mode === "totalDamage" || mode === "both" ? mode : "dps";
    if (persist) {
      this.safeSetStorage(this.storageKeys.displayMode, this.displayMode);
    }
    this.updateDisplayToggleLabel();
  },

  // Settings are edited in a separate window, so this window has to be told
  // when one changes or it keeps rendering with the value it read at startup.
  //
  // Every window loads the same document, so the control for the setting exists
  // here too, already wired to the handler that applies it. Rather than
  // duplicate that logic, set the control to the incoming value and fire the
  // same event a click would — one code path for local and remote changes.
  //
  // Only the options that change what the meter draws are listed. Custom
  // dropdowns (theme, layout, player limit) are not native inputs and need
  // their own handling, so they are deliberately absent.
  applyRemoteSettingChange(key, value) {
    // A name typed in the Settings window. That window already told the
    // backend; this one only has to stop believing the old name, or it would
    // push the old one straight back.
    if (key === this.storageKeys.userName) {
      const name = String(value ?? "").trim();
      if (name === this.USER_NAME) return;
      this.USER_NAME = name;
      if (this.characterNameInput && document.activeElement !== this.characterNameInput) {
        this.characterNameInput.value = name;
      }
      this.renderCurrentRows();
      return;
    }
    if (key === this.storageKeys.encColumns) {
      this.encColumns = window.MeterColumns?.parse?.(value) ?? null;
      this.syncEncColumnsSettings();
      this.renderCurrentRows();
      return;
    }
    if (key === this.storageKeys.defaultMeterMode) {
      const validModes = TARGET_MODES;
      if (!validModes.includes(value)) return;
      if (this.settingsSelections) this.settingsSelections.defaultMeterMode = value;
      if (window.A2_VIEW !== "main" || value === this.targetSelection) return;
      this.setTargetSelection(value, {
        persist: true,
        syncBackend: true,
        reason: "default meter mode changed",
      });
      if (!this.isCollapse) this.fetchDps();
      return;
    }
    const selector = REMOTE_APPLIED_SETTING_CONTROLS[key];
    if (!selector) return;
    const control = document.querySelector(selector);
    if (!control) return;

    // Bail when the value already matches — this is what stops the echo. The
    // handler below writes the setting straight back, and the backend only
    // broadcasts real changes, so the round trip ends here.
    if (control.type === "checkbox") {
      const next = value !== "false";
      if (control.checked === next) return;
      control.checked = next;
    } else {
      if (String(control.value) === String(value)) return;
      control.value = value;
    }
    control.dispatchEvent(new Event("change", { bubbles: true }));
    if (control.type === "range") {
      control.dispatchEvent(new Event("input", { bubbles: true }));
    }
  },
});
