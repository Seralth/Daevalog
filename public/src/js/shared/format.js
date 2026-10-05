// Fight-time and number formatters shared by the front-end scripts.
const formatBattleTime = (ms) => {
  const totalMs = Number(ms);
  if (!Number.isFinite(totalMs) || totalMs <= 0) return "00:00";
  const totalSeconds = Math.floor(totalMs / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
};

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
