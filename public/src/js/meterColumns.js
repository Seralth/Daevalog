// The figures a meter row can show in Encounter mode (ENC), in the order they
// sit on the row. Until a choice is saved, rows keep the usual readout: the
// figure the display toggle picks, then damage %.
//
// `width` is the room a figure needs at the row's type sizes, in px. The first
// column uses the large type, so it gets `firstWidth`. A narrow meter drops
// columns from the end of the list until the rest fit.
(function (root) {
  const COLUMNS = [
    { key: "encDps", label: "ENCDPS", short: "ENC", width: 46, firstWidth: 62 },
    { key: "dps", label: "DPS (own active time)", short: "DPS", width: 46, firstWidth: 62 },
    { key: "pct", label: "Damage %", short: "%", width: 36, firstWidth: 52 },
    { key: "total", label: "Total damage", short: "Total", width: 40, firstWidth: 54 },
    { key: "crit", label: "Crit %", short: "CRI", width: 32, firstWidth: 44 },
    { key: "last10", label: "Last 10 s", short: "10s", width: 46, firstWidth: 62 },
    { key: "last30", label: "Last 30 s", short: "30s", width: 46, firstWidth: 62 },
    { key: "last60", label: "Last 60 s", short: "60s", width: 46, firstWidth: 62 },
    { key: "maxHit", label: "Max hit", short: "Max", width: 40, firstWidth: 54 },
    { key: "hits", label: "Hits", short: "HIT", width: 36, firstWidth: 48 },
  ];
  const BY_KEY = new Map(COLUMNS.map((c) => [c.key, c]));
  // Space between two figures (the row's flex gap).
  const GAP = 7;
  // Row space that is not figures: rank, class icon, a name cut to a few
  // letters, the gaps between them and the padding around the figures.
  const RESERVED = 150;

  // What the Settings switches show before a choice is saved: what the row
  // shows today for each position of the display toggle.
  const defaultsFor = (displayMode) =>
    displayMode === "totalDamage" ? ["total", "pct"]
      : displayMode === "both" ? ["encDps", "pct", "total"]
        : ["encDps", "pct"];

  // A saved choice, as "encDps,pct,hits". Unknown keys are ignored and the
  // order is always the list's. null when nothing valid is saved: then the
  // row keeps the usual readout.
  const parse = (value) => {
    if (typeof value !== "string" || !value.trim()) return null;
    const wanted = new Set(value.split(",").map((s) => s.trim()));
    const keys = COLUMNS.filter((c) => wanted.has(c.key)).map((c) => c.key);
    return keys.length ? keys : null;
  };

  const serialize = (keys) => {
    const wanted = new Set(keys || []);
    return COLUMNS.filter((c) => wanted.has(c.key)).map((c) => c.key).join(",");
  };

  // The leading columns that fit a list `listWidth` px wide. The first one
  // always stays, so a very narrow meter still shows a number.
  const fit = (keys, listWidth) => {
    if (!Array.isArray(keys) || !keys.length) return [];
    if (!(listWidth > 0)) return keys.slice();
    let room = listWidth - RESERVED;
    const out = [];
    for (const key of keys) {
      const col = BY_KEY.get(key);
      if (!col) continue;
      const need = (out.length ? col.width + GAP : col.firstWidth);
      if (out.length && need > room) break;
      room -= need;
      out.push(key);
    }
    return out;
  };

  // The text of one figure. `fmt` holds the meter's number formats:
  // rate (DPS, follows "Round DPS"), amount (1.23m) and count (12,345).
  // `damagePct` is the row's share of the damage shown, as for the usual
  // readout.
  const cellText = (key, row, fmt, damagePct) => {
    const num = (v) => Number(v) || 0;
    switch (key) {
      case "encDps": return fmt.rate(num(row.dps));
      case "dps": return fmt.rate(num(row.activeDps));
      case "pct": return `${num(damagePct).toFixed(1)}%`;
      case "total": return fmt.amount(num(row.totalDamage));
      case "crit": {
        const hits = num(row.hits);
        return hits > 0 ? `${Math.round((num(row.critHits) / hits) * 100)}%` : "-";
      }
      case "last10": return fmt.rate(num(row.last10Dps));
      case "last30": return fmt.rate(num(row.last30Dps));
      case "last60": return fmt.rate(num(row.last60Dps));
      case "maxHit": return fmt.amount(num(row.maxHit));
      case "hits": return fmt.count(num(row.hits));
      default: return "";
    }
  };

  root.MeterColumns = { COLUMNS, defaultsFor, parse, serialize, fit, cellText };
})(typeof window !== "undefined" ? window : globalThis);
