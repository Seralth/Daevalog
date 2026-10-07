// App: the mode menu. The footer's mode button opens a list of the modes.
"use strict";

Object.assign(DpsApp.prototype, {
  bindTargetModeMenu() {
    const button = this.targetModeBtn;
    const menu = document.querySelector(".targetModeMenu");
    if (!button || !menu) return;
    this.targetModeMenu = menu;
    menu.replaceChildren(...TARGET_MODE_ORDER.map((mode) => {
      const item = document.createElement("button");
      item.type = "button";
      item.className = "targetModeItem";
      item.tabIndex = -1;
      item.setAttribute("role", "menuitemradio");
      item.dataset.mode = mode;
      item.textContent = TARGET_MODE_LABELS[mode];
      // detail 0: Enter or Space, so the focus goes back to the button.
      item.addEventListener("click", (event) => this.pickTargetMode(mode, { focusButton: event?.detail === 0 }));
      return item;
    }));
    button.addEventListener("click", (event) => {
      if (this.isTargetModeMenuOpen() || this._targetModeMenuOpening) this.closeTargetModeMenu();
      else this.openTargetModeMenu({ focusItem: event?.detail === 0 });
    });
    button.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        this.closeTargetModeMenu();
        return;
      }
      if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
      event.preventDefault();
      if (this.isTargetModeMenuOpen()) this.focusCurrentTargetMode();
      else this.openTargetModeMenu({ focusItem: true });
    });
    menu.addEventListener("keydown", (event) => this.onTargetModeMenuKey(event));
    // A press anywhere else closes it. pointerdown, because the window drag
    // stops mousedown before other document listeners see it.
    document.addEventListener("pointerdown", (event) => {
      if (!this.isTargetModeMenuOpen()) return;
      const target = event.target?.nodeType === 3 ? event.target.parentElement : event.target;
      if (target?.closest?.(".targetModeMenu, .targetModeBtn")) return;
      this.closeTargetModeMenu();
    }, true);
    window.addEventListener("blur", () => this.closeTargetModeMenu());
    // Leaving the window closes it too. A layer overlay hears nothing of a
    // click on another window, has no focus to lose and, under KWin, gets no
    // leave event, so the menu also closes after 5 s without pointer or key
    // activity in the window.
    document.documentElement?.addEventListener("mouseleave", () => this.closeTargetModeMenuIn(400));
    for (const type of ["pointermove", "keydown"]) {
      document.addEventListener(type, () => this.closeTargetModeMenuIn(5000), true);
    }
    this.updateTargetModeMenu();
  },

  closeTargetModeMenuIn(ms) {
    if (!this.isTargetModeMenuOpen()) return;
    clearTimeout(this._targetModeMenuTimer);
    this._targetModeMenuTimer = setTimeout(() => this.closeTargetModeMenu(), ms);
  },

  isTargetModeMenuOpen() {
    return !!this.targetModeMenu?.classList.contains("isOpen");
  },

  updateTargetModeMenu() {
    this.targetModeMenu?.querySelectorAll(".targetModeItem").forEach((item) => {
      item.setAttribute("aria-checked", String(item.dataset.mode === this.targetSelection));
    });
  },

  // Returns a promise that settles once the menu is open.
  openTargetModeMenu({ focusItem = false } = {}) {
    const menu = this.targetModeMenu;
    if (!menu || this._overlayLocked || this.isTargetModeMenuOpen()) return Promise.resolve();
    const opening = {};
    this._targetModeMenuOpening = opening;
    // A Wayland layer overlay cannot see its place on screen; the backend can.
    return Promise.resolve(window.javaBridge?.getOverlayPlace?.())
      .catch(() => null)
      .then((place) => {
        if (this._targetModeMenuOpening !== opening) return;
        this._targetModeMenuOpening = null;
        this.hideHoverTooltip?.();
        this.updateTargetModeMenu();
        menu.classList.add("isOpen");
        this.targetModeBtn.setAttribute("aria-expanded", "true");
        this.placeTargetModeMenu(place);
        this.closeTargetModeMenuIn(5000);
        if (focusItem) this.focusCurrentTargetMode();
      });
  },

  focusCurrentTargetMode() {
    const menu = this.targetModeMenu;
    (menu?.querySelector('.targetModeItem[aria-checked="true"]') ?? menu?.querySelector(".targetModeItem"))?.focus();
  },

  closeTargetModeMenu({ focusButton = false } = {}) {
    this._targetModeMenuOpening = null;
    clearTimeout(this._targetModeMenuTimer);
    if (!this.isTargetModeMenuOpen()) return;
    this.targetModeMenu.classList.remove("isOpen", "isRow");
    this.targetModeBtn?.setAttribute("aria-expanded", "false");
    if (focusButton) this.targetModeBtn?.focus();
    window.javaBridge?.updateOverlaySize?.();
  },

  pickTargetMode(mode, { focusButton = false } = {}) {
    this.closeTargetModeMenu({ focusButton });
    if (mode === this.targetSelection) return;
    this.setTargetSelection(mode, { persist: true, syncBackend: true, reason: "mode menu" });
    if (!this.isCollapse) this.fetchDps();
  },

  // The menu stays inside the window: a layer overlay shows nothing outside
  // its own surface. A column above the button when the meter has room, else
  // below it, where the window grows to hold it as for the hover tooltip.
  // With room for neither (a short meter at the bottom of the screen), the
  // modes sit in one row above the button. `place` is the layer overlay's
  // place on its monitor, or null for a normal window.
  placeTargetModeMenu(place = null) {
    const menu = this.targetModeMenu;
    const parent = menu?.offsetParent;
    if (!parent) return;
    const origin = parent.getBoundingClientRect();
    const originLeft = origin.left + (parent.clientLeft || 0);
    const originTop = origin.top + (parent.clientTop || 0);
    const button = this.targetModeBtn.getBoundingClientRect();
    const gap = 4;
    const margin = 8;
    menu.classList.remove("isRow");
    const screen = window.screen || {};
    const windowTop = Array.isArray(place) ? Number(place[1]) || 0 : (window.screenY || 0) - (screen.availTop || 0);
    const above = button.top - gap - margin;
    const below = (screen.availHeight || window.innerHeight) - windowTop - button.bottom - gap - margin;
    const columnHeight = menu.offsetHeight;
    const row = columnHeight > above && columnHeight > below;
    menu.classList.toggle("isRow", row);
    const width = menu.offsetWidth;
    const height = menu.offsetHeight;
    const top = row || columnHeight <= above
      ? Math.max(0, button.top - gap - height)
      : button.bottom + gap;
    menu.style.left = `${Math.max(margin, button.right - width) - originLeft}px`;
    menu.style.top = `${top - originTop}px`;
    window.javaBridge?.updateOverlaySize?.();
  },

  onTargetModeMenuKey(event) {
    const items = [...this.targetModeMenu.querySelectorAll(".targetModeItem")];
    const at = items.indexOf(document.activeElement);
    const focus = (i) => items[(i + items.length) % items.length]?.focus();
    switch (event.key) {
      case "ArrowDown":
      case "ArrowRight": focus(at + 1); break;
      case "ArrowUp":
      case "ArrowLeft": focus(at < 0 ? -1 : at - 1); break;
      case "Home": focus(0); break;
      case "End": focus(-1); break;
      case "Escape": this.closeTargetModeMenu({ focusButton: true }); break;
      case "Tab": this.closeTargetModeMenu(); return;
      default: return;
    }
    event.preventDefault();
  },
});
