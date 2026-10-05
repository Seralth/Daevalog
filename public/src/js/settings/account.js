// Settings: the account sign-in panel.
"use strict";

Object.assign(DpsApp.prototype, {
  setAccountState(text) {
    if (!this.accountStateEl) return;
    this.accountStateEl.textContent = text;
    // A short state stays on one line beside its button; a long one (an
    // error saying why sign-in failed) wraps rather than being cut off.
    this.accountStateEl.title = text || "";
    this.accountStateEl.classList.toggle("isLong", String(text || "").length > 40);
  },

  // Reflects whatever the backend reports. Called on open, after sign-out, and
  // from the "account-changed" event the device-grant poll emits when the
  // player approves or declines in the browser.
  async refreshAccountPanel(result) {
    if (!this.accountStateEl) return;
    if (result && result.connected === false && result.error) {
      const msg = String(result.error);
      this.setAccountState(
        window.i18n?.format?.("settings.account.failed", { error: msg }, `Sign-in failed: ${msg}`)
      );
      if (this.accountCodeBox) this.accountCodeBox.style.display = "none";
      if (this.accountConnectBtn) this.accountConnectBtn.disabled = false;
      return;
    }

    // The last known answer first, so the panel is right the moment it opens;
    // the server's can take seconds, and nothing else waits on it.
    if (!result) {
      try {
        const seen = await window.javaBridge?.accountStatusCached?.();
        if (seen) this.paintAccount(seen.who);
      } catch {}
    }

    let who = null;
    try {
      who = await window.javaBridge?.accountStatus?.();
    } catch (err) {
      // A token is stored but could not be checked (keyring locked, server
      // down): say why, and do not ask for a new sign-in.
      this.paintAccountUnavailable(typeof err === "string" ? err : err?.message || String(err));
      return;
    }
    this.paintAccount(who);
  },

  paintAccountUnavailable(message) {
    if (this.accountCodeBox?.style.display === "block") return;
    if (this.accountConnectBtn) this.accountConnectBtn.style.display = "none";
    if (this.accountSignOutBtn) this.accountSignOutBtn.style.display = "";
    this.setAccountState(message);
  },

  paintAccount(who) {
    const signedIn = !!who;
    // A sign-in waiting for approval in the browser keeps its code and its
    // "waiting" line; its outcome arrives on "account-changed".
    if (!signedIn && this.accountCodeBox?.style.display === "block") return;
    if (this.accountCodeBox && signedIn) this.accountCodeBox.style.display = "none";
    if (this.accountConnectBtn) {
      this.accountConnectBtn.disabled = false;
      this.accountConnectBtn.style.display = signedIn ? "none" : "";
    }
    if (this.accountSignOutBtn) {
      this.accountSignOutBtn.style.display = signedIn ? "" : "none";
    }

    if (!signedIn) {
      this.setAccountState(window.i18n?.t?.("settings.account.signedOut", "Not signed in"));
      return;
    }
    const name = who.displayName || who.display_name || "";
    let label = window.i18n?.format?.(
      "settings.account.signedIn",
      { name },
      `Signed in as ${name}`
    );
    if (who.supporter) {
      label += " · " + window.i18n?.t?.("settings.account.supporter", "Supporter");
    }
    this.setAccountState(label);
  },
});
