// Fight-time and number formatters shared by the front-end scripts.
const formatBattleTime = (ms) => {
  const totalMs = Number(ms);
  if (!Number.isFinite(totalMs) || totalMs <= 0) return "00:00";
  const totalSeconds = Math.floor(totalMs / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
};

// A time of day, in the format Settings' Display Time picks. 24-hour: 14:05,
// 00:30. 12-hour: 2:05 PM, 12:30 AM; in Korean 오후 2:05, in Chinese 下午 2:05.
// Fight lengths are durations, not times of day: formatBattleTime.
const TIME_FORMAT_KEY = "dpsMeter.timeFormat";
const CLOCK_FORMAT_EVENT = "clock-format-changed";
const formatClock = (ms, { hour24 = false, seconds = false, lang = "en" } = {}) => {
  const d = new Date(Number(ms));
  if (isNaN(d.getTime())) return "";
  const two = (n) => String(n).padStart(2, "0");
  const hour = d.getHours();
  const rest = `${two(d.getMinutes())}${seconds ? `:${two(d.getSeconds())}` : ""}`;
  if (hour24) return `${two(hour)}:${rest}`;
  const h12 = `${hour % 12 || 12}:${rest}`;
  const pm = hour >= 12;
  if (lang === "ko") return `${pm ? "\uC624\uD6C4" : "\uC624\uC804"} ${h12}`; // 오후 / 오전
  if (lang === "zh-Hans" || lang === "zh-Hant") return `${pm ? "\u4E0B\u5348" : "\u4E0A\u5348"} ${h12}`; // 下午 / 上午
  return `${h12} ${pm ? "PM" : "AM"}`;
};

// The player's choice in Settings, else the system's clock (LC_TIME, read by
// the backend), else 12-hour.
const clockIs24h = () => {
  const bridge = typeof window !== "undefined" ? window.javaBridge : null;
  const chosen = bridge?.getSetting?.(TIME_FORMAT_KEY);
  return (chosen === "24h" || chosen === "12h" ? chosen : bridge?.systemTimeFormat?.()) === "24h";
};

// A time of day as Settings asks for it, in the page's language.
const clockText = (ms, { seconds = false } = {}) => formatClock(ms, {
  hour24: clockIs24h(),
  seconds,
  lang: (typeof window !== "undefined" && window.i18n?.getLanguage?.()) || "en",
});

const formatMMSS = (ms) => {
  const v = Math.max(0, Math.floor(Number(ms) || 0));
  const sec = Math.floor(v / 1000);
  const mm = String(Math.floor(sec / 60)).padStart(2, "0");
  const ss = String(sec % 60).padStart(2, "0");
  return `${mm}:${ss}`;
};

const formatDamageCompact = (v) => {
  const n = Number(v);
  if (!Number.isFinite(n)) return "-";
  const abs = Math.abs(n);
  if (abs >= 1_000_000) {
    return `${(n / 1_000_000).toFixed(2)}m`;
  }
  if (abs >= 1_000) {
    return `${(n / 1_000).toFixed(2)}k`;
  }
  return `${Math.round(n)}`;
};

// 1.23k, 4.5m, and whole numbers below 1k through `formatter`.
const formatAbbreviated = (value, formatter) => {
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
  // Whole numbers below 1k too: a per-second rate read "368.036".
  return formatter.format(Math.round(n));
};

// An amount per second as Details' overview writes it; "-" with no time.
const perSecondText = (amount, battleMs, formatter) =>
  (battleMs > 0 ? formatAbbreviated((amount / battleMs) * 1000, formatter) : "-");

const formatDamage = (v) => {
  const n = Number(v);
  if (!Number.isFinite(n)) return "-";
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}m`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return `${Math.round(n)}`;
};

const pctText = (v) => {
  const n = Number(v);
  return Number.isFinite(n) ? `${n.toFixed(1)}%` : "-";
};

const fmtDps = (v) => {
  if (v >= 1_000_000) return (v / 1_000_000).toFixed(1).replace(/\.0$/, "") + "M";
  if (v >= 1_000) return (v / 1_000).toFixed(v >= 10_000 ? 0 : 1).replace(/\.0$/, "") + "K";
  return String(Math.round(v));
};
