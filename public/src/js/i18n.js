const createI18n = ({
  defaultLanguage = "en",
  storageKey = "dpsMeter.language",
  supportedLanguages = [
    "en", "de", "es", "fr", "ja", "ko", "pt", "ru", "zh-Hant", "zh-Hans",
  ],
} = {}) => {
  let currentLanguage = defaultLanguage;
  let uiStrings = {};
  let skillStrings = {};
  let npcStrings = {};
  let dungeonStrings = {};
  const listeners = new Set();

  const safeGetStorage = (key) => {
    try {
      const bridgeValue = window.javaBridge?.getSetting?.(key);
      if (bridgeValue !== undefined && bridgeValue !== null) {
        return bridgeValue;
      }
    } catch {
      // ignore
    }
    try {
      return localStorage.getItem(key);
    } catch {
      return null;
    }
  };

  const safeSetStorage = (key, value) => {
    try {
      window.javaBridge?.setSetting?.(key, value);
    } catch {
      // ignore
    }
    try {
      localStorage.setItem(key, value);
    } catch {
      // ignore
    }
  };

  const normalizeLanguage = (lang) =>
    supportedLanguages.includes(lang) ? lang : defaultLanguage;

  const resolveUrl = (path) => {
    try {
      return new URL(path, document.baseURI || window.location.href).toString();
    } catch {
      return path;
    }
  };

  const parseJsonText = (text) => {
    if (typeof text !== "string") return {};
    try {
      const parsed = JSON.parse(text);
      return parsed && typeof parsed === "object" ? parsed : {};
    } catch {
      return {};
    }
  };

  const normalizeBridgePath = (path) => {
    if (!path) return "/";
    const trimmed = path.startsWith("./") ? path.slice(2) : path;
    return trimmed.startsWith("/") ? trimmed : `/${trimmed}`;
  };

  const loadJsonFromBridge = (path) => {
    const raw = window.javaBridge?.readResource?.(normalizeBridgePath(path));
    return parseJsonText(raw);
  };

  const loadJson = async (path) => {
    const url = resolveUrl(path);
    try {
      const res = await fetch(url, { cache: "no-store" });
      if (res.ok || res.status === 0) {
        const buffer = await res.arrayBuffer();
        const text = new TextDecoder("utf-8").decode(buffer);
        const data = parseJsonText(text);
        if (Object.keys(data).length) return data;
      }
    } catch {
      // ignore and fall back
    }

    const xhrText = await new Promise((resolve) => {
      try {
        const xhr = new XMLHttpRequest();
        xhr.open("GET", url, true);
        xhr.responseType = "arraybuffer";
        xhr.onload = () => {
          if (xhr.status && xhr.status !== 200) {
            resolve(null);
            return;
          }
          if (!xhr.response) {
            resolve("");
            return;
          }
          try {
            const decoded = new TextDecoder("utf-8").decode(xhr.response);
            resolve(decoded);
          } catch {
            resolve("");
          }
        };
        xhr.onerror = () => resolve(null);
        xhr.send();
      } catch {
        resolve(null);
      }
    });

    if (xhrText) {
      const parsed = parseJsonText(xhrText);
      if (Object.keys(parsed).length) return parsed;
    }

    return loadJsonFromBridge(path);
  };

  const resolveKey = (obj, key) => {
    if (!obj || !key) return undefined;
    return key.split(".").reduce((acc, part) => (acc ? acc[part] : undefined), obj);
  };

  const t = (key, fallback = "") => {
    const value = resolveKey(uiStrings, key);
    if (typeof value === "string") return value;
    return fallback;
  };

  const format = (key, vars = {}, fallback = "") => {
    const template = t(key, fallback);
    if (!template) return fallback;
    return template.replace(/\{(\w+)\}/g, (_, varKey) => {
      const replacement = vars[varKey];
      return replacement === undefined || replacement === null ? "" : String(replacement);
    });
  };

  const getSkillName = (code, fallback = "") => {
    const value = skillStrings?.[String(code)];
    if (typeof value === "string" && value.trim()) return value;
    // Theostone DOT codes: 7-digit codes (3000000-3099999) map to 8-digit IDs (code*10+1)
    const num = Number(code);
    if (num >= 3000000 && num <= 3099999) {
      const tsValue = skillStrings?.[String(num * 10 + 1)];
      if (typeof tsValue === "string" && tsValue.trim()) return tsValue;
    }
    return fallback;
  };

  const getNpcName = (id, fallback = "") => {
    const value = npcStrings?.[String(id)];
    if (typeof value === "string" && value.trim()) return value;
    if (value && typeof value === "object") {
      const name = value?.name;
      if (typeof name === "string" && name.trim()) return name;
    }
    return fallback;
  };

  const applyTranslations = () => {
    document.querySelectorAll("[data-i18n]").forEach((el) => {
      const key = el.dataset.i18n;
      const text = t(key, el.textContent ?? "");
      if (text) el.textContent = text;
    });

    document.querySelectorAll("[data-i18n-tip]").forEach((el) => {
      const key = el.dataset.i18nTip;
      const text = t(key, el.getAttribute("data-tip") ?? "");
      if (text) el.setAttribute("data-tip", text);
    });

    // Native tooltips, for windows without the meter's own (data-tip) one.
    document.querySelectorAll("[data-i18n-title]").forEach((el) => {
      const key = el.dataset.i18nTitle;
      const text = t(key, el.getAttribute("title") ?? "");
      if (text) el.setAttribute("title", text);
    });

    document.querySelectorAll("[data-i18n-placeholder]").forEach((el) => {
      const key = el.dataset.i18nPlaceholder;
      const text = t(key, el.getAttribute("placeholder") ?? "");
      if (text) el.setAttribute("placeholder", text);
    });

    document.querySelectorAll("[data-i18n-aria-label]").forEach((el) => {
      const key = el.dataset.i18nAriaLabel;
      const text = t(key, el.getAttribute("aria-label") ?? "");
      if (text) el.setAttribute("aria-label", text);
    });
  };

  const LANGUAGE_EVENT = "language-changed";

  let languageSeq = 0;
  const setLanguage = async (lang, { persist = true } = {}) => {
    const next = normalizeLanguage(lang || defaultLanguage);
    const seq = ++languageSeq;
    currentLanguage = next;

    if (persist) {
      safeSetStorage(storageKey, next);
      // Each window holds its own strings: the others follow this change.
      window.__TAURI__?.event?.emit?.(LANGUAGE_EVENT, next);
    }

    const localized = async (kind) => {
      const strings = await loadJson(`./i18n/${kind}/${next}.json`);
      return Object.keys(strings).length || next === "en"
        ? strings : loadJson(`./i18n/${kind}/en.json`);
    };
    const [ui, skills, npcs, dungeons] = await Promise.all([
      localized("ui"), localized("skills"), localized("npcs"), localized("dungeons"),
    ]);
    // A newer choice started while these loaded: it wins, whichever loads last.
    if (seq !== languageSeq) return;

    uiStrings = ui || {};
    skillStrings = skills || {};
    npcStrings = npcs || {};
    dungeonStrings = dungeons || {};
    document.documentElement.setAttribute("lang", currentLanguage);
    applyTranslations();
    listeners.forEach((listener) => listener(currentLanguage));
  };

  const init = async () => {
    const stored = safeGetStorage(storageKey);
    await setLanguage(stored || defaultLanguage, { persist: false });
    // A language picked in another window (Settings): History, Details and
    // the meter showed the old one until they were reopened.
    window.__TAURI__?.event?.listen?.(LANGUAGE_EVENT, (event) => {
      const lang = normalizeLanguage(event?.payload);
      if (lang !== currentLanguage) void setLanguage(lang, { persist: false });
    });
  };

  const onChange = (listener) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  };

  // The instance's difficulty in the game's own words, with its party tier
  // ({ key: "advanced", label: "Conquest [Hard] · Conquest Tier 4" }), or null
  // where the game gives none: sealed, quest and Abyss dungeons among others.
  // scripts/game-tables.py takes both from the game's Dungeon table.
  const getDungeonDifficulty = (dungeonId) => {
    const entry = dungeonStrings?.[String(Number(dungeonId) || 0)];
    // A Conquest row may carry only its tier (Krao Cave's Conquest is tier 1).
    return entry?.label ? { key: entry.difficulty || "tier", label: entry.label } : null;
  };

  // "Cradle of Nihility (Conquest [Hard] · Conquest Tier 4)" for an instance,
  // or the name alone where the game gives no difficulty.
  const getDungeonLabel = (dungeonId) => {
    const entry = dungeonStrings?.[String(dungeonId)];
    if (!entry || !entry.name) return "";
    const tier = getDungeonDifficulty(dungeonId);
    return tier ? `${entry.name} (${tier.label})` : entry.name;
  };

  return {
    init,
    setLanguage,
    t,
    format,
    getSkillName,
    getNpcName,
    getDungeonLabel,
    getDungeonDifficulty,
    getLanguage: () => currentLanguage,
    onChange,
  };
};

window.i18n = createI18n();
