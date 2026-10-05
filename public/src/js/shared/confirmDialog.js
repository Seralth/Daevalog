// A yes/no question in the meter's own style. window.confirm() opens a
// native WebKitGTK box that ignores the meter's theme.
(() => {
  const ask = (text, { ok = "OK", cancel = "Cancel" } = {}) =>
    new Promise((resolve) => {
      const modal = document.createElement("div");
      modal.className = "reportModal isOpen confirmModal";
      const card = document.createElement("div");
      card.className = "reportCard";
      card.setAttribute("role", "alertdialog");
      card.setAttribute("aria-modal", "true");
      const body = document.createElement("div");
      body.className = "confirmText";
      body.textContent = text;
      const actions = document.createElement("div");
      actions.className = "confirmActions";
      const button = (label, primary) => {
        const b = document.createElement("button");
        b.type = "button";
        b.className = primary ? "reportBtn confirmYes" : "reportBtn";
        b.textContent = label;
        return b;
      };
      const no = button(cancel, false);
      const yes = button(ok, true);
      const done = (answer) => {
        document.removeEventListener("keydown", onKey, true);
        modal.remove();
        resolve(answer);
      };
      const onKey = (e) => {
        if (e.key === "Escape") { e.preventDefault(); done(false); }
      };
      no.addEventListener("click", () => done(false));
      yes.addEventListener("click", () => done(true));
      modal.addEventListener("click", (e) => { if (e.target === modal) done(false); });
      document.addEventListener("keydown", onKey, true);
      actions.append(no, yes);
      card.append(body, actions);
      modal.append(card);
      document.body.append(modal);
      no.focus();
    });
  window.confirmDialog = { ask };
})();
