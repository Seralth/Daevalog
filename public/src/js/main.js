// Start-up: creates the app and starts it once the bridge is ready.
const dpsApp = DpsApp.createInstance();
window.dpsApp = dpsApp;
const debug = globalThis.uiDebug || { log: () => {}, clear: () => {} };

window.addEventListener("error", (event) => {
  debug?.log?.("window.error", {
    message: event.message,
    source: event.filename,
    line: event.lineno,
    column: event.colno,
  });
});

window.addEventListener("unhandledrejection", (event) => {
  debug?.log?.("unhandledrejection", event.reason);
});


let appStarted = false;
const startApp = async ({ forced = false } = {}) => {
  if (appStarted) return;
  appStarted = true;
  debug?.log?.("startApp", {
    readyState: document.readyState,
    hasDpsData: !!window.dpsData,
    hasJavaBridge: !!window.javaBridge,
    forced,
  });
  // Quit, Close and Escape in the Settings window are answered by a script
  // the window runs before this page (SETTINGS_WINDOW_SCRIPT in app.rs), so
  // they work before any of this has loaded.
  try {
    await window.i18n?.init?.();
    window.lucide?.createIcons?.();
    dpsApp.start();
    if (window.A2_VIEW === "details") {
      dpsApp.enterDetailsWindowMode();
    } else if (window.A2_VIEW === "settings") {
      dpsApp.enterSettingsWindowMode();
    } else if (window.A2_VIEW === "history") {
      dpsApp.enterHistoryWindowMode();
    }
    window.javaBridge?.notifyUiReady?.();

  } catch (err) {
    debug?.log?.("startApp.error", err);
  }
};

const waitForBridgeAndStart = (attempt = 0) => {
  // JavaFX WebView injects these after loadWorker SUCCEEDED (slightly later than DOMContentLoaded)
  const ready = !!window.javaBridge && !!window.dpsData;

  debug?.log?.("waitForBridge", {
    attempt,
    readyState: document.readyState,
    hasDpsData: !!window.dpsData,
    hasJavaBridge: !!window.javaBridge,
  });

  if (ready) {
    startApp();
    return;
  }

  if (attempt >= 200) {
    debug?.log?.("waitForBridge.timeout", "Bridge not ready after 10s; forcing UI startup.");
    startApp({ forced: true });
    return;
  }

  setTimeout(() => waitForBridgeAndStart(attempt + 1), 50);
};

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", waitForBridgeAndStart, { once: true });
} else {
  waitForBridgeAndStart();
}
