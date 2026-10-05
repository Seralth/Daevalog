// Settings: the keybind buttons.
"use strict";

Object.assign(DpsApp.prototype, {
  setupKeybindButtons() {
    const MOD_ALT = 1;
    const MOD_CONTROL = 2;
    const MOD_SHIFT = 4;
    const MOD_WIN = 8;

    const vkKeyName = (keyCode) => {
      if (keyCode >= 65 && keyCode <= 90) return String.fromCharCode(keyCode);
      if (keyCode >= 48 && keyCode <= 57) return String(keyCode - 48);
      if (keyCode >= 112 && keyCode <= 123) return `F${keyCode - 111}`;
      if (keyCode >= 96 && keyCode <= 105) return `Num${keyCode - 96}`;
      const names = {
        8: "Backspace", 9: "Tab", 13: "Enter", 19: "Pause", 20: "CapsLock",
        27: "Esc", 32: "Space", 33: "PgUp", 34: "PgDn", 35: "End", 36: "Home",
        37: "Left", 38: "Up", 39: "Right", 40: "Down",
        45: "Insert", 46: "Delete",
        106: "Num*", 107: "Num+", 109: "Num-", 110: "Num.", 111: "Num/",
        144: "NumLock", 145: "ScrollLock",
        186: ";", 187: "=", 188: ",", 189: "-", 190: ".", 191: "/",
        192: "`", 219: "[", 220: "\\", 221: "]", 222: "'",
      };
      return names[keyCode] || `Key${keyCode}`;
    };

    const formatBinding = (mods, keyCode) => {
      if (!mods && !keyCode) return "—";
      const parts = [];
      if (mods & MOD_CONTROL) parts.push("Ctrl");
      if (mods & MOD_ALT) parts.push("Alt");
      if (mods & MOD_SHIFT) parts.push("Shift");
      if (mods & MOD_WIN) parts.push("Win");
      if (keyCode) parts.push(vkKeyName(keyCode));
      return parts.join("+") || "—";
    };

    const isModifierKeyCode = (kc) =>
      kc === 16 || kc === 17 || kc === 18 || kc === 91 || kc === 92 ||
      kc === 93 || kc === 224;

    const reloadBtn = document.querySelector(".reloadKeybindBtn");
    const toggleBtn = document.querySelector(".toggleKeybindBtn");
    const lockKeyBtn = document.querySelector(".lockKeybindBtn");

    this.refreshKeybindLabels = () => {
      const reloadLabel = window.javaBridge?.getCurrentHotKey?.() || "";
      const toggleLabel = window.javaBridge?.getCurrentToggleWindowHotKey?.() || "";
      if (reloadBtn) {
        reloadBtn.querySelector(".keybindText").textContent = reloadLabel || "Ctrl+Alt+R";
      }
      if (toggleBtn) {
        toggleBtn.querySelector(".keybindText").textContent = toggleLabel || "Ctrl+Alt+Up";
      }
      if (lockKeyBtn) {
        lockKeyBtn.querySelector(".keybindText").textContent =
          window.javaBridge?.getCurrentLockHotKey?.() || "Ctrl+Alt+L";
      }
    };
    this.refreshKeybindLabels();

    let activeRecording = null;

    const stopRecording = () => {
      if (!activeRecording) return;
      activeRecording.btn.classList.remove("recording");
      activeRecording = null;
    };

    const startRecording = (btn, type) => {
      stopRecording();
      btn.classList.add("recording");
      btn.querySelector(".keybindText").textContent =
        this.i18n?.t?.("settings.keybind.pressKeys", "Press keys...") ?? "Press keys...";
      activeRecording = { btn, type };
    };

    const handleKeybindClick = (btn, type) => {
      if (activeRecording?.btn === btn) {
        stopRecording();
        this.refreshKeybindLabels();
        return;
      }
      startRecording(btn, type);
    };

    reloadBtn?.addEventListener("click", () => handleKeybindClick(reloadBtn, "reload"));
    toggleBtn?.addEventListener("click", () => handleKeybindClick(toggleBtn, "toggle"));
    lockKeyBtn?.addEventListener("click", () => handleKeybindClick(lockKeyBtn, "lock"));

    document.addEventListener("keydown", (event) => {
      if (!activeRecording) return;
      event.preventDefault();
      event.stopPropagation();

      if (event.key === "Escape") {
        stopRecording();
        this.refreshKeybindLabels();
        return;
      }

      if (isModifierKeyCode(event.keyCode)) {
        const parts = [];
        if (event.ctrlKey) parts.push("Ctrl");
        if (event.altKey) parts.push("Alt");
        if (event.shiftKey) parts.push("Shift");
        if (event.metaKey) parts.push("Win");
        activeRecording.btn.querySelector(".keybindText").textContent =
          parts.length ? parts.join("+") + "+..." : (this.i18n?.t?.("settings.keybind.pressKeys", "Press keys...") ?? "Press keys...");
        return;
      }

      let mods = 0;
      if (event.ctrlKey) mods |= MOD_CONTROL;
      if (event.altKey) mods |= MOD_ALT;
      if (event.shiftKey) mods |= MOD_SHIFT;
      if (event.metaKey) mods |= MOD_WIN;

      const modCount = ((mods & MOD_CONTROL) ? 1 : 0) +
        ((mods & MOD_ALT) ? 1 : 0) +
        ((mods & MOD_SHIFT) ? 1 : 0) +
        ((mods & MOD_WIN) ? 1 : 0);

      if (modCount < 1) {
        activeRecording.btn.querySelector(".keybindText").textContent =
          this.i18n?.t?.("settings.keybind.needModifiers", "Need 1+ modifier") ?? "Need 1+ modifier";
        setTimeout(() => {
          if (activeRecording) {
            activeRecording.btn.querySelector(".keybindText").textContent =
              this.i18n?.t?.("settings.keybind.pressKeys", "Press keys...") ?? "Press keys...";
          }
        }, 1200);
        return;
      }

      const vk = event.keyCode;
      const label = formatBinding(mods, vk);
      const { type } = activeRecording;

      stopRecording();

      if (type === "reload") {
        window.javaBridge?.setHotkey?.(mods, vk);
        if (reloadBtn) reloadBtn.querySelector(".keybindText").textContent = label;
      } else if (type === "toggle") {
        window.javaBridge?.setToggleWindowHotkey?.(mods, vk);
        if (toggleBtn) toggleBtn.querySelector(".keybindText").textContent = label;
      } else if (type === "lock") {
        window.javaBridge?.setLockHotkey?.(mods, vk);
        if (lockKeyBtn) lockKeyBtn.querySelector(".keybindText").textContent = label;
      }
    }, true);

    document.addEventListener("click", (event) => {
      if (!activeRecording) return;
      if (event.target?.closest?.(".keybindBtn")) return;
      stopRecording();
      this.refreshKeybindLabels();
    });
  },
});
