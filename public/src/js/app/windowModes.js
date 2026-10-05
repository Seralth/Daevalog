// App: the monitor choice and the Settings, History and Details window modes.
"use strict";

Object.assign(DpsApp.prototype, {
  // The "details" window runs this same bundle. Rather than a second app, it
  // reuses the panel that already exists in index.html: the overlay chrome is
  // hidden by CSS and the panel is held open on the whole fight.
  /** "off" or a monitor index as a string. */
  getDetailsMonitorSetting() {
    const raw = this.safeGetSetting(this.storageKeys.detailsMonitor);
    if (raw === null || raw === undefined || String(raw).trim() === "") return "off";
    return String(raw);
  },

  async refreshMonitorList() {
    try {
      const list = await window.javaBridge?.listMonitors?.();
      this.monitorList = Array.isArray(list) ? list : [];
    } catch {
      this.monitorList = [];
    }
    return this.monitorList;
  },

  monitorLabel(monitor, position) {
    const n = Number.isFinite(position) ? position + 1 : Number(monitor?.index) + 1;
    const w = Math.round(Number(monitor?.width) || 0);
    const h = Math.round(Number(monitor?.height) || 0);
    const notes = [];
    if (monitor?.isPrimary) {
      notes.push(this.i18n?.t?.("settings.detailsMonitor.primary", "primary") ?? "primary");
    }
    // Where the screen physically sits — a resolution alone does not tell the
    // user which of their monitors they just selected.
    const side = String(monitor?.side || "");
    if (side) {
      notes.push(this.i18n?.t?.(`settings.detailsMonitor.side.${side}`, side) ?? side);
    }
    const suffix = notes.length ? ` (${notes.join(", ")})` : "";
    return `${n} — ${w}×${h}${suffix}`;
  },

  /**
   * Apply the "Show Details on monitor" setting. Opening is best-effort: if the
   * chosen display has been unplugged since it was saved, fall back to off
   * rather than leaving a window stranded off-screen.
   */
  async applyDetailsMonitor(value, { persist = false } = {}) {
    const next = value === "off" || value === null || value === undefined ? "off" : String(value);
    this.detailsMonitor = next;
    if (persist) {
      this.safeSetSetting(this.storageKeys.detailsMonitor, next);
    }
    if (next === "off") {
      await window.javaBridge?.closeDetailsWindow?.();
      this.setDetailsMonitorHint("");
      return;
    }
    const index = Number(next);
    const monitors = await this.refreshMonitorList();
    if (!Number.isFinite(index) || !monitors.some((m) => Number(m.index) === index)) {
      this.setDetailsMonitorHint(
        this.i18n?.t?.("settings.detailsMonitor.missing", "That display is not connected.") ??
          "That display is not connected."
      );
      await window.javaBridge?.closeDetailsWindow?.();
      this.detailsMonitor = "off";
      if (persist) this.safeSetSetting(this.storageKeys.detailsMonitor, "off");
      return;
    }
    try {
      await window.javaBridge?.openDetailsWindow?.(index);
      this.setDetailsMonitorHint("");
    } catch (err) {
      this.setDetailsMonitorHint(String(err?.message || err || ""));
    }
  },

  setDetailsMonitorHint(text) {
    if (!this.detailsMonitorHint) return;
    this.detailsMonitorHint.textContent = text || "";
    this.detailsMonitorHint.style.display = text ? "" : "none";
  },

  /**
   * Hand a Details view to the standalone Details window.
   *
   * Every route into Details from the overlay — a meter row, a fight picked in
   * History — goes through here, so there is exactly one Details surface and
   * the overlay never grows to contain the panel. The backend creates the
   * window if it is not up yet and places it where the user last left it.
   *
   * `fallback` is the old in-overlay behaviour, run when the window cannot be
   * reached at all (bridge without the command, window build failed). Showing
   * the panel in the overlay beats showing nothing.
   */
  openDetailsSurface(request, fallback) {
    const runFallback = () => {
      try { fallback?.(); } catch (err) { console.error("[Daevalog] details fallback failed", err); }
    };
    // A Details window is already the destination, so it renders in place
    // rather than asking for yet another window. The overlay and the History
    // window are both senders: History in particular must route, or picking a
    // fight would replace the list instead of opening beside it.
    if (window.A2_VIEW === "details") {
      runFallback();
      return;
    }
    const send = window.javaBridge?.requestDetailsView;
    if (typeof send !== "function") {
      runFallback();
      return;
    }
    let result;
    try {
      result = send.call(window.javaBridge, request);
    } catch (err) {
      console.error("[Daevalog] requestDetailsView failed", err);
      runFallback();
      return;
    }
    Promise.resolve(result).catch((err) => {
      console.error("[Daevalog] requestDetailsView failed", err);
      runFallback();
    });
  },

  // The "settings" window runs this same bundle with only the settings panel
  // visible, so its markup and wiring are reused rather than duplicated.
  enterSettingsWindowMode() {
    document.body.classList.add("isSettingsWindow");
    this.refreshMonitorList().then(() => this.initializeSettingsDropdowns());
    this._loadDeviceDropdown();
    this.settingsPanel?.classList.add("isOpen");
    // Closing is wired in startApp, before anything that can be slow.
    requestAnimationFrame(() => {
      requestAnimationFrame(() => window.javaBridge?.toolWindowReady?.("settings"));
    });
  },

  // The Battle History window: this bundle again, showing only the history
  // panel. It is a browser the user leaves open — picking a fight opens that
  // fight in a window of its own and the list stays exactly as it was, so
  // several fights can be read side by side.
  enterHistoryWindowMode() {
    document.body.classList.add("isHistoryWindow");
    const openList = () => this.historyUI?.open?.();
    openList();
    // open() reads a cache that an async prefetch fills, and this window is
    // created precisely in order to show the list — so it renders before its
    // own prefetch lands and would otherwise sit empty forever. Re-render once
    // the data is genuinely there. Safe to re-open blindly: this resolves in
    // the first moments of the window, before anyone can scroll or filter.
    Promise.resolve(window.javaBridge?.refreshFightHistory?.())
      .then(() => openList())
      .catch(() => {});

    // Frameless, so the panel's own × closes the window rather than just
    // hiding the list — there is nothing behind it here.
    document.querySelector(".historyClose")?.addEventListener("click", (event) => {
      event.stopPropagation();
      window.javaBridge?.closeToolWindow?.();
    });
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape") window.javaBridge?.closeToolWindow?.();
    });

    // Nothing sits behind the list, so if something closes it, put it back.
    if (this._historyWindowTimer) clearInterval(this._historyWindowTimer);
    this._historyWindowTimer = setInterval(() => {
      if (!this.historyUI?.isOpen?.()) openList();
    }, 1000);

    requestAnimationFrame(() => {
      requestAnimationFrame(() => window.javaBridge?.toolWindowReady?.("history"));
    });
  },

  enterDetailsWindowMode() {
    document.body.classList.add("isDetailsWindow");
    // Two kinds of window run this mode: the single live "details" window, which
    // is the one the monitor setting governs, and a "details-<fightId>" window
    // showing one saved fight, of which there can be several.
    const label = window.__TAURI__?.window?.getCurrentWindow?.()?.label || "details";
    const isFightWindow = label.startsWith("details-");
    if (isFightWindow) document.body.classList.add("isFightWindow");

    // Frameless window, so the panel header supplies the close control.
    document.querySelector(".detailsWindowClose")?.addEventListener("click", () => {
      if (isFightWindow) {
        // One of several, and nothing to do with the monitor setting.
        window.javaBridge?.closeToolWindow?.();
        return;
      }
      // Closing the live window also turns the setting off, otherwise it would
      // reopen on next launch.
      this.safeSetSetting(this.storageKeys.detailsMonitor, "off");
      window.javaBridge?.closeDetailsWindow?.();
    });
    const openAll = () => {
      this.detailsUI?.open?.(null, { defaultTargetAll: true, pin: true, force: true });
    };
    // Confirm on the screen itself which monitor this is. Backed by the
    // placement the backend actually performed, not by what was requested.
    window.__TAURI__?.event?.listen?.("details-placed", (event) => {
      const badge = document.querySelector(".detailsMonitorBadge");
      if (!badge) return;
      const p = event?.payload || {};
      const numEl = badge.querySelector(".detailsMonitorBadgeNum");
      const textEl = badge.querySelector(".detailsMonitorBadgeText");
      if (numEl) numEl.textContent = String(p.number ?? "");
      if (textEl) {
        const label = this.i18n?.t?.("settings.detailsMonitor.badge", "Monitor") ?? "Monitor";
        const primary = p.isPrimary
          ? ` · ${this.i18n?.t?.("settings.detailsMonitor.primary", "primary") ?? "primary"}`
          : "";
        textEl.textContent = `${label} ${p.number ?? ""} · ${p.width}×${p.height}${primary}`;
      }
      badge.classList.remove("isVisible");
      void badge.offsetWidth; // restart the fade if it fires twice
      badge.classList.add("isVisible");
    });

    // Views routed here from the overlay (a meter row, a fight from History).
    // The request arrives two ways and only one of them can be relied on: a
    // window that was created to serve the request has no listener attached
    // when it is emitted, so it pulls the parked request on startup instead.
    // The seq stamp makes applying it twice a no-op.
    this._lastDetailsRequestSeq = 0;
    this._detailsRequestsInFlight = 0;
    const applyRequest = async (payload) => {
      if (!payload || typeof payload !== "object") return;
      const seq = Number(payload.seq) || 0;
      if (seq && seq <= this._lastDetailsRequestSeq) return;
      this._lastDetailsRequestSeq = seq;
      // Hold off the keep-open watchdog: a fight has to be loaded from disk
      // before the panel opens, and the panel is closed until it lands.
      this._detailsRequestsInFlight += 1;
      try {
        if (payload.kind === "fight") {
          const raw = await window.javaBridge?.getFightDetails?.(String(payload.fightId || ""));
          const record = typeof raw === "string" ? this.safeParseJSON(raw, null) : raw;
          if (!record) {
            openAll();
            return;
          }
          this.detailsUI?.resetDetailsMode?.();
          await this.detailsUI?.openHistoryFight?.(record);
          return;
        }
        const row = payload.row && typeof payload.row === "object" ? payload.row : null;
        await this.detailsUI?.open?.(row, {
          force: true,
          pin: true,
          defaultTargetAll: !!payload.defaultTargetAll,
          defaultTargetId: payload.defaultTargetId ?? null,
        });
      } catch (err) {
        console.error("[Daevalog] details request failed", err);
      } finally {
        this._detailsRequestsInFlight = Math.max(0, this._detailsRequestsInFlight - 1);
      }
    };
    const listening = window.__TAURI__?.event?.listen?.(
      "details-request",
      (event) => { void applyRequest(event?.payload); }
    );

    // A fight window has a saved fight waiting for it, so showing live combat
    // first would only flash the wrong content. The fight branch of
    // applyRequest falls back to openAll() if the record cannot be loaded, so
    // this cannot strand the window empty.
    if (!isFightWindow) openAll();
    // Tell the backend to reveal the window now that the panel has painted.
    // Sequenced after the listener so a request emitted meanwhile is not lost.
    Promise.resolve(listening)
      .catch(() => {})
      .then(() => window.javaBridge?.takePendingDetailsRequest?.())
      .then((pending) => (pending ? applyRequest(pending) : undefined))
      .catch(() => {})
      .then(() => {
        requestAnimationFrame(() => {
          requestAnimationFrame(() => window.javaBridge?.detailsWindowReady?.());
        });
      });
    // The panel can be closed from inside (back button, escape). In this window
    // there is nothing behind it, so re-assert rather than leave a blank screen.
    // History counts as content: it is the other panel that legitimately fills
    // this window, and reopening Details over it would fight the back button.
    // Only the live window: openAll() shows current combat, which in a window
    // opened to display one saved fight would quietly replace that fight.
    if (!isFightWindow) {
      if (this._detailsWindowTimer) clearInterval(this._detailsWindowTimer);
      this._detailsWindowTimer = setInterval(() => {
        if (this._detailsRequestsInFlight > 0) return;
        if (!this.detailsPanel?.classList?.contains("open")) openAll();
      }, 1000);
    }
  },
});
