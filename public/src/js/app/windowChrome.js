// App: window drag, resize, overlay lock and capture suspend.
"use strict";

Object.assign(DpsApp.prototype, {
  bindDragToMoveWindow() {
    let isDragging = false;
    let startX = 0;
    let startY = 0;
    let initialStageX = 0;
    let initialStageY = 0;
    let pendingStageX = 0;
    let pendingStageY = 0;
    let lastMovedX = Number.NaN;
    let lastMovedY = Number.NaN;
    let dragRafId = null;
    let hasDragMoved = false;

    const flushMove = (force = false) => {
      dragRafId = null;
      if ((!isDragging && !force) || !window.javaBridge) return;
      const deltaSinceLastX = Math.abs(pendingStageX - lastMovedX);
      const deltaSinceLastY = Math.abs(pendingStageY - lastMovedY);
      if (!force && Number.isFinite(deltaSinceLastX) && Number.isFinite(deltaSinceLastY)) {
        if (deltaSinceLastX < 2 && deltaSinceLastY < 2) return;
      }
      if (pendingStageX === lastMovedX && pendingStageY === lastMovedY) return;
      window.javaBridge.moveWindow(pendingStageX, pendingStageY);
      lastMovedX = pendingStageX;
      lastMovedY = pendingStageY;
    };

    document.addEventListener("mousedown", (e) => {
      if (e.button !== 0) return;
      const targetEl = e.target?.nodeType === Node.TEXT_NODE ? e.target.parentElement : e.target;
      if (targetEl?.closest?.(".resizeHandle")) {
        return;
      }
      if (targetEl?.closest?.(".headerBtn, .footerBtn, .bossIcon")) {
        return;
      }
      if (targetEl?.closest?.(".settingsPanel, .historyPanel, .detailsBody, .detailsSettingsMenu")) {
        return;
      }
      if (targetEl?.closest?.("button, input, select, textarea, a, [data-no-drag]")) {
        return;
      }
      isDragging = true;
      hasDragMoved = false;
      this.isWindowDragging = true;
      this.deferFetchUntilDragEnd = true;
      // Don't freeze pointer events yet — defer until the 3px drag threshold
      // is crossed so that simple clicks on meter bars still fire normally.
      startX = e.screenX;
      startY = e.screenY;
      initialStageX = window.screenX;
      initialStageY = window.screenY;
      pendingStageX = initialStageX;
      pendingStageY = initialStageY;
      lastMovedX = Number.NaN;
      lastMovedY = Number.NaN;
    });

    document.addEventListener("mousemove", (e) => {
      if (!isDragging || !window.javaBridge) return;

      const deltaX = e.screenX - startX;
      const deltaY = e.screenY - startY;
      if (!hasDragMoved && (Math.abs(deltaX) > 3 || Math.abs(deltaY) > 3)) {
        hasDragMoved = true;
        this.setWindowDragFreeze(true);
        this.elList?.classList?.add?.("dragInteracting");
        // Hide heavy panels during drag to reduce per-frame repaint cost;
        // place a lightweight ghost outline so the user sees where they are.
        for (const sel of [".settingsPanel", ".detailsPanel", ".historyPanel"]) {
          const el = document.querySelector(sel);
          if (el && el.offsetParent !== null) {
            const r = el.getBoundingClientRect();
            const ghost = document.createElement("div");
            ghost.style.cssText = `position:fixed;left:${r.left}px;top:${r.top}px;width:${r.width}px;height:${r.height}px;`
              + "background:rgba(0,0,0,0.35);border:1px solid rgba(255,255,255,0.12);border-radius:8px;pointer-events:none;z-index:9999;";
            document.body.appendChild(ghost);
            el.style.visibility = "hidden";
            el._dragHidden = true;
            el._dragGhost = ghost;
          }
        }
      }
      pendingStageX = initialStageX + deltaX;
      pendingStageY = initialStageY + deltaY;

      if (dragRafId !== null) return;
      dragRafId = requestAnimationFrame(flushMove);
    });

    document.addEventListener("mouseup", () => {
      if (!isDragging) return;
      isDragging = false;
      this.isWindowDragging = false;
      this.setWindowDragFreeze(false);
      if (hasDragMoved) {
        this.elList?.classList?.remove?.("dragInteracting");
        this.suppressRowInteractionUntilMs = this.nowMs() + 120;
        // Restore panels hidden during drag
        for (const sel of [".settingsPanel", ".detailsPanel", ".historyPanel"]) {
          const el = document.querySelector(sel);
          if (el?._dragHidden) {
              el.style.visibility = "";
              el._dragGhost?.remove();
              delete el._dragHidden;
              delete el._dragGhost;
            }
        }
      }
      if (dragRafId !== null) {
        cancelAnimationFrame(dragRafId);
        dragRafId = null;
      }
      flushMove(true);
      if (this.deferFetchUntilDragEnd) {
        this.deferFetchUntilDragEnd = false;
        this.fetchDps();
      }
    });
  },

  bindResizeHandle() {
    this.resizeHandle = document.querySelector(".resizeHandle");
    this.meterEl = document.querySelector(".meter");
    if (!this.resizeHandle || !this.meterEl) return;

    let isResizing = false;
    let startX = 0;
    let startY = 0;
    let startWidth = 0;
    let startHeight = 0;
    const minWidth = 300;
    const minHeight = 30;

    // Screen coordinates, not client ones: a window manager that keeps windows
    // on screen (KWin) moves the window while it grows during the drag, which
    // shifts clientX/Y and made the size jump past the cursor (issue #11).
    const onMouseMove = (event) => {
      if (!isResizing) return;
      // The button came up outside the window, where no mouseup arrives; the
      // next move inside shows it. A move outside is not trusted: a fast drag
      // leaves the window before it has grown (KWin, XWayland) and those moves
      // can report no button held, which cut the resize short.
      const inside = event.clientX >= 0 && event.clientY >= 0
        && event.clientX < window.innerWidth && event.clientY < window.innerHeight;
      if ((event.buttons & 1) === 0 && inside) {
        onMouseUp();
        return;
      }
      const nextWidth = Math.max(minWidth, startWidth + (event.screenX - startX));
      const nextHeight = Math.max(minHeight, startHeight + (event.screenY - startY));
      this.meterEl.style.width = `${nextWidth}px`;
      this.meterEl.style.height = `${nextHeight}px`;
    };

    const onMouseUp = () => {
      if (!isResizing) return;
      isResizing = false;
      // Convert fixed height to min-height so the meter can still grow
      // when new rows are added, while preserving the user's minimum.
      const currentHeight = this.meterEl.style.height;
      if (currentHeight) {
        this.meterEl.style.minHeight = currentHeight;
        this.meterEl.style.height = "";
      }
    };

    this.resizeHandle.addEventListener("mousedown", (event) => {
      event.preventDefault();
      event.stopPropagation();
      const rect = this.meterEl.getBoundingClientRect();
      startWidth = rect.width;
      startHeight = rect.height;
      startX = event.screenX;
      startY = event.screenY;
      isResizing = true;
    });

    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp);
  },

  // ===== Click-through lock =====
  // Locked, the overlay lets clicks through to the game and cannot be
  // dragged; only its lock button stays clickable (the backend watches the
  // pointer, see OverlayLock in app/overlay_lock.rs), and a hotkey toggles it too. Offered
  // only where the backend can do that: Windows, not Wayland.

  initOverlayLock() {
    this._overlayLocked = false;
    this.showLockBtnCheckbox = document.querySelector(".showLockBtnCheckbox");
    const isOverlay = window.A2_VIEW === "main";
    Promise.resolve(window.javaBridge?.overlayLockSupported?.())
      .then((supported) => {
        if (!supported) return;
        document.querySelectorAll(".lockSetting").forEach((el) => { el.style.display = ""; });
        const show = this.safeGetSetting(this.storageKeys.showLockBtn) === "true";
        this._showLockSetting = show;
        if (this.showLockBtnCheckbox) {
          this.showLockBtnCheckbox.checked = show;
          this.showLockBtnCheckbox.addEventListener("change", (event) => {
            const isChecked = !!event.target?.checked;
            this.safeSetSetting(this.storageKeys.showLockBtn, String(isChecked));
            if (!isOverlay) return;
            this._showLockSetting = isChecked;
            // A locked overlay keeps its button whatever the setting says.
            this._applyLockBtnVisibility(isChecked || this._overlayLocked);
          });
        }
        if (!isOverlay) return;
        this._applyLockBtnVisibility(show);
        window.addEventListener("resize", () => this._sendLockBtnRect());
        // Header buttons shown or hidden move the lock button without a
        // window resize; a locked overlay must follow it.
        if (window.ResizeObserver && this.lockBtn?.parentElement) {
          new ResizeObserver(() => this._sendLockBtnRect()).observe(this.lockBtn.parentElement);
        }
        // Stay locked across restarts, as the player left it.
        if (this.safeGetSetting(this.storageKeys.overlayLocked) === "true") {
          requestAnimationFrame(() => this._setOverlayLocked(true));
        }
      })
      .catch(() => {});
  },

  _applyLockBtnVisibility(show) {
    if (this.lockBtn) this.lockBtn.style.display = show ? "" : "none";
    this.headerBtns?.classList.toggle("hasLockBtn", !!show);
    if (show) requestAnimationFrame(() => this._sendLockBtnRect());
  },

  // Where the button is, so the backend keeps it clickable while locked.
  _sendLockBtnRect() {
    if (!this.lockBtn || this.lockBtn.style.display === "none") return;
    const r = this.lockBtn.getBoundingClientRect();
    if (!r.width || !r.height) return;
    window.javaBridge?.setLockButtonRect?.(r.left, r.top, r.width, r.height, window.devicePixelRatio || 1);
  },

  _setOverlayLocked(locked) {
    this._sendLockBtnRect();
    window.javaBridge?.setOverlayLocked?.(!!locked);
    this._onOverlayLockChanged(!!locked);
  },

  // Also called when the hotkey toggled the lock (the backend has done it).
  _onOverlayLockChanged(locked) {
    this._overlayLocked = !!locked;
    // A locked overlay takes no clicks, so an open mode menu could not close.
    if (this._overlayLocked) this.closeTargetModeMenu?.();
    this.safeSetSetting(this.storageKeys.overlayLocked, String(this._overlayLocked));
    document.body.classList.toggle("overlayLocked", this._overlayLocked);
    // Locked, the button is always shown: on Linux a tray or a hotkey may not
    // exist, and the button is then the only way back.
    if (window.A2_VIEW === "main") {
      this._applyLockBtnVisibility(!!this._showLockSetting || this._overlayLocked);
    }
    if (!this.lockBtn) return;
    this.lockBtn.classList.toggle("isLocked", this._overlayLocked);
    const icon = document.createElement("i");
    icon.setAttribute("data-lucide", this._overlayLocked ? "lock" : "lock-open");
    this.lockBtn.replaceChildren(icon);
    window.lucide?.createIcons?.({ root: this.lockBtn });
  },

  _applySuspendBtnVisibility(show) {
    if (this.suspendBtn) {
      this.suspendBtn.style.display = show ? "" : "none";
    }
    if (this.headerBtns) {
      this.headerBtns.classList.toggle("hasSuspendBtn", !!show);
    }
  },

  _setCaptureSuspended(suspended) {
    this._captureSuspended = !!suspended;
    window.javaBridge?.suspendCapture?.(this._captureSuspended);
    this._updateSuspendBtnIcon();
    this._updateSuspendStatusMessage();
  },

  _updateSuspendBtnIcon() {
    if (!this.suspendBtn) return;
    const iconEl = this.suspendBtn.querySelector("i, svg");
    if (!iconEl) return;
    const iconName = this._captureSuspended ? "power-off" : "power";
    this.suspendBtn.classList.toggle("isSuspended", !!this._captureSuspended);
    if (iconEl.tagName === "I") {
      iconEl.setAttribute("data-lucide", iconName);
    } else {
      // SVG already rendered — replace with a new <i> tag and re-render
      const newIcon = document.createElement("i");
      newIcon.setAttribute("data-lucide", iconName);
      this.suspendBtn.replaceChildren(newIcon);
    }
    window.lucide?.createIcons?.({ root: this.suspendBtn });
  },

  _updateSuspendStatusMessage() {
    const el = this.analysisStatusEl || document.querySelector(".battleTime .analysisStatus");
    if (!el) return;
    if (this._captureSuspended) {
      el.textContent = this.i18n?.t?.("battleTime.suspended", "App suspended") ?? "App suspended";
      el.style.display = "";
      // Ensure the battle time bar is visible so the message shows
      if (this.battleTimeRoot) this.battleTimeRoot.classList.add("isVisible");
    } else {
      el.textContent = this.i18n?.t?.("battleTime.analysing", "Monitoring data...") ?? "Monitoring data...";
    }
  },
});
