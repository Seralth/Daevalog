// Settings whose change in the Settings window must redraw the meter, mapped to
// the control that applies them. See DpsApp.applyRemoteSettingChange().
const REMOTE_APPLIED_SETTING_CONTROLS = {
  "dpsMeter.roundDps": ".roundDpsCheckbox",
  "dpsMeter.showTotalDps": ".showTotalDpsCheckbox",
  "dpsMeter.pinMeToTop": ".pinMeToTopCheckbox",
  "dpsMeter.mainPlayerNamesBold": ".playerNamesBoldCheckbox",
  "dpsMeter.mainPlayerDpsBold": ".playerDpsBoldCheckbox",
  "dpsMeter.showPing": ".showPingCheckbox",
  "dpsMeter.bossNameSize": ".bossNameSizeInput",
  "dpsMeter.showSuspendBtn": ".showSuspendBtnCheckbox",
  "dpsMeter.showLockBtn": ".showLockBtnCheckbox",
};

class DpsApp {
  constructor() {
    if (DpsApp.instance) return DpsApp.instance;

    this.POLL_MS = 100;
    this.WINDOW_TITLE_POLL_MS = 3000;
    this.USER_NAME = "";
    this.onlyShowUser = false;
    this.debugLoggingEnabled = false;
    this.pinMeToTop = false;
    this.slimMode = false;
    this.mainPlayerNamesBold = true;
    this.mainPlayerDpsBold = true;
    this.includeMainMeterScreenshot = false;
    this.saveScreenshotToFolder = false;
    this.screenshotFolder = "";
    this.storageKeys = {
      userName: "dpsMeter.userName",
      onlyShowUser: "dpsMeter.onlyShowUser",
      allTargetsWindowMs: "dpsMeter.allTargetsWindowMs",
      encounterTimeoutSec: "dpsMeter.encounterTimeoutSec",
      encColumns: "dpsMeter.encColumns",
      trainSelectionMode: "dpsMeter.trainSelectionMode",
      targetSelectionWindowMs: "dpsMeter.targetSelectionWindowMs",
      meterFillOpacity: "dpsMeter.meterFillOpacity",
      detailsBackgroundOpacity: "dpsMeter.detailsBackgroundOpacity",
      detailsFontSize: "dpsMeter.detailsFontSize",
      detailsIconSize: "dpsMeter.detailsIconSize",
      detailsIncludeMeterScreenshot: "dpsMeter.detailsIncludeMeterScreenshot",
      detailsSaveScreenshotToFolder: "dpsMeter.detailsSaveScreenshotToFolder",
      detailsScreenshotFolder: "dpsMeter.detailsScreenshotFolder",
      detailsHiddenColumns: "dpsMeter.detailsHiddenColumns",
      detailsSeenColumns: "dpsMeter.detailsSeenColumns",
      defaultMeterMode: "dpsMeter.defaultMeterMode",
      targetSelection: "dpsMeter.targetSelection",
      displayMode: "dpsMeter.displayMode",
      language: "dpsMeter.language",
      debugLogging: "dpsMeter.debugLoggingEnabled",
      pinMeToTop: "dpsMeter.pinMeToTop",
      mainPlayerNamesBold: "dpsMeter.mainPlayerNamesBold",
      mainPlayerDpsBold: "dpsMeter.mainPlayerDpsBold",
      showPing: "dpsMeter.showPing",
      showTotalDps: "dpsMeter.showTotalDps",
      roundDps: "dpsMeter.roundDps",
      playerLimit: "dpsMeter.playerLimit",
      theme: "dpsMeter.theme",
      slimMode: "dpsMeter.slimMode",
      autoHideMeter: "dpsMeter.autoHideMeter",
      bossLogs: "dpsMeter.bossLogsEnabled",
      saveRawPackets: "dpsMeter.saveRawPackets",
      autoUpload: "dpsMeter.autoUpload",
      waylandLayer: "dpsMeter.waylandLayer",
      windowOpacity: "dpsMeter.windowOpacity",
      bossNameSize: "dpsMeter.bossNameSize",
      betaUi: "dpsMeter.betaUi",
      detailsMonitor: "dpsMeter.detailsMonitor",
      showSuspendBtn: "dpsMeter.showSuspendBtn",
      showLockBtn: "dpsMeter.showLockBtn",
      overlayLocked: "dpsMeter.overlayLocked",
    };

    this.dpsFormatter = new Intl.NumberFormat("en-US");
    this.lastJson = null;
    this.isCollapse = false;
    this._windowHidden = false;
    this.displayMode = "dps";
    this.betaUi = true;
    this.theme = "aion2";
    this.availableThemes = [
      "aion2",
      "asmodian",
      "cogni",
      "elyos",
      "ember",
      "fera",
      "frost",
      "natura",
      "obsidian",
      "varian",
    ];
    this.supportQrImages = {
      afdian: "./assets/afdian.png",
      kofi: "./assets/kofi.png",
      wechat: "./assets/wechat.png",
    };
    this.jobColorMap = jobColorMap;

    // 빈데이터 덮어쓰기 방지 스냅샷
    this.lastSnapshot = null;
    // reset 직후 서버가 구 데이터 계속 주는 현상 방지
    this.resetPending = false;
    this.refreshPending = false;
    this.refreshPendingStartedAt = 0;

    this.BATTLE_TIME_BASIS = "render";
    this.GRACE_MS = 30000;
    this.GRACE_ARM_MS = 3000;
    this.DETAILS_FONT_SIZE_MIN = 11;
    this.DETAILS_FONT_SIZE_MAX = 20;
    this.DETAILS_ICON_SIZE_MIN = 20;
    this.DETAILS_ICON_SIZE_MAX = 56;


    // battleTime 캐시
    this._battleTimeVisible = false;
    this._lastBattleTimeMs = null;

    this._pollTimer = null;
    this._windowTitleTimer = null;

    this.i18n = window.i18n;
    this.targetSelection = "bossTargets";
    this.listSortDirection = "desc";
    this.lastTargetMode = "";
    this.lastTargetName = "";
    this.lastTargetId = 0;
    this._lastRenderedListSignature = "";
    this._lastRenderedTargetLabel = "";
    this._lastTargetSelection = this.targetSelection;
    this._lastRenderedRowsSummary = null;
    this.localPlayerId = null;
    this.trainSelectionMode = "all";
    this._detailsFlashTimer = null;
    this._meterFlashTimer = null;
    this.pinnedDetailsRowId = null;
    this.hoveredDetailsRowId = null;
    this.setWindowDragFreeze(false);
    this.latestRowsById = new Map();
    this.isWindowDragging = false;
    this.isMeterBarHovered = false;
    this.deferFetchUntilDragEnd = false;
    this.deferFetchUntilHoverEnd = false;
    this.suppressRowInteractionUntilMs = 0;
    this.hoverTooltipCacheByRowId = new Map();
    this.hoverTooltipRequestSeqByRowId = new Map();
    this.hoverTooltipEl = null;
    this.hoverMousePos = { x: 0, y: 0 };
    this.hoverTooltipPendingRowIds = new Set();

    DpsApp.instance = this;
  }

  safeGetStorage(key) {
    try {
      return localStorage.getItem(key);
    } catch (e) {
      globalThis.uiDebug?.log?.("localStorage.get blocked", { key, error: String(e) });
      return null;
    }
  }

  safeSetStorage(key, value) {
    try {
      localStorage.setItem(key, value);
    } catch (e) {
      globalThis.uiDebug?.log?.("localStorage.set blocked", { key, error: String(e) });
    }
  }

  safeGetSetting(key) {
    try {
      const bridgeValue = window.javaBridge?.getSetting?.(key);
      if (bridgeValue !== undefined && bridgeValue !== null) {
        return bridgeValue;
      }
    } catch (e) {
      globalThis.uiDebug?.log?.("getSetting blocked", { key, error: String(e) });
    }
    return this.safeGetStorage(key);
  }

  safeSetSetting(key, value) {
    try {
      window.javaBridge?.setSetting?.(key, value);
    } catch (e) {
      globalThis.uiDebug?.log?.("setSetting blocked", { key, error: String(e) });
    }
    this.safeSetStorage(key, value);
  }


  static createInstance() {
    if (!DpsApp.instance) DpsApp.instance = new DpsApp();
    return DpsApp.instance;
  }

  start() {
    window._dpsApp = this;
    this.elList = document.querySelector(".list");
    this.elBossName = document.querySelector(".bossName");
    this.elBossName.textContent = this.getDefaultTargetLabel();
    this._lastRenderedTargetLabel = this.elBossName.textContent;
    this.elBossHpBar = document.querySelector(".bossHpBar");
    this.elBossHpFill = document.querySelector(".bossHpFill");
    this.elBossHpText = document.querySelector(".bossHpText");
    this.battleTimeRoot = document.querySelector(".battleTime");
    this.analysisStatusEl = document.querySelector(".analysisStatus");
    this.aionRunning = false;
    this.isDetectingPort = false;
    this._connectionStatusOverride = false;

    this.resetBtn = document.querySelector(".resetBtn");
    this.suspendBtn = document.querySelector(".suspendBtn");
    this.lockBtn = document.querySelector(".lockBtn");
    this.headerBtns = document.querySelector(".headerBtns");
    this.targetModeBtn = document.querySelector(".footerBtns .targetModeBtn");
    this.collapseBtn = document.querySelector(".collapseBtn");
    this.metricToggleBtn = document.querySelector(".metricToggleBtn");

    this.bindHeaderButtons();
    this.bindDragToMoveWindow();
    this.bindResizeHandle();
    this.initHoverTooltip();

    this.meterUI = createMeterUI({
      elList: this.elList,
      dpsFormatter: this.dpsFormatter,
      getUserName: () => this.USER_NAME,
      getMetric: (row) => this.getMetricForRow(row),
      getSortDirection: () => this.listSortDirection,
      getPinUserToTop: () => this.pinMeToTop,
      getPlayerLimit: () => this.playerLimit,
      getColumns: () => this.getRowColumns(),
      columnFormats: {
        rate: (v) => this.formatDpsThousands(v),
        amount: (v) => this.formatAbbreviatedNumber(v),
        count: (v) => this.dpsFormatter.format(Math.round(v)),
      },
      getColumnLabel: (key) => {
        const col = window.MeterColumns?.COLUMNS?.find((c) => c.key === key);
        return this.i18n?.t(`meter.columns.${key}`, col?.short ?? key) ?? col?.short ?? key;
      },
      onHoverUserRow: (row, event) => {
        if (this.shouldSuppressRowInteractions()) return;
        this.openHoverDetailsRow(row, event);
      },
      onLeaveUserRow: () => {
        this.hoveredDetailsRowId = null;
        requestAnimationFrame(() => {
          const hoveredRow = this.elList?.querySelector?.(".item:hover");
          if (hoveredRow) return;
          // Only the hover tooltip belongs to hovering. Leaving a row used to
          // close the Details view as well, which killed a Details opened any
          // other way — the target-name path clears pinnedDetailsRowId before
          // opening, so the pin guard never protected it. Hover never opens the
          // panel (openHoverDetailsRow bails when it is already open), so there
          // is nothing here for it to close.
          this.hideHoverTooltip();
        });
      },
      onClickUserRow: (row) => {
        if (!row || this.isWindowDragging) return;
        const rowId = Number(row?.id);
        const pinnedId = Number.isFinite(rowId) && rowId > 0 ? rowId : null;
        this.hideHoverTooltip();
        const options = this.getDefaultDetailsOpenOptions();
        // Details lives in its own window; the overlay only says what to show.
        // Only the in-overlay fallback pins the row — pinning suppresses the
        // hover tooltip, which should stay live when the panel is elsewhere.
        this.openDetailsSurface(
          {
            kind: "row",
            row: {
              id: pinnedId,
              name: row?.name ?? "",
              job: row?.job ?? "",
              isIdentifying: !!row?.isIdentifying,
            },
            defaultTargetAll: !!options.defaultTargetAll,
            defaultTargetId: options.defaultTargetId ?? null,
          },
          () => {
            this.pinnedDetailsRowId = pinnedId;
            this.detailsUI.open(row, { pin: true, ...options });
          }
        );
      },
    });


    const withBacklog = (text) => {
      if (!window.javaBridge?.isRunningFromIde?.()) return text;
      const backlog = window.javaBridge?.getParsingBacklog?.();
      if (!Number.isFinite(backlog)) return text;
      return `${text} (backlog: ${backlog})`;
    };

    const getBattleTimeStatusText = () => {
      if (this._captureSuspended) {
        return this.i18n?.t?.("battleTime.suspended", "App suspended") ?? "App suspended";
      }
      if (!this.aionRunning) {
        const text = this.i18n?.t("battleTime.notRunning", "AION2 not running") ?? "AION2 not running";
        return withBacklog(text);
      }
      if (this.isDetectingPort) {
        const text = this.i18n?.t("connection.detecting", "Detecting AION2 connection...") ??
          "Detecting AION2 connection...";
        return withBacklog(text);
      }
      if (this.battleTime?.getState?.() === "state-idle") {
        const text = this.i18n?.t("battleTime.idle", "Idle") ?? "Idle";
        return withBacklog(text);
      }
      const text = this.i18n?.t("battleTime.analysing", "Monitoring data...") ?? "Monitoring data...";
      return withBacklog(text);
    };

    this.battleTime = createBattleTimeUI({
      rootEl: document.querySelector(".battleTime"),
      tickSelector: ".tick",
      statusSelector: ".status",
      analysisSelector: ".analysisStatus",
      getAnalysisText: getBattleTimeStatusText,
      graceMs: this.GRACE_MS,
      graceArmMs: this.GRACE_ARM_MS,
      idleMs: 60000,
      visibleClass: "isVisible",
    });
    this.battleTime.setVisible(false);
    this.updateConnectionStatusUi();

    this.pingEl = document.querySelector(".pingDisplay");
    this.showPing = this.safeGetSetting(this.storageKeys.showPing) !== "false";
    // Ping is pushed immediately from PingTracker via window._dpsApp.updatePing().
    // A slow fallback poll handles edge cases (e.g. push not wired yet on startup).
    this._pingTimer = setInterval(() => this.updatePing(), 30000);

    this.showTotalDps = this.safeGetSetting(this.storageKeys.showTotalDps) !== "false";
    // Defaults on: `!== "false"` treats "never set" as enabled.
    this.roundDps = this.safeGetSetting(this.storageKeys.roundDps) !== "false";
    // Columns for ENC mode rows; null keeps the usual readout.
    this.encColumns = window.MeterColumns?.parse?.(this.safeGetSetting(this.storageKeys.encColumns)) ?? null;
    this.meterTotalBar = document.querySelector(".meterTotalBar");
    this.meterTotalDpsEl = document.querySelector(".meterTotalDps");
    this.meterTotalDmgEl = document.querySelector(".meterTotalDmg");
    const savedLimit = parseInt(this.safeGetSetting(this.storageKeys.playerLimit), 10);
    this.playerLimit = Number.isFinite(savedLimit) && savedLimit >= 1 ? savedLimit : 6;

    this.detailsPanel = document.querySelector(".detailsPanel");
    this.detailsClose = document.querySelector(".detailsClose");
    this.detailsBackBtn = document.querySelector(".detailsBackBtn");
    this.detailsFightTitleEl = document.querySelector(".detailsFightTitle");
    this.detailsPartyListEl = document.querySelector(".detailsPartyList");
    this.detailsScreenshotBtn = document.querySelector(".detailsScreenshotBtn");
    this.detailsScreenshotNote = document.querySelector(".detailsScreenshotNote");
    this.detailsStatsEl = document.querySelector(".detailsStats");
    this.skillsListEl = document.querySelector(".skills");

    this.detailsUI = createDetailsUI({
      detailsPanel: this.detailsPanel,
      detailsClose: this.detailsClose,
      detailsBackBtn: this.detailsBackBtn,
      detailsFightTitleEl: this.detailsFightTitleEl,
      detailsPartyListEl: this.detailsPartyListEl,
      detailsStatsEl: this.detailsStatsEl,
      skillsListEl: this.skillsListEl,
      dpsFormatter: this.dpsFormatter,
      getDetails: (row, options) => this.getDetails(row, options),
      getDetailsContext: () => this.getDetailsContext(),
      getDungeonId: () => this.lastDungeonId,
      onPinnedRowChange: (rowId) => {
        const nextId = Number(rowId);
        this.pinnedDetailsRowId = Number.isFinite(nextId) && nextId > 0 ? nextId : null;
      },
      // "Back to History" means the History window now, not a list drawn over
      // the fight — a fight window is one of several and the list is a window
      // in its own right. Only the overlay still opens the panel inline.
      onBack: () => {
        if (window.A2_VIEW === "main") {
          this.historyUI?.open?.();
          return;
        }
        // detailsUI.close() already ran, so a fight window is now empty. Going
        // back means "done with this fight": raise the list and dismiss this
        // window rather than leaving a blank one behind.
        const label = window.__TAURI__?.window?.getCurrentWindow?.()?.label || "";
        const isFightWindow = label.startsWith("details-");
        const sent = window.javaBridge?.requestDetailsView?.({ kind: "history" });
        Promise.resolve(sent)
          .catch(() => { if (!isFightWindow) this.historyUI?.open?.(); })
          .then(() => { if (isFightWindow) window.javaBridge?.closeToolWindow?.(); });
      },
    });
    if (this.detailsScreenshotBtn) {
      let screenshotNoteTimer = null;
      this.detailsScreenshotBtn.addEventListener("click", async () => {
        const tooltipText =
          this.i18n?.t("details.screenshot.captured", "Captured Screenshot") ?? "Captured Screenshot";
        const detailsRect = this.detailsPanel?.classList?.contains("open")
          ? this.detailsPanel.getBoundingClientRect()
          : null;
        const includeMeter = !!this.includeMainMeterScreenshot;
        // In its own window, Details cannot measure the meter (a different
        // window), so the backend adds the meter window to the capture. When
        // Details is a panel beside the meter, both are measured here.
        const ownWindow = document.documentElement.classList.contains("detailsWindow");
        const meterRect = !ownWindow ? document.querySelector(".meter")?.getBoundingClientRect?.() : null;
        const rects = [detailsRect, includeMeter ? meterRect : null].filter(Boolean);
        if (!rects.length) return;
        // Kept inside the window: in its own window the panel's side and
        // bottom padding overhang the edges by 10px, and the capture took
        // whatever was behind the window there (issue #6).
        const left = Math.max(0, Math.min(...rects.map((r) => r.left)));
        const top = Math.max(0, Math.min(...rects.map((r) => r.top)));
        const right = Math.min(window.innerWidth, Math.max(...rects.map((r) => r.right)));
        const bottom = Math.min(window.innerHeight, Math.max(...rects.map((r) => r.bottom)));
        const saveFile = !!this.saveScreenshotToFolder;
        const result = await window.javaBridge?.captureScreenshot?.({
          x: left,
          y: top,
          width: Math.max(1, right - left),
          height: Math.max(1, bottom - top),
          scale: window.devicePixelRatio || 1,
          includeMeter: includeMeter && ownWindow,
          saveFile,
          folder: saveFile ? this.screenshotFolder : null,
          filename: saveFile ? this.buildScreenshotFilename() : null,
        });
        const clipboardSuccess = !!result?.clipboard;
        const fileSuccess = !!result?.file;
        if (!this.detailsScreenshotNote) return;
        const note = (key, fallback) => this.i18n?.t(key, fallback) ?? fallback;
        if (!clipboardSuccess && !fileSuccess) {
          this.detailsScreenshotNote.textContent = note("details.screenshot.failed", "Screenshot failed");
        } else if (clipboardSuccess && fileSuccess) {
          this.detailsScreenshotNote.textContent = note("details.screenshot.savedToBoth", "Saved to clipboard + file");
        } else if (fileSuccess) {
          this.detailsScreenshotNote.textContent = note("details.screenshot.savedToFile", "Saved to file");
        } else if (saveFile) {
          this.detailsScreenshotNote.textContent = note("details.screenshot.fileFailed", "Saved to clipboard (file failed)");
        } else {
          this.detailsScreenshotNote.textContent = note("details.screenshot.savedToClipboard", "Saved to clipboard");
        }
        this.detailsScreenshotBtn.setAttribute("title", fileSuccess ? `${tooltipText}: ${result.file}` : tooltipText);
        this.detailsScreenshotNote.classList.add("isVisible");
        // Flash after the capture, so the flash is not in the picture.
        if (clipboardSuccess || fileSuccess) {
          if (this.detailsPanel) this.triggerDetailsFlash();
          if (includeMeter && meterRect) this.triggerMeterFlash();
        }
        if (screenshotNoteTimer) window.clearTimeout(screenshotNoteTimer);
        screenshotNoteTimer = window.setTimeout(() => {
          this.detailsScreenshotNote?.classList.remove("isVisible");
          this.detailsScreenshotNote.textContent = "";
        }, 2000);
      });
    }
    this.setupDetailsPanelSettings();
    this.setupSettingsPanel();
    this.detailsUI?.updateLabels?.();
    this.i18n?.onChange?.((lang) => {
      this.settingsSelections.language = lang;
      this.initializeSettingsDropdowns();
      this.detailsUI?.updateLabels?.();
      this.detailsUI?.refresh?.();
      this.updateDisplayToggleLabel();
      if (this.battleTime?.setAnalysisTextProvider) {
        this.battleTime.setAnalysisTextProvider(getBattleTimeStatusText);
      }
      this.refreshConnectionInfo();
      this.refreshBossLabel();
      this.updateSupportVisibility(lang);
      this.updateSupportPrimaryAction(lang);
      this.updateSupportQrImage(this.supportPrimaryButton?.dataset.support || "afdian");
    });
    this.setupConsoleDebugging();
    this.bindNativeHotkeyBridge();

    const storedDisplayMode = this.safeGetStorage(this.storageKeys.displayMode);
    this.setDisplayMode(storedDisplayMode || this.displayMode, { persist: false });

    // History is a browser you leave open: picking a fight launches it into a
    // window of its own so several can be compared, and the list stays put.
    // Only the in-overlay fallback closes itself, because there the list and
    // the fight would otherwise be stacked in the same 30px-row window.
    this.historyUI = typeof createHistoryUI === "function"
      ? createHistoryUI({
          onOpenFight: (record) => {
            const inPlace = () => {
              this.historyUI?.close?.();
              this.detailsUI?.openHistoryFight?.(record);
            };
            const fightId = record?.id ? String(record.id) : "";
            if (!fightId) {
              inPlace();
              return;
            }
            this.openDetailsSurface({ kind: "fight", fightId }, inPlace);
          },
        })
      : null;

    this.startPolling();
    this.startWindowTitlePolling();
    this.fetchDps();
  }

  bindNativeHotkeyBridge() {
    if (this._nativeHotkeyBridgeBound) return;
    this._nativeHotkeyBridgeBound = true;

    window.addEventListener("nativeResetHotKey", () => {
      this.refreshDamageData({ reason: "native hotkey refresh" });
    });
  }

  nowMs() {
    return typeof performance !== "undefined" ? performance.now() : Date.now();
  }

  shouldSuppressRowInteractions() {
    return this.isWindowDragging || this.nowMs() < Number(this.suppressRowInteractionUntilMs || 0);
  }

  setWindowDragFreeze(active) {
    const enabled = !!active;
    document.documentElement?.classList?.toggle?.("windowDragFreeze", enabled);
    document.body?.classList?.toggle?.("windowDragFreeze", enabled);
    if (enabled) {
      try {
        const selection = window.getSelection?.();
        selection?.removeAllRanges?.();
      } catch {
        // noop
      }
    }
  }

  setMeterHoverFreeze(active) {
    const next = !!active;
    if (this.isMeterBarHovered === next) return;
    this.isMeterBarHovered = next;
    if (!next && this.deferFetchUntilHoverEnd) {
      this.deferFetchUntilHoverEnd = false;
      this.fetchDps();
    }
  }

  safeParseJSON(raw, fallback = {}) {
    if (typeof raw !== "string") {
      return fallback;
    }
    try {
      const value = JSON.parse(raw);
      return value && typeof value === "object" ? value : fallback;
    } catch {
      return fallback;
    }
  }

  startPolling() {
    if (this._pollTimer) return;
    this._pollTimer = setInterval(() => this.fetchDps(), this.POLL_MS);
  }

  startWindowTitlePolling() {
    if (this._windowTitleTimer) return;
    this._windowTitleTimer = setInterval(
      () => this.checkAion2WindowTitle(),
      this.WINDOW_TITLE_POLL_MS
    );
    this.checkAion2WindowTitle();
  }

  parseCharacterNameFromWindowTitle(title) {
    const trimmed = String(title ?? "").trim();
    if (!trimmed) return "";
    if (!trimmed.toLowerCase().startsWith("aion2")) return "";
    const remainder = trimmed.slice(5).trim();
    if (!remainder) return "";
    return remainder.replace(/^[|l:-]+/i, "").trim();
  }

  checkAion2WindowTitle() {
    const title = window.javaBridge?.getAion2WindowTitle?.();
    const running = typeof title === "string" && title.trim().length > 0;
    if (running !== this.aionRunning) {
      this.aionRunning = running;
      this.refreshConnectionInfo();
      this.updateConnectionStatusUi();
    }
    if (!running) return;
    this.syncCharacterNameFromGame();
    // Acting on the title tells the backend a name and resets the meter, so
    // only the overlay does it; the other windows just show the name.
    if (window.A2_VIEW !== "main") return;
    const detectedName = this.parseCharacterNameFromWindowTitle(title);
    // Act on the title only when it changes. It is not kept current (a new
    // character keeps the title of the session it was created in), so a title
    // that merely disagrees with the game's own record of who is playing is
    // stale, and acting on it every poll would reset the meter every poll.
    if (detectedName === this._lastTitleName) return;
    this._lastTitleName = detectedName;
    if (!detectedName || detectedName === this.USER_NAME) return;
    const hadPreviousName = !!this.USER_NAME;
    if (hadPreviousName) {
      // Character switch means new TCP connection — full refresh resets
      // port detection, backend data, and UI so new damage displays immediately.
      this.refreshDamageData({ reason: "character switch" });
    }
    this.setUserName(detectedName, { persist: true, syncBackend: true });
    if (this.characterNameInput && document.activeElement !== this.characterNameInput) {
      this.characterNameInput.value = detectedName;
    }
  }

  // Once the game has sent its self record the backend knows who is playing,
  // and that beats both the window title and the name remembered from last
  // time. Adopt it quietly: this is not a character switch, so none of
  // setUserName's resets apply. Returns whether the game has named the local
  // player (a name of "" is an unnamed tutorial character).
  syncCharacterNameFromGame() {
    const raw = window.javaBridge?.getConnectionInfo?.();
    const info = typeof raw === "string" ? this.safeParseJSON(raw, {}) : {};
    if (!info?.characterNameFromGame) return false;
    const name = String(info.characterName ?? "").trim();
    if (name === this.USER_NAME) return true;
    this.USER_NAME = name;
    if (this.characterNameInput && document.activeElement !== this.characterNameInput) {
      this.characterNameInput.value = name;
    }
    // A tutorial character's missing name is not worth remembering over the
    // last real one.
    if (name) {
      this.safeSetStorage(this.storageKeys.userName, name);
    }
    this.renderCurrentRows();
    return true;
  }

  stopPolling() {
    if (!this._pollTimer) return;
    clearInterval(this._pollTimer);
    this._pollTimer = null;
  }

  /** Called from Kotlin when the window is hidden/shown (hotkey or auto-hide). */
  _setWindowHidden(hidden) {
    this._windowHidden = !!hidden;
    if (hidden) {
      this.stopPolling();
    } else {
      this.startPolling();
    }
  }

  resetAll({ callBackend = true } = {}) {
    this.resetPending = !!callBackend;


    this.lastSnapshot = null;
    this.lastJson = null;
    this.lastTargetMode = "";
    this.lastTargetName = "";
    this.lastTargetId = 0;
    this._lastRenderedListSignature = "";
    this._lastRenderedTargetLabel = "";
    this._lastRenderedRowsSummary = null;

    this._battleTimeVisible = false;
    this._lastBattleTimeMs = null;
    this.battleTime?.reset?.();
    this.battleTime?.setVisible?.(false);

    this.pinnedDetailsRowId = null;
    this.hoveredDetailsRowId = null;
    this.setWindowDragFreeze(false);
    this.setMeterHoverFreeze(false);
    this.detailsUI?.close?.({ keepPinned: false });
    this.meterUI?.onResetMeterUi?.();
    if (this.meterTotalBar) this.meterTotalBar.style.display = "none";

    if (this.elBossName) {
      this.elBossName.textContent = this.getDefaultTargetLabel();
    }
    if (this.battleTimeRoot) {
      this.battleTimeRoot.classList.add("isVisible");
    }
    if (this.analysisStatusEl) {
      if (this._captureSuspended) {
        this.analysisStatusEl.textContent =
          this.i18n?.t?.("battleTime.suspended", "App suspended") ?? "App suspended";
      } else {
        this.analysisStatusEl.textContent =
          this.i18n?.t("battleTime.analysing", "Monitoring data...") ?? "Monitoring data...";
      }
      this.analysisStatusEl.style.display = "";
    }
    this.updateConnectionStatusUi();
    this.logDebug("Target label reset: resetAll invoked.");
    this.logDebug("Meter list reset: resetAll invoked.");
    if (callBackend) {
      window.javaBridge?.resetDps?.();
    }
  }




  initHoverTooltip() {
    const container = document.querySelector(".container");
    if (!container || this.hoverTooltipEl) return;
    const tooltip = document.createElement("div");
    tooltip.className = "hoverDetailsTooltip";
    tooltip.setAttribute("aria-hidden", "true");
    container.appendChild(tooltip);
    this.hoverTooltipEl = tooltip;
  }

  hideHoverTooltip() {
    if (!this.hoverTooltipEl) return;
    this.hoverTooltipEl.classList.remove("isVisible");
    this.hoverTooltipEl.innerHTML = "";
  }

  openHoverDetailsRow(row, event = null) {
    if (!row || this.pinnedDetailsRowId !== null || this.shouldSuppressRowInteractions()) return;
    const rowId = Number(row?.id);
    if (!Number.isFinite(rowId) || rowId <= 0) return;
    if (event && Number.isFinite(event.clientX) && Number.isFinite(event.clientY)) {
      this.hoverMousePos = { x: event.clientX, y: event.clientY };
    }
    const isSameRow = this.hoveredDetailsRowId === rowId;
    this.hoveredDetailsRowId = rowId;
    if (this.detailsUI?.isOpen?.()) {
      return;
    }
    // Skip redundant tooltip renders when still hovering the same row
    if (isSameRow && this.hoverTooltipEl?.classList.contains("isVisible")) {
      this.positionHoverTooltip();
      return;
    }
    this.detailsUI?.close?.({ keepPinned: false });
    this.applyHoverTooltip(row, { forceRefresh: !isSameRow });
  }

  getJobColor(job) {
    return this.jobColorMap[String(job || "")] || "";
  }

  renderHoverTooltip(details, row, rowEl) {
    if (!this.hoverTooltipEl || !rowEl) return;
    const skills = Array.isArray(details?.skills) ? details.skills.slice(0, 5) : [];
    const tooltipState = details?.state || "empty";
    const stateFallback = {
      loading: "Loading...",
      empty: "No skill data for this fight",
      error: "Could not load skills",
    }[tooltipState] || "No skill data for this fight";
    const stateText = this.i18n?.t(`details.hoverTooltip.${tooltipState}`, stateFallback) ?? stateFallback;
    const dps = Number(row?.dps) || 0;
    const dpsText = `${this.dpsFormatter.format(dps)}${this.i18n?.t("meter.dpsSuffix", "/s") ?? "/s"}`;
    const totalDamage = Number(row?.totalDamage) || 0;
    const totalDamageText = this.dpsFormatter.format(totalDamage);

    const skillsHtml = skills
      .map((skill, index) => {
        const name = String(skill?.name || "-").replace(/</g, "&lt;").replace(/>/g, "&gt;");
        const dmg = this.dpsFormatter.format(Number(skill?.dmg) || 0);
        const theostoneNameColor = window.skillIcons?.getTheostoneNameColor?.(skill) || "";
        const skillColor = theostoneNameColor || this.getJobColor(skill?.job || row?.job);
        const skillStyle = skillColor ? ` style="color:${skillColor}"` : "";
        const iconHtml = `<img class="skillIcon isPlaceholder" alt="" src="data:image/gif;base64,R0lGODlhAQABAAAAACw=" onerror="window.skillIcons&&window.skillIcons.handleImgError&&window.skillIcons.handleImgError(this)">`;
        return `<div class="hoverDetailsTooltipSkill"><span class="idx">${index + 1}.</span><span class="name">${iconHtml}<span class="skillName"${skillStyle}>${name}</span></span><span class="dmg">${dmg}</span></div>`;
      })
      .join("");

    const tooltipName = String(row?.name || "-").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    const tooltipClassIcon = row?.job ? `<img class="hoverDetailsTooltipClassIcon" src="./assets/${row.job}.png" alt="" onerror="this.style.display='none'">` : "";
    this.hoverTooltipEl.innerHTML = `
      <div class="hoverDetailsTooltipHeader">${tooltipClassIcon}${tooltipName}</div>
      <div class="hoverDetailsTooltipStats">
        <span>${this.i18n?.t("header.display.dps", "DPS") ?? "DPS"}: ${dpsText}</span>
        <span>${this.i18n?.t("details.stats.totalDamage", "Total Damage") ?? "Total Damage"}: ${totalDamageText}</span>
      </div>
      <div class="hoverDetailsTooltipSkills">${skillsHtml || `<div class="hoverDetailsTooltipSkill muted">${stateText}</div>`}</div>
    `;
    // Apply cached skill icons to tooltip img elements
    if (window.skillIcons?.applyIconToImage && skills.length) {
      const iconEls = this.hoverTooltipEl.querySelectorAll(".hoverDetailsTooltipSkill .skillIcon");
      iconEls.forEach((img, i) => {
        if (skills[i]) window.skillIcons.applyIconToImage(img, skills[i]);
      });
    }
    this.hoverTooltipEl.classList.add("isVisible");
    this.positionHoverTooltip(rowEl);
  }

  positionHoverTooltip(rowEl = null) {
    const tooltip = this.hoverTooltipEl;
    if (!tooltip) return;
    const container = tooltip.offsetParent;
    const origin = container?.getBoundingClientRect?.() || { left: 0, top: 0 };
    const screen = window.screen || {};
    const availableWidth = Math.max(32, (screen.availLeft || 0) + (screen.availWidth || window.innerWidth)
      - (window.screenX || 0) - origin.left);
    const availableHeight = Math.max(32, (screen.availTop || 0) + (screen.availHeight || window.innerHeight)
      - (window.screenY || 0) - origin.top);
    const margin = 8;
    const gap = 12;
    tooltip.style.maxWidth = `${Math.min(380, availableWidth - margin * 2)}px`;
    tooltip.style.minWidth = `${Math.min(200, availableWidth - margin * 2)}px`;
    tooltip.style.maxHeight = `${availableHeight - margin * 2}px`;
    const rowBounds = rowEl?.getBoundingClientRect?.();
    const x = (this.hoverMousePos?.x ?? rowBounds?.left ?? origin.left) - origin.left;
    const y = (this.hoverMousePos?.y ?? rowBounds?.bottom ?? origin.top) - origin.top;
    const width = tooltip.offsetWidth;
    const height = tooltip.offsetHeight;
    let left = x + gap;
    let top = y + gap;
    if (left + width + margin > availableWidth) left = x - width - gap;
    if (top + height + margin > availableHeight) top = y - height - gap;
    tooltip.style.left = `${Math.max(margin, Math.min(availableWidth - width - margin, left))}px`;
    tooltip.style.top = `${Math.max(margin, Math.min(availableHeight - height - margin, top))}px`;
    window.javaBridge?.updateOverlaySize?.();
  }

  applyHoverTooltip(row, { forceRefresh = false } = {}) {
    const rowId = Number(row?.id);
    if (!Number.isFinite(rowId) || rowId <= 0) return;
    const rowEl = this.elList?.querySelector?.(`.item[data-row-id="${rowId}"]`);
    if (!rowEl) return;

    const cached = this.hoverTooltipCacheByRowId.get(rowId);
    if (cached) {
      this.renderHoverTooltip(cached, row, rowEl);
      if (!forceRefresh) return;
    }

    this.renderHoverTooltip({ skills: [], state: "loading" }, row, rowEl);
    if (!forceRefresh && this.hoverTooltipPendingRowIds.has(rowId)) {
      return;
    }
    const requestSeq = (this.hoverTooltipRequestSeqByRowId.get(rowId) || 0) + 1;
    this.hoverTooltipRequestSeqByRowId.set(rowId, requestSeq);
    this.hoverTooltipPendingRowIds.add(rowId);

    this.getDetails(row, { maxSkills: 5, showSkillIcons: false, summaryOnly: true })
      .then((details) => {
        this.hoverTooltipPendingRowIds.delete(rowId);
        const currentSeq = this.hoverTooltipRequestSeqByRowId.get(rowId);
        if (currentSeq !== requestSeq || this.hoveredDetailsRowId !== rowId) return;
        const lightweightDetails = { skills: Array.isArray(details?.skills) ? details.skills.slice(0, 5) : [] };
        this.hoverTooltipCacheByRowId.set(rowId, lightweightDetails);
        this.renderHoverTooltip(lightweightDetails, row, rowEl);
      })
      .catch((error) => {
        this.hoverTooltipPendingRowIds.delete(rowId);
        const currentSeq = this.hoverTooltipRequestSeqByRowId.get(rowId);
        if (currentSeq !== requestSeq || this.hoveredDetailsRowId !== rowId) return;
        window.javaBridge?.logToDebug?.(`Hover skill details failed: ${error?.message || error}`);
        this.renderHoverTooltip({ skills: [], state: "error" }, row, rowEl);
      });
  }




  _isAnyPanelOpen() {
    return this.settingsPanel?.classList.contains("isOpen")
      || this.historyUI?.isOpen?.()
      || this.detailsUI?.isOpen?.();
  }

  fetchDps() {
    if (this.isCollapse || this._windowHidden) return;
    if (this.isWindowDragging) {
      this.deferFetchUntilDragEnd = true;
      return;
    }
    if (this.isMeterBarHovered) {
      this.deferFetchUntilHoverEnd = true;
      return;
    }
    const now = this.nowMs();
    // Throttle meter DOM updates while a panel is open (1s instead of 100ms)
    if (this._isAnyPanelOpen()) {
      if (this._lastPanelRenderAt && now - this._lastPanelRenderAt < 1000) return;
      this._lastPanelRenderAt = now;
    } else {
      this._lastPanelRenderAt = 0;
    }
    const raw = window.dpsData?.getDpsData?.();
    // globalThis.uiDebug?.log?.("getBattleDetail", raw);

    // 값이 없으면 타이머 숨김
    if (typeof raw !== "string") {
      this._rawLastChangedAt = now;

      this._lastBattleTimeMs = null;
      this._battleTimeVisible = false;
      this.battleTime.setVisible(false);
      this.updateConnectionStatusUi();
      return;
    }

    if (raw === this.lastJson && !this.refreshPending) {
      // Staleness watchdog: if the backend JSON hasn't changed for 10+ seconds
      // but the backend IS still parsing packets, something may be stuck.
      // Force a re-process to recover from invisible backend exceptions.
      if (!this._rawLastChangedAt) this._rawLastChangedAt = now;
      const staleMs = now - this._rawLastChangedAt;
      if (staleMs > 5_000) {
        const lastParsed = Number(window.javaBridge?.getLastParsedAtMs?.());
        const parsingRecently = Number.isFinite(lastParsed) && lastParsed > 0 && (Date.now() - lastParsed) < 15_000;
        if (parsingRecently) {
          this.logDebug(`Staleness watchdog: data unchanged for ${Math.round(staleMs/1000)}s while backend parsing active — forcing re-render`);
          this.lastJson = null;
          this._rawLastChangedAt = now;
          return;
        }
      }

      const shouldBeVisible = this._battleTimeVisible && !this.isCollapse;

      this.battleTime.setVisible(shouldBeVisible);
      if (shouldBeVisible) {
        this.battleTime.update(now, this._lastBattleTimeMs);
      }

      this.updateConnectionStatusUi();
      return;
    }
    this._rawLastChangedAt = now;

    const previousTargetName = this.lastTargetName;
    const previousTargetMode = this.lastTargetMode;
    const previousTargetId = this.lastTargetId;
    const {
      rows,
      targetName,
      targetMode,
      battleTimeMs,
      targetId,
      localPlayerId,
      targetMaxHp,
      targetTotalDamage,
      targetCurrentHp,
      dungeonId,
    } = this.buildRowsFromPayload(raw);
    // Boss mode with no boss engaged: say what it is waiting for. The rows of
    // the last fight can still be on screen (the meter keeps them), so this
    // goes by the target alone. Set before anything below can return early,
    // so switching back to Boss mode shows it again on the next update.
    const waitingForBoss = targetMode === "bossTargets" && !(Number(targetId) > 0);
    if (waitingForBoss !== Boolean(this._waitingForBoss)) {
      this._waitingForBoss = waitingForBoss;
      this.updateConnectionStatusUi();
    }
    if (this.refreshPending) {
      const pendingAgeMs = Math.max(0, now - (Number(this.refreshPendingStartedAt) || 0));
      const allowFallbackResume = rows.length > 0 && pendingAgeMs >= 1000;

      if (rows.length > 0 && !allowFallbackResume) {
        return;
      }

      this.refreshPending = false;
      this.refreshPendingStartedAt = 0;

      if (rows.length === 0) {
        this.lastJson = raw;
        this.lastSnapshot = [];
        this._lastRenderedListSignature = "";
        this._lastRenderedRowsSummary = null;
        this.meterUI?.onResetMeterUi?.();
        return;
      }
    }

    this.lastJson = raw;
    this.setLocalPlayerIdFromBackend(localPlayerId);
    this._lastBattleTimeMs = battleTimeMs;
    this.lastTargetMode = targetMode;
    this.lastTargetName = targetName;
    this.lastTargetId = targetId;

    if (
      targetId !== this._lastLoggedTargetId ||
      targetMode !== this._lastLoggedTargetMode ||
      targetName !== this._lastLoggedTargetName
    ) {
      const reasons = [];
      if (targetId !== this._lastLoggedTargetId) reasons.push("targetId changed");
      if (targetMode !== this._lastLoggedTargetMode) reasons.push("mode changed");
      if (targetName !== this._lastLoggedTargetName) reasons.push("name changed");
      console.log("[Target Lock]", {
        targetId,
        targetName,
        targetMode,
        reason: reasons.join(", ") || "initial",
      });
      this._lastLoggedTargetId = targetId;
      this._lastLoggedTargetMode = targetMode;
      this._lastLoggedTargetName = targetName;
    }


    const showByServer = rows.length > 0;
    if (this.resetPending) {
      const resetAck = rows.length === 0;

      this._battleTimeVisible = false;
      this.battleTime.setVisible(false);

      if (!resetAck) {
        return;
      }

      this.resetPending = false;
    }
    const isOutOfCombat = this.isOutOfCombatState();
    // 빈값은 ui 안덮어씀
    let rowsToRender = rows;
    const listReasons = [];
    if (rows.length === 0) {
      if (this.lastSnapshot) rowsToRender = this.lastSnapshot;
      else {
        this._battleTimeVisible = false;
        this.battleTime.setVisible(false);
        this.updateConnectionStatusUi();
        return;
      }
    } else if (!isOutOfCombat) {
      this.lastSnapshot = rows;
    } else if (this.lastSnapshot) {
      const updatedSnapshot = this.updateSnapshotNicknameForUser(rows, this.lastSnapshot);
      this.lastSnapshot = updatedSnapshot;
      rowsToRender = updatedSnapshot;
    } else {
      this.lastSnapshot = rows;
      rowsToRender = rows;
      listReasons.push("idle baseline captured");
    }

    // 타이머 표시 여부
    const showByRender = rowsToRender.length > 0;
    const showBattleTime = this.BATTLE_TIME_BASIS === "server" ? showByServer : showByRender;

    let nextBattleTimeMs = battleTimeMs;
    if (
      Number.isFinite(Number(nextBattleTimeMs)) &&
      Number.isFinite(Number(this._lastBattleTimeMs)) &&
      targetId === previousTargetId &&
      Number(nextBattleTimeMs) < Number(this._lastBattleTimeMs)
    ) {
      nextBattleTimeMs = this._lastBattleTimeMs;
    }

    const eligible = showBattleTime && Number.isFinite(Number(nextBattleTimeMs));

    this._battleTimeVisible = eligible;
    const shouldBeVisible = eligible && !this.isCollapse;

    this.battleTime.setVisible(shouldBeVisible);

    if (shouldBeVisible) {
      this.battleTime.update(now, nextBattleTimeMs);
    }

    this.updateConnectionStatusUi();
    if (targetMode === "trainTargets" && this.isLocalUserIdentified()) {
      rowsToRender = rowsToRender.filter((row) => row.name === this.USER_NAME);
    }
    // render
    this.lastDungeonId = dungeonId;
    const nextTargetLabel = this.getTargetLabel({ targetId, targetName, targetMode, dungeonId });
    if (this.elBossName) {
      if (this.elBossName.textContent !== nextTargetLabel) {
        this.elBossName.textContent = nextTargetLabel;
        this.fitBossName();
      }
      this.elBossName.classList.toggle("isAllTargets", targetMode === "allTargets");
    }
    this.updateBossHpBar(targetMaxHp, targetTotalDamage, targetCurrentHp);
    if (
      nextTargetLabel !== this._lastRenderedTargetLabel ||
      previousTargetName !== targetName ||
      previousTargetMode !== targetMode
    ) {
      const reasons = [];
      if (previousTargetName !== targetName || previousTargetMode !== targetMode) {
        reasons.push("payload target changed");
      }
      if (!targetName) {
        reasons.push(`default label for mode ${targetMode || "unknown"}`);
      }
      this.logDebug(
        `Target label changed: "${this._lastRenderedTargetLabel}" -> "${nextTargetLabel}" (reason: ${reasons.join(
          "; "
        )}).`
      );
      this._lastRenderedTargetLabel = nextTargetLabel;
    }
    const rowsSummary = this.getRowsSummary(rowsToRender);
    if (rowsSummary.listSignature !== this._lastRenderedListSignature) {
      const changeReasons = this.describeRowsChange(rowsSummary, this._lastRenderedRowsSummary);
      const reasonText = [...changeReasons, ...listReasons].filter(Boolean).join("; ");
      this.logDebug(
        `Meter list changed (${rowsToRender.length} rows). reason: ${
          reasonText || "list membership changed"
        }.`
      );
      this._lastRenderedListSignature = rowsSummary.listSignature;
      this._lastRenderedRowsSummary = rowsSummary;
    }
    this.latestRowsById = new Map(rowsToRender.map((row) => [String(row.id), row]));
    this.hoverTooltipCacheByRowId.clear();
    this.updateMeterTotalBar(rowsToRender);
    this.meterUI.updateFromRows(rowsToRender);
  }

  buildRowsFromPayload(raw) {
    const payload = this.safeParseJSON(raw, {});
    const targetName = typeof payload?.targetName === "string" ? payload.targetName : "";
    const targetMode = typeof payload?.targetMode === "string" ? payload.targetMode : "";
    const targetIdRaw = payload?.targetId;
    const targetId = Number.isFinite(Number(targetIdRaw)) ? Number(targetIdRaw) : 0;
    const localPlayerIdRaw = payload?.localPlayerId;
    const localPlayerId = Number.isFinite(Number(localPlayerIdRaw))
      ? Number(localPlayerIdRaw)
      : null;

    const mapObj = payload?.map && typeof payload.map === "object" ? payload.map : {};
    const rows = this.buildRowsFromMapObject(mapObj, localPlayerId);

    const battleTimeMsRaw = payload?.battleTime;
    const battleTimeMs = Number.isFinite(Number(battleTimeMsRaw)) ? Number(battleTimeMsRaw) : null;

    const targetMaxHp = Number.isFinite(Number(payload?.targetMaxHp)) ? Number(payload.targetMaxHp) : 0;
    const targetTotalDamage = Number.isFinite(Number(payload?.targetTotalDamage))
      ? Number(payload.targetTotalDamage)
      : 0;
    const dungeonId = Math.trunc(Number(payload?.dungeonId)) || 0;
    const targetCurrentHp = Number.isFinite(Number(payload?.targetCurrentHp))
      ? Number(payload.targetCurrentHp)
      : -1;

    return {
      rows,
      targetName,
      targetMode,
      battleTimeMs,
      targetId,
      localPlayerId,
      targetMaxHp,
      targetTotalDamage,
      targetCurrentHp,
      dungeonId,
    };
  }

  buildRowsFromMapObject(mapObj, localPlayerId = null) {
    const rows = [];
    const localId = Number(localPlayerId) > 0 ? Number(localPlayerId) : null;

    for (const [id, value] of Object.entries(mapObj || {})) {
      const numericId = Number(id);
      if (Number.isFinite(numericId) && numericId <= 0) {
        continue;
      }
      const isObj = value && typeof value === "object";

      const job = isObj ? (value.job ?? "") : "";
      const nickname = isObj ? (value.nickname ?? "") : "";
      const idText = String(id);
      const hasNickname = !!nickname && nickname !== idText;
      const isIdentifying = !hasNickname;
      const name = hasNickname ? nickname : idText;

      const dpsRaw = isObj ? value.dps : value;
      const dps = Math.trunc(Number(dpsRaw));
      const totalDamage = Math.trunc(Number(isObj ? value.amount : 0));

      // 소수점 한자리
      const contribRaw = isObj ? Number(value.damageContribution) : NaN;
      const damageContribution = Number.isFinite(contribRaw)
        ? Math.round(contribRaw * 10) / 10
        : NaN;

      if (!Number.isFinite(dps)) {
        continue;
      }

      // Combat power comes from the party roster packet, so it only exists for
      // players actually in your party; 0 means "unknown", not "zero CP".
      const combatPower = Math.trunc(Number(isObj ? value.combatPower : 0)) || 0;
      const num = (v) => (isObj && Number.isFinite(Number(v)) ? Number(v) : 0);

      rows.push({
        id: String(id),
        name,
        job,
        dps,
        totalDamage,
        damageContribution,
        combatPower,
        // For the ENC columns: DPS over the player's own active time and over
        // the last 10/30/60 s, direct hits, crits among them, biggest hit.
        activeDps: num(value?.activeDps),
        last10Dps: num(value?.last10Dps),
        last30Dps: num(value?.last30Dps),
        last60Dps: num(value?.last60Dps),
        hits: num(value?.hits),
        critHits: num(value?.critHits),
        maxHit: num(value?.maxHit),
        isUser: name === this.USER_NAME || numericId === localId,
        isIdentifying,
      });
    }

    const dedupedByName = new Map();
    for (const row of rows) {
      const key = String(row.name ?? "");
      if (!key) continue;
      const existing = dedupedByName.get(key);
      if (!existing) {
        dedupedByName.set(key, row);
        continue;
      }
      const scoreRow = (candidate) => {
        let score = 0;
        if (candidate.job) score += 2;
        if (!candidate.isIdentifying) score += 1;
        return score;
      };
      const existingScore = scoreRow(existing);
      const nextScore = scoreRow(row);
      if (nextScore > existingScore) {
        dedupedByName.set(key, row);
        continue;
      }
      if (nextScore < existingScore) {
        continue;
      }
      const existingId = Number(existing.id);
      const nextId = Number(row.id);
      if (!Number.isFinite(existingId) || (Number.isFinite(nextId) && nextId > existingId)) {
        dedupedByName.set(key, row);
      }
    }

    return Array.from(dedupedByName.values());
  }

  isOutOfCombatState() {
    const state = this.battleTime?.getState?.();
    return state === "state-idle" || state === "state-ended";
  }

  updateSnapshotNicknameForUser(rows, snapshot) {
    if (!Array.isArray(snapshot) || snapshot.length === 0) return snapshot;
    const localId = Number(this.localPlayerId);
    if (!Number.isFinite(localId) || localId <= 0) return snapshot;
    const incoming = Array.isArray(rows)
      ? rows.find((row) => Number(row?.id) === localId && !row.isIdentifying)
      : null;
    if (!incoming) return snapshot;
    return snapshot.map((row) => {
      if (Number(row?.id) !== localId) return row;
      return {
        ...row,
        name: incoming.name,
        isIdentifying: false,
        isUser: incoming.isUser,
      };
    });
  }

  getDefaultMeterFillOpacity() {
    const raw = getComputedStyle(document.documentElement)
      .getPropertyValue("--meter-fill-opacity")
      .trim();
    const value = Number.parseFloat(raw);
    if (!Number.isFinite(value)) return 100;
    return Math.round(value * 100);
  }

  normalizeMeterOpacity(value, fallback = 100) {
    const numeric = Number(value);
    if (!Number.isFinite(numeric)) return fallback;
    return Math.min(100, Math.max(10, Math.round(numeric)));
  }

  applyMeterFillOpacity(percent, { persist } = {}) {
    const normalized = this.normalizeMeterOpacity(percent, 100);
    document.documentElement.style.setProperty("--meter-fill-opacity", String(normalized / 100));
    if (persist) {
      this.safeSetSetting(this.storageKeys.meterFillOpacity, String(normalized));
    }
  }

  getDefaultBossNameSize() {
    const raw = getComputedStyle(document.documentElement)
      .getPropertyValue("--boss-name-size")
      .trim();
    const value = Number.parseFloat(raw);
    return Number.isFinite(value) ? value : 15;
  }

  normalizeBossNameSize(value, fallback = 15) {
    const numeric = Number(value);
    if (!Number.isFinite(numeric)) return fallback;
    return Math.min(20, Math.max(8, Math.round(numeric * 2) / 2));
  }

  applyBossNameSize(px, { persist } = {}) {
    const normalized = this.normalizeBossNameSize(px, 15);
    document.documentElement.style.setProperty("--boss-name-size", `${normalized}px`);
    // fitBossName reads the computed size, so re-run it: a bigger size may now
    // overflow and need shrinking, a smaller one may have room to grow back.
    this.fitBossName();
    if (persist) {
      this.safeSetSetting(this.storageKeys.bossNameSize, String(normalized));
    }
  }

  applyWindowOpacity(percent, { persist } = {}) {
    const normalized = Math.max(0, Math.min(100, Math.round(Number(percent))));
    document.documentElement.style.setProperty("--window-opacity", String(normalized / 100));
    if (persist) {
      this.safeSetSetting(this.storageKeys.windowOpacity, String(normalized));
    }
  }

  resetAllSettings() {
    for (const key of Object.values(this.storageKeys)) {
      try {
        localStorage.removeItem(key);
      } catch (_) {}
    }
    try {
      window.javaBridge?.clearAllSettings?.();
    } catch (_) {}
    window.location.reload();
  }

  triggerDetailsFlash() {
    if (!this.detailsPanel) return;
    this.detailsPanel.classList.remove("flash");
    void this.detailsPanel.offsetWidth;
    this.detailsPanel.classList.add("flash");
    if (this._detailsFlashTimer) window.clearTimeout(this._detailsFlashTimer);
    this._detailsFlashTimer = window.setTimeout(() => {
      this.detailsPanel?.classList.remove("flash");
    }, 1000);
  }

  triggerMeterFlash() {
    const meterEl = document.querySelector(".meter");
    if (!meterEl) return;
    meterEl.classList.remove("flash");
    void meterEl.offsetWidth;
    meterEl.classList.add("flash");
    if (this._meterFlashTimer) window.clearTimeout(this._meterFlashTimer);
    this._meterFlashTimer = window.setTimeout(() => {
      meterEl.classList.remove("flash");
    }, 1000);
  }

  captureMainMeterScreenshot() {
    const meterRect = document.querySelector(".meter")?.getBoundingClientRect?.();
    if (!meterRect) return;
    const scale = window.devicePixelRatio || 1;
    const success = window.javaBridge?.captureScreenshotToClipboard?.(
      meterRect.left,
      meterRect.top,
      meterRect.width,
      meterRect.height,
      scale
    );
    if (success) {
      this.triggerMeterFlash();
    }
  }

  reinitTargetSelection(reason) {
    this.resetTargetTrackingState();
    window.javaBridge?.restartTargetSelection?.();
    this.refreshPending = false;
    this.refreshPendingStartedAt = 0;
    this.resetPending = false;
    this.lastJson = null;
    this.lastSnapshot = null;
    this._lastRenderedListSignature = "";
    this._lastRenderedRowsSummary = null;
    this.setTargetSelection(this.targetSelection, { persist: false, syncBackend: true, reason });
    if (!this.isCollapse) {
      this.fetchDps();
    }
  }

  resetTargetTrackingState() {
    this.lastTargetMode = "";
    this.lastTargetName = "";
    this.lastTargetId = 0;
    this._lastRenderedTargetLabel = "";
    this._lastLoggedTargetId = null;
    this._lastLoggedTargetMode = null;
    this._lastLoggedTargetName = null;
  }

  // Who "you" are is the backend's call: it has the game's own word, which
  // a window does not. Each window only reads it from the dps payload, for the
  // highlight and the Settings field, and never sends it back. Windows used to
  // bind a row by name or echo the id they last saw; after a zone change that
  // bound a stale id, or a party placeholder at startup.
  setLocalPlayerIdFromBackend(actorId) {
    const id = Number(actorId);
    const next = Number.isFinite(id) && id > 0 ? Math.trunc(id) : null;
    if (this.localPlayerId === next) return;
    this.logDebug(`Local player id ${this.localPlayerId ?? "none"} -> ${next ?? "none"}.`);
    this.localPlayerId = next;
    if (this.localActorIdInput && document.activeElement !== this.localActorIdInput) {
      this.localActorIdInput.value = next ? String(next) : "";
    }
  }

  getDetailsContext() {
    const raw = window.dpsData?.getDetailsContext?.();
    if (!raw) return null;
    if (typeof raw === "string") {
      return this.safeParseJSON(raw, null);
    }
    return raw;
  }

  async getDetails(
    row,
    { targetId = null, attackerIds = null, totalTargetDamage = null, showSkillIcons = false, maxSkills = null, summaryOnly = false } = {}
  ) {
    let raw = null;
    let backendFiltered = false;
    if (window._historyDetailsOverride) {
      raw = window._historyDetailsOverride;
    } else if (targetId && window.dpsData?.getTargetDetails) {
      const payload = Array.isArray(attackerIds) ? JSON.stringify(attackerIds) : "";
      raw = await window.dpsData.getTargetDetails(targetId, payload);
      backendFiltered = true;
    } else {
      raw = await window.dpsData?.getBattleDetail?.(row.id, summaryOnly);
    }
    let detailObj = raw;
    // globalThis.uiDebug?.log?.("getBattleDetail", detailObj);

    if (typeof raw === "string") detailObj = this.safeParseJSON(raw, {});
    if (!detailObj || typeof detailObj !== "object") detailObj = {};

    const skills = [];
    let totalDmg = 0;

    let totalTimes = 0;
    let totalCrit = 0;
    let totalParry = 0;
    let totalBack = 0;
    let totalPerfect = 0;
    let totalDouble = 0;
    let totalMultiHitCount = 0;
    let totalMultiHitDamage = 0;
    let totalRegen = 0;

    const pushSkill = ({
      codeKey,
      name,
      time,
      dmg,
      crit = 0,
      parry = 0,
      back = 0,
      frontal = 0,
      perfect = 0,
      double = 0,
      shieldBlock = 0,
      ironWall = 0,
      regeneration = 0,
      perfectBlock = 0,
      miss = 0,
      resist = 0,
      regen = 0,
      multiHitCount = 0,
      multiHitDamage = 0,
      multiHitHits = 0,
      minDmg = 0,
      maxDmg = 0,
      countForTotals = true,
      job = "",
      actorId = null,
      isDot = false,
      hitTimestamps = null,
      specs = null,
    }) => {
      const dmgInt = Math.trunc(Number(String(dmg ?? "").replace(/,/g, ""))) || 0;
      // A skill that only missed or was resisted still gets its row.
      if (dmgInt <= 0 && !(Number(miss) > 0 || Number(resist) > 0)) {
        return;
      }

      const t = Number(time) || 0;

      totalDmg += dmgInt;
      totalRegen += Number(regen) || 0;
      if (countForTotals) {
        totalTimes += t;
        totalCrit += Number(crit) || 0;
        totalParry += Number(parry) || 0;
        totalBack += Number(back) || 0;
        totalPerfect += Number(perfect) || 0;
        totalDouble += Number(double) || 0;
        totalMultiHitCount += Number(multiHitCount) || 0;
        totalMultiHitDamage += Number(multiHitDamage) || 0;
      }
      skills.push({
        code: String(codeKey),
        name,
        time: t,
        crit: Number(crit) || 0,
        parry: Number(parry) || 0,
        back: Number(back) || 0,
        frontal: Number(frontal) || 0,
        perfect: Number(perfect) || 0,
        double: Number(double) || 0,
        shieldBlock: Number(shieldBlock) || 0,
        ironWall: Number(ironWall) || 0,
        regeneration: Number(regeneration) || 0,
        perfectBlock: Number(perfectBlock) || 0,
        miss: Number(miss) || 0,
        resist: Number(resist) || 0,
        regen: Number(regen) || 0,
        multiHitCount: Number(multiHitCount) || 0,
        multiHitDamage: Number(multiHitDamage) || 0,
        multiHitHits: Number(multiHitHits) || 0,
        minDmg: Number(minDmg) || 0,
        maxDmg: Number(maxDmg) || 0,
        dmg: dmgInt,
        job,
        actorId,
        isDot,
        hitTimestamps: Array.isArray(hitTimestamps) ? hitTimestamps : [],
        specs: Array.isArray(specs) ? specs : null,
      });
    };

    const detailSkills = Array.isArray(detailObj?.skills) ? detailObj.skills : null;
    const attackerIdSet = !backendFiltered && Array.isArray(attackerIds) && attackerIds.length > 0
      ? new Set(attackerIds.map(Number))
      : null;
    if (detailSkills) {
      for (const value of detailSkills) {
        if (!value || typeof value !== "object") continue;
        // Filter by selected player when viewing history (skip when backend already filtered)
        if (attackerIdSet) {
          const skillActorId = Number(value.actorId);
          if (!Number.isFinite(skillActorId) || !attackerIdSet.has(skillActorId)) continue;
        }
        const code = String(value.code ?? "");
        const nameRaw = typeof value.name === "string" ? value.name.trim() : "";
        const translatedName = (this.i18n?.getSkillName?.(code, nameRaw) ?? nameRaw).replace(/^Theostone:/, "Theo:");
        const baseName =
          translatedName ||
          this.i18n?.format?.("skills.fallback", { code }, `Skill ${code}`) ||
          `Skill ${code}`;
        const dotName =
          this.i18n?.format?.("skills.dot", { name: baseName }, `${baseName} - DOT`) ||
          `${baseName} - DOT`;
        const isDot = !!value.isDot;
        const actorId = Number(value.actorId);

        pushSkill({
          codeKey: isDot ? `${code}-dot` : code,
          name: isDot ? dotName : baseName,
          time: value.time,
          dmg: value.dmg,
          crit: value.crit,
          parry: value.parry,
          back: value.back,
          frontal: value.frontal,
          perfect: value.perfect,
          double: value.double,
          shieldBlock: value.shieldBlock,
          ironWall: value.ironWall,
          // Saved before the rename.
          regeneration: value.regeneration ?? value.smite,
          perfectBlock: value.perfectBlock ?? value.powershard,
          miss: value.miss,
          resist: value.resist,
          regen: value.regen,
          multiHitCount: value.multiHitCount,
          multiHitDamage: value.multiHitDamage,
          multiHitHits: value.multiHitHits,
          minDmg: value.minDmg,
          maxDmg: value.maxDmg,
          job: value.job ?? "",
          countForTotals: !isDot,
          actorId: Number.isFinite(actorId) ? actorId : null,
          isDot,
          hitTimestamps: Array.isArray(value.hitTimestamps) ? value.hitTimestamps : [],
          specs: Array.isArray(value.specs) ? value.specs : null,
        });
      }
    } else {
      for (const [code, value] of Object.entries(detailObj)) {
        if (!value || typeof value !== "object") continue;

        const nameRaw = typeof value.skillName === "string" ? value.skillName.trim() : "";
        const translatedName = (this.i18n?.getSkillName?.(code, nameRaw) ?? nameRaw).replace(/^Theostone:/, "Theo:");
        const baseName =
          translatedName ||
          this.i18n?.format?.("skills.fallback", { code }, `Skill ${code}`) ||
          `Skill ${code}`;
        const dotName =
          this.i18n?.format?.("skills.dot", { name: baseName }, `${baseName} - DOT`) ||
          `${baseName} - DOT`;

        // 일반 피해
        pushSkill({
          codeKey: code,
          name: baseName,
          time: value.times,
          dmg: value.damageAmount,
          crit: value.critTimes,
          parry: value.parryTimes,
          back: value.backTimes,
          frontal: value.frontalTimes,
          perfect: value.perfectTimes,
          double: value.doubleTimes,
          regen: value.healAmount,
        });

        // 도트피해
        if (Number(String(value.dotDamageAmount ?? "").replace(/,/g, "")) > 0) {
          pushSkill({
            codeKey: `${code}-dot`, // 유니크키
            name: dotName,
            time: value.dotTimes,
            dmg: value.dotDamageAmount,
            countForTotals: false,
          });
        }
      }
    }


    if (Number.isFinite(Number(maxSkills)) && Number(maxSkills) > 0 && skills.length > Number(maxSkills)) {
      skills.sort((a, b) => (Number(b?.dmg) || 0) - (Number(a?.dmg) || 0));
      skills.length = Number(maxSkills);
    }
    const pct = (num, den) => {
      if (den <= 0) return 0;
      return Math.round((num / den) * 1000) / 10;
    };
    const perActorStatsMap = new Map();
    for (const skill of skills) {
      if (!Number.isFinite(Number(skill.actorId))) continue;
      const actorId = Number(skill.actorId);
      const entry =
        perActorStatsMap.get(actorId) || {
          actorId,
          job: skill.job ?? "",
          totalDmg: 0,
          totalTimes: 0,
          totalCrit: 0,
          totalParry: 0,
          totalBack: 0,
          totalPerfect: 0,
          totalDouble: 0,
          totalHits: 0,
          totalRegen: 0,
          multiHitCount: 0,
          multiHitDamage: 0,
        };
      entry.totalDmg += Number(skill.dmg) || 0;
      entry.totalRegen += Number(skill.regen) || 0;
      entry.multiHitCount += Number(skill.multiHitCount) || 0;
      entry.multiHitDamage += Number(skill.multiHitDamage) || 0;
      if (!skill.isDot) {
        entry.totalHits += Number(skill.time) || 0;
        entry.totalTimes += Number(skill.time) || 0;
        entry.totalCrit += Number(skill.crit) || 0;
        entry.totalParry += Number(skill.parry) || 0;
        entry.totalBack += Number(skill.back) || 0;
        entry.totalPerfect += Number(skill.perfect) || 0;
        entry.totalDouble += Number(skill.double) || 0;
      }
      if (!entry.job && skill.job) {
        entry.job = skill.job;
      }
      perActorStatsMap.set(actorId, entry);
    }
    const fallbackContribution = Number(row?.damageContribution);
    const baseTotalDamage = Number.isFinite(Number(totalTargetDamage))
      ? Number(totalTargetDamage)
      : Number(detailObj?.totalTargetDamage);
    const contributionPct =
      Number.isFinite(baseTotalDamage) && baseTotalDamage > 0
        ? (totalDmg / baseTotalDamage) * 100
        : fallbackContribution;
    const battleTimeMsRaw = Number(detailObj?.battleTime);
    const combatTime = Number.isFinite(battleTimeMsRaw)
      ? formatBattleTime(battleTimeMsRaw)
      : this.battleTime?.getCombatTimeText?.() ?? "00:00";

    const perActorStats = [...perActorStatsMap.values()]
      .map((entry) => ({
        actorId: entry.actorId,
        job: entry.job,
        totalDmg: entry.totalDmg,
        totalTimes: entry.totalTimes,
        totalCrit: entry.totalCrit,
        totalParry: entry.totalParry,
        totalBack: entry.totalBack,
        totalPerfect: entry.totalPerfect,
        totalDouble: entry.totalDouble,
        totalHits: entry.totalHits,
        totalRegen: entry.totalRegen,
        multiHitCount: entry.multiHitCount,
        multiHitDamage: entry.multiHitDamage,
        contributionPct:
          Number.isFinite(baseTotalDamage) && baseTotalDamage > 0
            ? (entry.totalDmg / baseTotalDamage) * 100
            : 0,
        totalCritPct: pct(entry.totalCrit, entry.totalTimes),
        totalParryPct: pct(entry.totalParry, entry.totalTimes),
        totalBackPct: pct(entry.totalBack, entry.totalTimes),
        totalPerfectPct: pct(entry.totalPerfect, entry.totalTimes),
        totalDoublePct: pct(entry.totalDouble, entry.totalTimes),
        combatTime,
      }))
      .sort((a, b) => b.totalDmg - a.totalDmg);

    // Process healing (DMG/HEAL toggle): filter by selected member, resolve skill
    // names, and compute heal-mode stat totals. Mirrors the damage skill pipeline.
    const healSkillsRaw = Array.isArray(detailObj?.healSkills) ? detailObj.healSkills : [];
    const healSkillsOut = [];
    let totalHeal = 0;
    let healTicks = 0;
    let healHotTicks = 0;
    const healActors = new Set();
    for (const v of healSkillsRaw) {
      if (!v || typeof v !== "object") continue;
      const aId = Number(v.actorId);
      if (attackerIdSet && (!Number.isFinite(aId) || !attackerIdSet.has(aId))) continue;
      const code = String(v.code ?? "");
      const nameRaw = typeof v.name === "string" ? v.name.trim() : "";
      const name = (this.i18n?.getSkillName?.(code, nameRaw) ?? nameRaw) || `Skill ${code}`;
      const amt = Number(v.dmg) || 0;
      const ticks = Number(v.time) || 0;
      const isHot = !!v.isDot;
      if (amt <= 0) continue;
      totalHeal += amt;
      healTicks += ticks;
      if (isHot) healHotTicks += ticks;
      if (Number.isFinite(aId)) healActors.add(aId);
      healSkillsOut.push({
        actorId: Number.isFinite(aId) ? aId : null,
        code: Number.isFinite(Number(code)) ? Number(code) : v.code,
        name,
        dmg: amt,
        time: ticks,
        isDot: isHot,
        crit: 0, parry: 0, back: 0, frontal: 0, perfect: 0, double: 0,
        regen: 0, multiHitCount: 0, multiHitDamage: 0, multiHitHits: 0,
        minDmg: 0, maxDmg: 0, job: v.job ?? "", specs: null, hitTimestamps: [],
      });
    }
    const healBattleMs = Number.isFinite(battleTimeMsRaw) ? battleTimeMsRaw : 0;
    const healPerSecText = healBattleMs > 0
      ? `${this.formatAbbreviatedNumber(totalHeal / healBattleMs * 1000)}`
      : "-";

    return {
      totalDmg,
      totalHeal,
      healTicks,
      healHotTicks,
      healSkillCount: healSkillsOut.length,
      healPerSecText,
      contributionPct,
      totalCritPct: pct(totalCrit, totalTimes),
      totalParryPct: pct(totalParry, totalTimes),
      totalBackPct: pct(totalBack, totalTimes),
      totalPerfectPct: pct(totalPerfect, totalTimes),
      totalDoublePct: pct(totalDouble, totalTimes),
      totalHits: totalTimes,
      multiHitCount: totalMultiHitCount,
      multiHitDamage: totalMultiHitDamage,
      multiHitPct: pct(totalMultiHitCount, totalTimes),
      totalRegen,
      combatTime,
      battleTimeMs: Number.isFinite(battleTimeMsRaw) ? battleTimeMsRaw : 0,
      maxHp: Number(detailObj?.maxHp) || 0,

      skills,
      // Per-actor/skill healing (DMG/HEAL toggle), filtered to the selected member
      // and name-resolved (see processing above).
      healSkills: healSkillsOut,
      showSkillIcons,
      perActorStats,
      showCombinedTotals: !attackerIds || attackerIds.length === 0,
      pingHistory: Array.isArray(detailObj?.pingHistory) ? detailObj.pingHistory : [],
    };
  }

  bindHeaderButtons() {
    this.logoBtn = document.querySelector(".bossIcon");
    this.collapseBtn?.addEventListener("click", () => {
      this.listSortDirection = this.listSortDirection === "asc" ? "desc" : "asc";
      this.renderCurrentRows();

      const iconName =
        this.listSortDirection === "asc" ? "arrow-down-wide-narrow" : "arrow-up-wide-narrow";
      const iconEl =
        this.collapseBtn.querySelector("svg") || this.collapseBtn.querySelector("[data-lucide]");
      if (!iconEl) {
        return;
      }

      iconEl.setAttribute("data-lucide", iconName);
      window.lucide?.createIcons?.({ root: this.collapseBtn });
    });
    this.resetBtn?.addEventListener("click", () => {
      this.refreshDamageData({ reason: "manual refresh" });
    });
    this.suspendBtn?.addEventListener("click", () => {
      this._setCaptureSuspended(!this._captureSuspended);
    });
    this.lockBtn?.addEventListener("click", () => {
      this._setOverlayLocked(!this._overlayLocked);
    });
    this.targetModeBtn?.addEventListener("click", () => {
      const modes = TARGET_MODE_CYCLE;
      const currentIndex = modes.indexOf(this.targetSelection);
      const nextMode = modes[(currentIndex + 1) % modes.length];
      console.log("[Target Mode Toggle]", {
        from: this.targetSelection,
        to: nextMode,
      });
      this.setTargetSelection(nextMode, {
        persist: true,
        syncBackend: true,
        reason: "header toggle",
      });
      if (!this.isCollapse) {
        this.fetchDps();
      }
    });
    this.metricToggleBtn?.addEventListener("click", () => {
      // DPS -> total damage -> both -> DPS.
      const order = ["dps", "totalDamage", "both"];
      const nextMode = order[(order.indexOf(this.displayMode) + 1) % order.length];
      this.setDisplayMode(nextMode, { persist: true });
      this.renderCurrentRows();
    });
    this.logoBtn?.addEventListener("click", () => {
      this.captureMainMeterScreenshot();
    });
    this.logoBtn?.setAttribute("data-no-drag", "true");

    // Click on boss name area → open Details for current mob (all players) or history
    // The target name is a plain label now: no click target and no data-no-drag,
    // so the whole top of the window is grab-and-drag surface. Details is opened
    // from a player row, History from its own header button.

    const historyBtn = document.querySelector(".historyBtn");
    if (historyBtn) {
      // History is part of the Details surface, not the overlay: opening it
      // here would make the 30px-tall meter grow to swallow a fight list. It
      // goes to the Details window, which then swaps between the list and a
      // fight in place. The in-overlay panel remains the fallback for when
      // that window cannot be reached at all.
      const openHistory = () => {
        this.openDetailsSurface({ kind: "history" }, () => this.historyUI?.open?.());
      };
      historyBtn.addEventListener("click", () => {
        if (this.isWindowDragging) return;
        openHistory();
      });
      historyBtn.addEventListener("keydown", (event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          openHistory();
        }
      });
      historyBtn.setAttribute("data-no-drag", "true");
    }
  }

  setupConsoleDebugging() {
    if (this._consoleDebuggingEnabled) {
      return;
    }
    this._consoleDebuggingEnabled = true;

    window.addEventListener("error", (event) => {
      console.error("[UI Error]", event.error || event.message, event);
    });

    window.addEventListener("unhandledrejection", (event) => {
      console.error("[UI Promise Rejection]", event.reason || event);
    });

    document.addEventListener("click", (event) => {
      const target = event.target;
      if (!target || typeof target.closest !== "function") return;
      const menuTarget =
        target.closest("[role='menu']") ||
        target.closest(".detailsSettingsMenu") ||
        target.closest(".detailsDropdownMenu") ||
        target.closest(".settingsPanel");
      if (!menuTarget) return;
      const menuClass = menuTarget.className || menuTarget.getAttribute?.("role") || "menu";
      const targetLabel =
        target.getAttribute?.("aria-label") ||
        target.getAttribute?.("data-i18n") ||
        target.textContent?.trim() ||
        target.tagName;
      console.log("[UI Menu Click]", { menu: menuClass, target: targetLabel });
    });
  }



  buildScreenshotFilename() {
    const now = new Date();
    const pad = (value) => String(value).padStart(2, "0");
    const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}_${pad(
      now.getHours()
    )}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
    return `AION2_DPS_${stamp}.png`;
  }

  updateDisplayToggleLabel() {
    if (!this.metricToggleBtn) return;
    const [labelKey, labelFallback, ariaKey, ariaFallback] = {
      totalDamage: ["header.display.total", "DMG", "header.display.ariaDamage", "Showing total damage"],
      both: ["header.display.both", "BOTH", "header.display.ariaBoth", "Showing DPS and total damage"],
      dps: ["header.display.dps", "DPS", "header.display.ariaDps", "Showing DPS"],
    }[this.displayMode];
    const label = this.i18n?.t(labelKey, labelFallback) ?? labelFallback;
    const ariaLabel = this.i18n?.t(ariaKey, ariaFallback) ?? ariaFallback;
    this.metricToggleBtn.textContent = label;
    this.metricToggleBtn.setAttribute("aria-label", ariaLabel);
  }

  // Boss remaining-HP bar. There is no live boss current-HP packet, so remaining
  // is derived from spawn-time max HP minus the damage the meter has tracked
  // against this target. Hidden unless a single boss target with known max HP.
  updateBossHpBar(maxHp, totalDamage, currentHp) {
    if (!this.elBossHpBar) return;
    const max = Number(maxHp) || 0;
    if (max <= 0) {
      if (this.elBossHpBar.style.display !== "none") {
        this.elBossHpBar.style.display = "none";
      }
      return;
    }
    // Prefer the real current-HP feed when we have one (>= 0); otherwise fall back
    // to deriving remaining = max - tracked damage.
    const live = Number(currentHp);
    const remaining =
      Number.isFinite(live) && live >= 0
        ? Math.min(max, Math.max(0, live))
        : Math.max(0, max - Math.max(0, Number(totalDamage) || 0));
    const pct = Math.max(0, Math.min(100, (remaining / max) * 100));
    if (this.elBossHpBar.style.display === "none") {
      this.elBossHpBar.style.display = "";
    }
    if (this.elBossHpFill) {
      this.elBossHpFill.style.width = `${pct.toFixed(1)}%`;
    }
    // Color shifts as the boss is whittled down.
    this.elBossHpBar.classList.toggle("isMid", pct > 25 && pct <= 50);
    this.elBossHpBar.classList.toggle("isLow", pct <= 25);
    if (this.elBossHpText) {
      // Split to the two ends of the band: the percentage rides the fill,
      // the absolute HP sits on the track. A single centred label was
      // getting sliced in half by the fill's leading edge at mid HP.
      const pctEl = this.elBossHpText.querySelector(".bossHpPct");
      const amtEl = this.elBossHpText.querySelector(".bossHpAmt");
      if (pctEl && amtEl) {
        pctEl.textContent = `${Math.round(pct)}%`;
        amtEl.textContent = `${this.formatAbbreviatedNumber(remaining)} / ${this.formatAbbreviatedNumber(max)}`;
      } else {
        this.elBossHpText.textContent = `${this.formatAbbreviatedNumber(remaining)} · ${Math.round(pct)}%`;
      }
    }
  }

  formatAbbreviatedNumber(value) {
    const n = Number(value);
    if (!Number.isFinite(n)) return "-";
    const abs = Math.abs(n);
    const units = [
      { value: 1e12, suffix: "t" },
      { value: 1e9, suffix: "b" },
      { value: 1e6, suffix: "m" },
      { value: 1e3, suffix: "k" },
    ];
    for (const unit of units) {
      if (abs >= unit.value) {
        const scaled = (n / unit.value).toFixed(2);
        const trimmed = scaled.replace(/\.?0+$/, "");
        return `${trimmed}${unit.suffix}`;
      }
    }
    return this.dpsFormatter.format(n);
  }

  // DPS to the nearest thousand: 1,012,326 reads as "1,012k", 554,874 as "555k".
  // Same rule as the combat power beside the name, so the two numbers on a row
  // are at the same precision. Below 500 the abbreviation would collapse to
  // "0k", so show the raw figure there instead. Turned off by the "Round DPS"
  // setting, which falls back to the exact figure.
  formatDpsThousands(value) {
    const n = Number(value);
    if (!Number.isFinite(n)) return "-";
    if (!this.roundDps) return this.dpsFormatter.format(Math.round(n));
    const thousands = Math.round(n / 1000);
    return thousands > 0
      ? `${this.dpsFormatter.format(thousands)}k`
      : this.dpsFormatter.format(Math.round(n));
  }

  refreshDamageData({ reason = "refresh" } = {}) {
    this.refreshPending = true;
    this.refreshPendingStartedAt = this.nowMs();
    this.lastSnapshot = null;
    this.lastJson = null;
    this.lastTargetMode = "";
    this.lastTargetName = "";
    this.lastTargetId = 0;
    this._lastRenderedListSignature = "";
    this._lastRenderedTargetLabel = "";
    this._lastRenderedRowsSummary = null;
    this.pinnedDetailsRowId = null;
    this.hoveredDetailsRowId = null;
    this.setWindowDragFreeze(false);
    this.setMeterHoverFreeze(false);
    this.detailsUI?.close?.({ keepPinned: false });
    this.lastSnapshot = [];
    this._lastRenderedRowsSummary = null;
    this._lastRenderedListSignature = "";
    this.meterUI?.onResetMeterUi?.();
    this.renderCurrentRows();

    if (this.elBossName) {
      this.elBossName.textContent = this.getDefaultTargetLabel(this.targetSelection);
    }

    const lastParsedAtMs = Number(window.javaBridge?.getLastParsedAtMs?.());
    if (Number.isFinite(lastParsedAtMs) && lastParsedAtMs > 0) {
      const idleMs = Date.now() - lastParsedAtMs;
      if (idleMs > 30_000) {
        window.javaBridge?.resetAutoDetection?.();
      }
    }

    window.javaBridge?.resetDps?.();
    window.javaBridge?.restartTargetSelection?.();
    this.logDebug(`Damage data refreshed (${reason}).`);
  }

  // The ENC columns the rows show, or null for the usual readout: other
  // modes, and ENC until a choice is saved.
  getRowColumns() {
    return this.targetSelection === "encounter" ? this.encColumns : null;
  }

  // The ENC column switches in Settings. Before a choice is saved they show
  // what the row shows today; the first change saves the whole set. The last
  // switch on cannot be turned off, so a row always has a number.
  initEncColumnsSettings() {
    const list = document.querySelector(".encColumnsList");
    const Columns = window.MeterColumns;
    if (!list || !Columns || list.dataset.wired) return;
    list.dataset.wired = "1";
    const current = () =>
      this.encColumns ?? Columns.defaultsFor(this.safeGetStorage(this.storageKeys.displayMode) || this.displayMode);
    this.encColumnsCheckboxes = new Map();
    for (const col of Columns.COLUMNS) {
      const label = document.createElement("label");
      label.className = "settingsToggle";
      const text = document.createElement("span");
      text.className = "settingsToggleLabel settingsLabel";
      text.dataset.i18n = `settings.encColumns.${col.key}`;
      text.textContent = this.i18n?.t(`settings.encColumns.${col.key}`, col.label) ?? col.label;
      const control = document.createElement("span");
      control.className = "settingsToggleControl";
      const input = document.createElement("input");
      input.type = "checkbox";
      input.dataset.column = col.key;
      const track = document.createElement("span");
      track.className = "settingsToggleTrack";
      track.setAttribute("aria-hidden", "true");
      control.append(input, track);
      label.append(text, control);
      list.appendChild(label);
      this.encColumnsCheckboxes.set(col.key, input);
      input.addEventListener("change", () => {
        const chosen = Columns.COLUMNS.map((c) => c.key)
          .filter((key) => this.encColumnsCheckboxes.get(key)?.checked);
        if (!chosen.length) {
          input.checked = true;
          return;
        }
        this.encColumns = chosen;
        this.safeSetSetting(this.storageKeys.encColumns, Columns.serialize(chosen));
        this.syncEncColumnsSettings();
        this.renderCurrentRows();
      });
    }
    this._encColumnsCurrent = current;
    this.syncEncColumnsSettings();
  }

  syncEncColumnsSettings() {
    if (!this.encColumnsCheckboxes || !this._encColumnsCurrent) return;
    const chosen = new Set(this._encColumnsCurrent());
    for (const [key, input] of this.encColumnsCheckboxes) {
      input.checked = chosen.has(key);
      input.disabled = chosen.size === 1 && chosen.has(key);
    }
  }

  getMetricForRow(row) {
    if (this.displayMode === "totalDamage") {
      const totalDamage = Number(row?.totalDamage) || 0;
      return {
        value: totalDamage,
        text: this.formatAbbreviatedNumber(totalDamage),
      };
    }
    const dps = Number(row?.dps) || 0;
    const dpsText = `${this.formatDpsThousands(dps)}${this.i18n?.t("meter.dpsSuffix", "/s") ?? "/s"}`;
    if (this.displayMode === "both") {
      // "408k (13k/s)", as a player asked: damage leads, so the bars follow it.
      const totalDamage = Number(row?.totalDamage) || 0;
      return { value: totalDamage, text: `${this.formatAbbreviatedNumber(totalDamage)} (${dpsText})` };
    }
    return { value: dps, text: dpsText };
  }

  updateMeterTotalBar(rows) {
    if (!this.meterTotalBar) return;
    if (!this.showTotalDps || !Array.isArray(rows) || rows.length <= 1) {
      this.meterTotalBar.style.display = "none";
      return;
    }
    const totalDmg = rows.reduce((sum, r) => sum + (Number(r?.totalDamage) || 0), 0);
    const totalDps = rows.reduce((sum, r) => sum + (Number(r?.dps) || 0), 0);
    this.meterTotalBar.style.display = "";
    if (this.meterTotalDpsEl) {
      // Matches the per-row readout directly above it; a full-precision total
      // over abbreviated rows reads as two different units.
      this.meterTotalDpsEl.textContent = `${this.formatDpsThousands(totalDps)}${this.i18n?.t("meter.dpsSuffix", "/s") ?? "/s"}`;
    }
    if (this.meterTotalDmgEl) {
      this.meterTotalDmgEl.textContent = this.formatAbbreviatedNumber(totalDmg);
    }
  }

  // "Send logs to a2tools.app": the newest packet captures, for the A2 Tools
  // developer. Packet logs are raw game traffic, names included, so it asks
  // first, every time. Daevalog cannot see logs once they are sent there.
  initSendLogs() {
    const btn = this.sendLogsBtn;
    const status = this.sendLogsStatus;
    if (!btn || btn.dataset.wired) return;
    btn.dataset.wired = "1";
    this.openLogFolderBtn?.addEventListener("click", () => window.javaBridge?.openDataFolder?.());
    const t = (key, fallback, vars) =>
      vars ? (window.i18n?.format?.(key, vars, fallback) ?? fallback)
           : (window.i18n?.t?.(key, fallback) ?? fallback);
    const show = (text) => {
      if (!status) return;
      status.removeAttribute("data-i18n");
      status.textContent = text;
    };
    btn.addEventListener("click", async () => {
      const ok = await window.confirmDialog.ask(t("settings.sendLogs.confirm",
        "Send your 3 newest packet logs to a2tools.app?\n\n" +
        "Packet logs are raw game traffic recorded while packet logging was on, " +
        "including character names. a2tools.app is run by the A2 Tools developer, " +
        "who decides who can see them and how long they are kept. Daevalog cannot " +
        "check or change that, and once they are sent it cannot see or help with them."),
        { ok: t("settings.sendLogs.send", "Send"), cancel: t("settings.sendLogs.cancel", "Cancel") });
      if (!ok) return;
      btn.disabled = true;
      show(t("settings.sendLogs.sending", "Sending..."));
      try {
        const result = await window.javaBridge?.sendLogsToDev?.();
        const code = result?.code || "?";
        show(t("settings.sendLogs.sent",
          `Sent to a2tools.app. Your report code is ${code}. Daevalog cannot help with ` +
          "logs sent there. For a problem with Daevalog, use Report a problem.",
          { code }));
      } catch (err) {
        const msg = String(err?.message || err || "");
        show(t("settings.sendLogs.failed", `Could not send: ${msg}`, { error: msg }));
      } finally {
        btn.disabled = false;
      }
    });
  }

  initPlayerLimitDropdown() {
    const wrapper = document.querySelector(".playerLimitDropdownWrapper");
    if (!wrapper) return;
    const btn = wrapper.querySelector(".playerLimitDropdownBtn");
    const menu = wrapper.querySelector(".playerLimitDropdownMenu");
    const textEl = btn?.querySelector(".settingsDropdownText");
    if (!btn || !menu || !textEl) return;

    const options = [4, 5, 6, 7, 8, 10, 12];
    textEl.textContent = String(this.playerLimit);

    for (const val of options) {
      const item = document.createElement("div");
      item.className = "settingsDropdownItem";
      item.textContent = String(val);
      item.dataset.value = String(val);
      if (val === this.playerLimit) item.classList.add("isActive");
      item.addEventListener("click", () => {
        this.playerLimit = val;
        this.safeSetSetting(this.storageKeys.playerLimit, String(val));
        textEl.textContent = String(val);
        menu.querySelectorAll(".settingsDropdownItem").forEach((el) =>
          el.classList.toggle("isActive", el.dataset.value === String(val))
        );
        menu.style.display = "none";
        this.renderCurrentRows();
      });
      menu.appendChild(item);
    }

    btn.addEventListener("click", () => {
      // The menu starts hidden by the stylesheet with no inline display, so
      // test for "open" rather than "closed" or the first click does nothing.
      menu.style.display = menu.style.display === "flex" ? "none" : "flex";
    });
    document.addEventListener("click", (e) => {
      if (!wrapper.contains(e.target)) menu.style.display = "none";
    });
  }

  renderCurrentRows() {
    if (this.isCollapse) return;
    let rowsToRender = Array.isArray(this.lastSnapshot) ? this.lastSnapshot : [];
    if (this.lastTargetMode === "trainTargets" && this.isLocalUserIdentified()) {
      rowsToRender = rowsToRender.filter((row) => row.name === this.USER_NAME);
    }
    const rowsSummary = this.getRowsSummary(rowsToRender);
    if (rowsSummary.listSignature !== this._lastRenderedListSignature) {
      const reasons = this.describeRowsChange(rowsSummary, this._lastRenderedRowsSummary);
      if (!Array.isArray(this.lastSnapshot) || this.lastSnapshot.length === 0) {
        reasons.push("no snapshot available");
      } else {
        reasons.push("renderCurrentRows refresh");
      }
      this.logDebug(
        `Meter list changed (${rowsToRender.length} rows). reason: ${
          reasons.join("; ") || "list membership changed"
        }.`
      );
      this._lastRenderedListSignature = rowsSummary.listSignature;
      this._lastRenderedRowsSummary = rowsSummary;
    }
    this.updateMeterTotalBar(rowsToRender);
    this.meterUI?.updateFromRows?.(rowsToRender);
  }

  getDefaultDetailsOpenOptions() {
    const numericLastTargetId = Number(this.lastTargetId);
    const hasConcreteTarget = Number.isFinite(numericLastTargetId) && numericLastTargetId > 0;
    const fallbackAllTargets =
      this.lastTargetMode === "lastHitByMe" &&
      !hasConcreteTarget &&
      !this.lastTargetName;
    const isTrainTargets = this.lastTargetMode === "trainTargets";
    const shouldDefaultAllTrainTargets = isTrainTargets && this.trainSelectionMode === "all";
    const shouldDefaultTrainTarget = isTrainTargets && this.trainSelectionMode === "highestDamage";
    const defaultTargetId = shouldDefaultTrainTarget
      ? (hasConcreteTarget ? numericLastTargetId : null)
      : hasConcreteTarget
        ? numericLastTargetId
        : null;
    return {
      defaultTargetAll: fallbackAllTargets || shouldDefaultAllTrainTargets || (!hasConcreteTarget && this.lastTargetMode === "allTargets"),
      defaultTargetId,
    };
  }

  refreshConnectionInfo({ skipSettingsRefresh = false } = {}) {
    if (!this.lockedIp || !this.lockedPort) return;
    const raw = window.javaBridge?.getConnectionInfo?.();
    if (typeof raw !== "string") {
      this.lockedIp.textContent = "-";
      this.lockedPort.textContent = "-";
      if (this.localActorIdInput && document.activeElement !== this.localActorIdInput) {
        this.localActorIdInput.value = "";
      }
      this.isDetectingPort = this.aionRunning;
      this.updateConnectionStatusUi();
      if (!skipSettingsRefresh) {
        this.refreshSettingsPanelIfOpen();
      }
      return;
    }
    const info = this.safeParseJSON(raw, {});

    // Show Npcap error if present
    const pcapErr = typeof info?.pcapError === "string" ? info.pcapError.trim() : "";
    if (pcapErr) {
      this.lockedIp.textContent = "";
      this.lockedPort.textContent = pcapErr;
      this.lockedPort.classList.add("isPcapError");
      this.updateConnectionStatusUi();
      if (!skipSettingsRefresh) this.refreshSettingsPanelIfOpen();
      return;
    }
    this.lockedPort.classList.remove("isPcapError");

    const deviceName = typeof info?.device === "string" && info.device.trim() ? info.device : "";
    const rawIp = info?.ip || "-";
    const ip =
      deviceName ||
      (rawIp === "127.0.0.1" || rawIp === "::1"
        ? this.i18n?.t("connection.loopback", "Local Loopback") ?? "Local Loopback"
        : rawIp);
    const hasPort = Number.isFinite(Number(info?.port));
    this.isDetectingPort = this.aionRunning && !hasPort;
    const port = hasPort
      ? String(info.port)
      : this.isDetectingPort
        ? this.i18n?.t("connection.detecting", "Detecting AION2 connection...")
        : this.i18n?.t("connection.auto", "Auto");
    this.lockedIp.textContent = ip;
    this.lockedPort.textContent = port;
    // Keep the device dropdown text in sync with the currently locked device
    if (this._autoDetectDevice && deviceName && this.deviceDropdownBtn) {
      const textEl = this.deviceDropdownBtn.querySelector(".settingsDropdownText");
      if (textEl) textEl.textContent = deviceName;
    }
    // Shown only: this status is polled and can be 3 s old, so the id the
    // meter uses comes from the dps updates (setLocalPlayerIdFromBackend).
    const polledLocalId = Number(info?.localPlayerId);
    const shownLocalId = this.localPlayerId ||
      (Number.isFinite(polledLocalId) && polledLocalId > 0 ? Math.trunc(polledLocalId) : null);
    if (this.localActorIdInput && document.activeElement !== this.localActorIdInput) {
      this.localActorIdInput.value = shownLocalId ? String(shownLocalId) : "";
    }
    // Never under the player's cursor: they may be typing a new name.
    // This window's own name first: it changes the moment the player saves
    // one, while the backend's copy here is a poll up to 3 s old and would
    // put the old name back. The game's name reaches USER_NAME through
    // syncCharacterNameFromGame, so nothing is lost.
    if (this.characterNameInput && document.activeElement !== this.characterNameInput) {
      const nickname = String(this.USER_NAME || info?.characterName || "").trim();
      this.characterNameInput.value = nickname;
    }
    // Do NOT reinitTargetSelection() when the backend-reported local id changes:
    // the local player churns entity ids mid-fight, and reinit → reset_combat
    // wipes all combat data and every teammate nickname. The "you" highlight
    // follows this.localPlayerId on the next dps render with no reset.
    this.updateConnectionStatusUi();
    if (!skipSettingsRefresh) {
      this.refreshSettingsPanelIfOpen();
    }
  }

  refreshSettingsPanelIfOpen() {
    if (!this.settingsPanel?.classList.contains("isOpen")) return;
    this.refreshConnectionInfo({ skipSettingsRefresh: true });
    this.updateSettingsVersion();
  }

  _updateDeviceDropdownState() {
    const disabled = this._autoDetectDevice;
    if (this.deviceDropdownBtn) this.deviceDropdownBtn.disabled = disabled;
    if (disabled && this.deviceDropdownMenu) this.deviceDropdownMenu.classList.remove("isOpen");
  }

  // The device list comes from the backend asynchronously, so fill the dropdown
  // once it has arrived rather than from whatever the cache held at the time.
  _loadDeviceDropdown() {
    const populate = () =>
      this._populateDeviceDropdown(this.safeGetSetting("dpsMeter.manualDevice") || null);
    const load = window.javaBridge?.loadAvailableDevices?.();
    if (!load) {
      populate();
      return;
    }
    load.then(populate, populate);
  }

  _populateDeviceDropdown(currentDevice) {
    if (!this.deviceDropdownBtn || !this.deviceDropdownMenu) return;
    const raw = window.javaBridge?.getAvailableDevices?.();
    const devices = typeof raw === "string" ? this.safeParseJSON(raw, []) : [];
    if (!Array.isArray(devices) || devices.length === 0) return;
    const options = devices.map((d) => ({ value: d, label: d }));
    // When auto-detect is on, show the currently locked device; otherwise show the manual selection
    const connRaw = window.javaBridge?.getConnectionInfo?.();
    const connInfo = typeof connRaw === "string" ? this.safeParseJSON(connRaw, {}) : {};
    const lockedDevice = typeof connInfo?.device === "string" && connInfo.device.trim() ? connInfo.device : "";
    // Unticking auto-detect leaves capture where it is until a device is picked,
    // so show that device rather than whichever happens to be listed first.
    const selected = this._autoDetectDevice
      ? (lockedDevice || currentDevice || devices[0])
      : (currentDevice || lockedDevice || devices[0]);
    this.deviceDropdownMenu.innerHTML = "";
    options.forEach((opt) => {
      const item = document.createElement("button");
      item.type = "button";
      item.className = "settingsDropdownItem";
      item.dataset.value = opt.value;
      item.textContent = opt.label;
      if (opt.value === selected) item.classList.add("isActive");
      item.addEventListener("click", () => {
        window.javaBridge?.setManualDevice?.(opt.value);
        this.deviceDropdownMenu.classList.remove("isOpen");
        const textEl = this.deviceDropdownBtn.querySelector(".settingsDropdownText");
        if (textEl) textEl.textContent = opt.label;
        this.deviceDropdownMenu.querySelectorAll(".settingsDropdownItem").forEach((el) =>
          el.classList.toggle("isActive", el.dataset.value === opt.value)
        );
        this.refreshConnectionInfo();
      });
      this.deviceDropdownMenu.appendChild(item);
    });
    const textEl = this.deviceDropdownBtn.querySelector(".settingsDropdownText");
    if (textEl) textEl.textContent = selected;
    this.deviceDropdownBtn.onclick = (event) => {
      if (this.deviceDropdownBtn.disabled) return;
      event.stopPropagation();
      // Close other dropdowns
      document.querySelectorAll(".settingsDropdownMenu.isOpen").forEach((menu) => {
        if (menu !== this.deviceDropdownMenu) menu.classList.remove("isOpen");
      });
      this.deviceDropdownMenu.classList.toggle("isOpen");
    };
  }

  updateSettingsVersion() {
    if (!this.settingsVersionValue) return;
    // The backend's label, "1.0 · r250" (or "1.0" when the build did not know its revision).
    const normalized = String(window.dpsData?.getVersion?.() || "").trim();
    this.settingsVersionValue.textContent = normalized ? `Daevalog ${normalized}` : "-";
    // The backend version fetch is async; retry briefly if it wasn't ready yet.
    if (!normalized && !this._versionRetryScheduled) {
      this._versionRetryScheduled = true;
      setTimeout(() => {
        this._versionRetryScheduled = false;
        this.updateSettingsVersion();
      }, 300);
    }
  }

  fitBossName() {
    const el = this.elBossName;
    if (!el) return;
    const container = el.parentElement;
    if (!container) return;
    // The stylesheet owns the boss-name size; this only ever shrinks a long
    // name to fit. It used to start from a hard-coded 18px (16 slim) and write
    // that as an inline style, which silently overrode whatever the CSS asked
    // for — so the header label ignored the design's type scale entirely.
    el.style.fontSize = "";
    const base = Number.parseFloat(getComputedStyle(el).fontSize);
    if (!Number.isFinite(base) || base <= 0) return;
    if (el.scrollWidth <= container.clientWidth) return;
    const minFs = Math.max(7, base * 0.72);
    for (let fs = base - 0.5; fs >= minFs; fs -= 0.5) {
      el.style.fontSize = `${fs}px`;
      if (el.scrollWidth <= container.clientWidth) return;
    }
  }

  updatePing(pushedMs) {
    if (!this.pingEl) return;
    if (!this.showPing) {
      this.pingEl.classList.remove("isVisible");
      return;
    }
    const ms = typeof pushedMs === "number" ? pushedMs : window.javaBridge?.getPingMs?.();
    if (typeof ms !== "number" || ms < 0) {
      this.pingEl.classList.remove("isVisible");
      return;
    }
    const textEl = this.pingEl.querySelector(".pingText");
    if (textEl) textEl.textContent = `${ms}ms`;
    else this.pingEl.textContent = `${ms}ms`;
    this.pingEl.classList.add("isVisible");
    this.pingEl.classList.remove("ping-good", "ping-warn", "ping-high", "ping-bad");
    this.pingEl.classList.add(
      ms < 100 ? "ping-good" : ms < 200 ? "ping-warn" : ms < 225 ? "ping-high" : "ping-bad"
    );
  }

  updateConnectionStatusUi() {
    if (!this.battleTimeRoot || !this.analysisStatusEl) return;
    if (!this.aionRunning) {
      this.applyConnectionStatusOverride(
        this.i18n?.t("battleTime.notRunning", "AION2 not running") ?? "AION2 not running"
      );
      return;
    }
    if (this.isDetectingPort) {
      this.applyConnectionStatusOverride(
        this.i18n?.t("connection.detecting", "Detecting AION2 connection...") ??
          "Detecting AION2 connection..."
      );
      return;
    }
    if (this._waitingForBoss && this.targetSelection === "bossTargets" && !this._captureSuspended) {
      this.applyConnectionStatusOverride(
        this.i18n?.t("battleTime.waitingBoss", "Waiting for a boss") ?? "Waiting for a boss"
      );
      return;
    }
    this.clearConnectionStatusOverride();
  }

  applyConnectionStatusOverride(text) {
    this._connectionStatusOverride = true;
    this.battleTimeRoot.classList.add("isVisible", "state-idle");
    this.analysisStatusEl.textContent = text;
    this.analysisStatusEl.style.display = "";
  }

  clearConnectionStatusOverride() {
    if (!this._connectionStatusOverride) return;
    this._connectionStatusOverride = false;
    this.battleTimeRoot.classList.remove("state-idle");
    if (!this._battleTimeVisible) {
      this.battleTimeRoot.classList.remove("isVisible");
    }
    if (this._captureSuspended) {
      this.analysisStatusEl.textContent =
        this.i18n?.t?.("battleTime.suspended", "App suspended") ?? "App suspended";
    } else {
      this.analysisStatusEl.textContent =
        this.i18n?.t("battleTime.analysing", "Ready - monitoring combat...") ??
        "Ready - monitoring combat...";
    }
    this.analysisStatusEl.style.removeProperty("display");
  }

  getRowsSummary(rows) {
    const safeRows = Array.isArray(rows) ? rows : [];
    const ids = safeRows.map((row) => String(row?.id ?? "")).sort();
    const names = safeRows.map((row) => String(row?.name ?? "")).sort();
    return {
      count: safeRows.length,
      ids,
      names,
      listSignature: ids.join("|"),
    };
  }

  describeRowsChange(nextSummary, previousSummary) {
    const reasons = [];
    if (!previousSummary) {
      reasons.push("initial meter render");
      return reasons;
    }
    if (previousSummary.count !== nextSummary.count) {
      reasons.push(`row count ${previousSummary.count} -> ${nextSummary.count}`);
    }
    const idsChanged =
      previousSummary.ids.length !== nextSummary.ids.length ||
      previousSummary.ids.some((id, index) => id !== nextSummary.ids[index]);
    if (idsChanged) {
      reasons.push("row ids changed");
    }
    const namesChanged =
      previousSummary.names.length !== nextSummary.names.length ||
      previousSummary.names.some((name, index) => name !== nextSummary.names[index]);
    if (namesChanged && !idsChanged) {
      reasons.push("row names changed");
    }
    return reasons;
  }

  logDebug(message) {
    if (!message) return;
    try {
      window.javaBridge?.logDebug?.(String(message));
    } catch (e) {
      globalThis.uiDebug?.log?.("logDebug blocked", { message: String(message), error: String(e) });
    }
  }

  isLocalUserIdentified() {
    const name = String(this.USER_NAME ?? "").trim();
    const localId = Number(this.localPlayerId);
    return Boolean(name) && Number.isFinite(localId) && localId > 0;
  }

  getDefaultTargetLabel(targetMode = "") {
    if (targetMode === "bossTargets") {
      return this.i18n?.t("target.boss", "Boss Targets") ?? "Boss Targets";
    }
    if (targetMode === "allTargets") {
      return this.i18n?.t("target.all", "All Targets") ?? "All Targets";
    }
    if (targetMode === "encounter") {
      return this.i18n?.t("target.encounter", "Encounter") ?? "Encounter";
    }
    if (targetMode === "trainTargets") {
      if (!this.isLocalUserIdentified()) {
        return this.i18n?.t("target.identifying", "Identifying you...") ?? "Identifying you...";
      }
      return this.i18n?.t("target.train", "Training Scarecrow") ?? "Training Scarecrow";
    }
    return this.i18n?.t("header.title", "Daevalog DPS Meter") ?? "Daevalog DPS Meter";
  }

  getTargetLabel({ targetId = 0, targetName = "", targetMode = "", dungeonId = 0 } = {}) {
    // In a party instance the title is the boss being fought, and the dungeon
    // between pulls. It used to stay on the dungeon throughout, so a whole run
    // read "Urugugu Canyon" past every boss. The modes that track no single
    // target (all targets, training) keep the dungeon.
    const dungeonLabel = Number(dungeonId) > 0
      ? (this.i18n?.getDungeonLabel?.(Number(dungeonId)) ?? "")
      : "";
    if (dungeonLabel) {
      const tracksOne = targetMode !== "allTargets" && targetMode !== "trainTargets" && targetMode !== "encounter";
      const numericId = Number(targetId);
      if (tracksOne && Number.isFinite(numericId) && numericId > 0) {
        const cleanName = typeof targetName === "string" ? targetName.trim() : "";
        const bossName = this.i18n?.getNpcName?.(numericId, cleanName) ?? cleanName;
        if (bossName) return bossName;
      }
      return dungeonLabel;
    }
    if (targetMode === "trainTargets" && !this.isLocalUserIdentified()) {
      return this.i18n?.t("target.identifying", "Identifying you...") ?? "Identifying you...";
    }
    if (targetMode === "allTargets" || targetMode === "trainTargets" || targetMode === "encounter") {
      return this.getDefaultTargetLabel(targetMode);
    }
    if (targetMode === "bossTargets" && (!Number(targetId) || Number(targetId) <= 0) && !targetName) {
      return this.getDefaultTargetLabel(targetMode);
    }
    if (targetMode === "lastHitByMe" && (!Number(targetId) || Number(targetId) <= 0) && !targetName) {
      // Only prompt to identify when we genuinely don't know who the player is.
      // Once identified, an empty target (idle / just after a reset or zone change)
      // should fall back to the neutral mode label, not keep showing "Identifying...".
      if (!this.isLocalUserIdentified()) {
        return this.i18n?.t("target.identifying", "Identifying you...") ?? "Identifying you...";
      }
      return this.getDefaultTargetLabel(targetMode);
    }
    const numericTargetId = Number(targetId);
    const cleanTargetName = typeof targetName === "string" ? targetName.trim() : "";
    if (Number.isFinite(numericTargetId) && numericTargetId > 0) {
      const localizedName = this.i18n?.getNpcName?.(numericTargetId, cleanTargetName) ?? cleanTargetName;
      return localizedName || `Mob #${numericTargetId}`;
    }
    if (cleanTargetName) {
      return cleanTargetName;
    }
    return this.getDefaultTargetLabel(targetMode);
  }

  updateTargetModeButton() {
    if (!this.targetModeBtn) return;
    const isBossTargets = this.targetSelection === "bossTargets";
    const isAllTargets = this.targetSelection === "allTargets";
    const isTrainTargets = this.targetSelection === "trainTargets";
    const isEncounter = this.targetSelection === "encounter";
    this.targetModeBtn.classList.toggle("isAllTargets", isAllTargets || isEncounter);
    this.targetModeBtn.classList.toggle("isTrainTargets", isTrainTargets);
    this.targetModeBtn.textContent = isBossTargets ? "BOSS" : isAllTargets ? "ALL" : isTrainTargets ? "TRAIN" : isEncounter ? "ENC" : "TARGET";
    const ariaLabel = isBossTargets
      ? "Boss targets mode"
      : isAllTargets
        ? "All targets mode"
        : isTrainTargets
          ? "Train targets mode"
          : isEncounter
            ? "Encounter mode"
            : "Target mode";
    this.targetModeBtn.setAttribute("aria-label", ariaLabel);
  }

  refreshBossLabel() {
    if (!this.elBossName) return;
    if (this.lastTargetName || this.lastTargetId) {
      return;
    }
    this.elBossName.textContent = this.getTargetLabel({
      targetMode: this.lastTargetMode,
      targetId: this.lastTargetId,
      targetName: this.lastTargetName,
      dungeonId: this.lastDungeonId,
    });
    this.elBossName.classList.toggle("isAllTargets", this.lastTargetMode === "allTargets");
  }

}

DpsApp.instance = null;
