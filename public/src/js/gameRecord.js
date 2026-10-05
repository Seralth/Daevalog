// The game's own Damage Analyzer numbers (Ctrl+X in game) beside the meter's,
// in Details for a saved fight. The backend matches records to fights and
// replays the fight's packets over the record's window (game_record_details).
const createGameRecordUI = () => {
  const panel = document.querySelector(".detailsPanel");
  const section = panel?.querySelector(".gameRecordSection");
  if (!section) return null;

  const switchBtns = [...section.querySelectorAll(".gameRecordViewBtn")];
  const pickEl = section.querySelector(".gameRecordPick");
  const meterTotalEl = section.querySelector(".gameRecordMeterTotal");
  const gameTotalEl = section.querySelector(".gameRecordGameTotal");
  const meterTotalBox = meterTotalEl?.closest(".gameRecordTotal");
  const verdictEl = section.querySelector(".gameRecordVerdict");
  const onlyDifferEl = section.querySelector(".gameRecordOnlyDiffer input");
  const onlyDifferLabel = section.querySelector(".gameRecordOnlyDiffer");
  const coverageEl = section.querySelector(".gameRecordCoverage");
  const tableEl = section.querySelector(".gameRecordTable");
  const copyBtn = section.querySelector(".gameRecordCopy");
  const bodyEls = [...section.querySelectorAll(".gameRecordBody")];

  const t = (key, fallback) => window.i18n?.t?.(key, fallback) ?? fallback;
  const f = (key, vars, fallback) =>
    window.i18n?.format?.(key, vars, fallback) ??
    Object.entries(vars).reduce((s, [k, v]) => s.replaceAll(`{${k}}`, v), fallback);
  const num = (v) => Number(v || 0).toLocaleString("en-US");
  const clock = (ms) => {
    const d = new Date(Number(ms));
    const p = (n) => String(n).padStart(2, "0");
    return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
  };

  // Damage, then the counts in the order the backend sends them.
  const COLUMNS = [
    ["hits", "Hits"], ["crit", "Crit"], ["perfect", "Perfect"], ["double", "Double"],
    ["front", "Front"], ["back", "Back"], ["addhit", "Add. hits"],
  ];

  let records = [];
  let current = null;
  let view = "both";
  let seq = 0;

  const syncView = () => {
    const compared = !!current?.compared;
    if (view === "both" && !compared) view = "game";
    switchBtns.forEach((b) => {
      const v = b.dataset.view;
      b.classList.toggle("isActive", v === view);
      b.setAttribute("aria-pressed", v === view ? "true" : "false");
      b.disabled = v === "both" && !compared;
    });
    // Meter: the usual skill list. Game record or Both: the record's table.
    panel.classList.toggle("showGameRecord", !!current && view !== "meter");
    bodyEls.forEach((el) => { el.hidden = view === "meter"; });
  };

  const summary = () => {
    const c = current;
    gameTotalEl.textContent = num(c.gameTotal);
    meterTotalBox.hidden = !c.compared;
    meterTotalEl.textContent = num(c.meterTotal);
    onlyDifferLabel.hidden = !c.compared || view !== "both";
    verdictEl.classList.remove("isMatch", "isDiffer");
    if (!c.compared) {
      verdictEl.textContent = t("gameRecord.noComparison", "No packets saved for this fight, so only the game's numbers are shown.");
      return;
    }
    const n = c.rows.length;
    const same = c.rows.filter((r) => r.same).length;
    verdictEl.classList.add(same === n ? "isMatch" : "isDiffer");
    verdictEl.textContent = same === n
      ? f("gameRecord.allMatch", { n }, "All {n} skills match")
      : f("gameRecord.someMatch", { x: same, n }, "{x} of {n} skills match");
  };

  const coverage = () => {
    const c = current;
    const parts = [
      f("gameRecord.window", { from: clock(c.startMs), to: clock(c.endMs) }, "Game record from {from} to {to}."),
    ];
    let covers = f("gameRecord.covers", { s: Math.round(c.coveredMs / 1000) }, "Covers {s} s of this fight");
    if (c.otherFights === 1) covers += `, ${t("gameRecord.alsoCoversOne", "also covers 1 other fight")}`;
    else if (c.otherFights > 1) covers += `, ${f("gameRecord.alsoCovers", { n: c.otherFights }, "also covers {n} other fights")}`;
    parts.push(`${covers}.`);
    parts.push(f("gameRecord.targetOnly", { target: c.target }, "Only damage to {target} is compared."));
    coverageEl.textContent = parts.join(" ");
  };

  const cell = (value, gameValue, showTag) => {
    const el = document.createElement("span");
    el.className = "gameRecordNum";
    el.textContent = num(value);
    if (showTag && value !== gameValue) {
      el.classList.add("isDiffer");
      const tag = document.createElement("span");
      tag.className = "gameRecordTag";
      tag.textContent = f("gameRecord.gameTag", { n: num(gameValue) }, "game {n}");
      el.appendChild(tag);
    }
    return el;
  };

  const table = () => {
    const c = current;
    const both = view === "both" && c.compared;
    const side = (r) => (both ? r.meter : r.game);
    const total = c.rows.reduce((s, r) => s + side(r).damage, 0) || 1;
    const onlyDiffer = both && onlyDifferEl.checked;
    const rows = c.rows.filter((r) => (both || r.game.damage > 0 || r.game.counts.some(Boolean)) && (!onlyDiffer || !r.same));

    tableEl.innerHTML = "";
    const head = document.createElement("div");
    head.className = "gameRecordRow gameRecordHead";
    const headCells = [
      t("gameRecord.col.skill", "Skill"), t("gameRecord.col.damage", "Damage"), t("gameRecord.col.share", "Share"),
      ...COLUMNS.map(([k, label]) => t(`gameRecord.col.${k}`, label)),
    ];
    headCells.forEach((label, i) => {
      const el = document.createElement("span");
      if (i === 0) el.className = "gameRecordName";
      el.textContent = label;
      head.appendChild(el);
    });
    tableEl.appendChild(head);

    rows.forEach((r) => {
      const row = document.createElement("div");
      row.className = "gameRecordRow";
      if (both && !r.same) row.classList.add("isDiffer");
      const name = document.createElement("span");
      name.className = "gameRecordName";
      name.textContent = c.names?.[r.skillId] || `#${r.skillId}`;
      row.appendChild(name);
      row.appendChild(cell(side(r).damage, r.game.damage, both));
      const share = document.createElement("span");
      share.className = "gameRecordShare";
      share.textContent = `${((side(r).damage / total) * 100).toFixed(1)}%`;
      row.appendChild(share);
      COLUMNS.forEach((_, i) => row.appendChild(cell(side(r).counts[i], r.game.counts[i], both)));
      tableEl.appendChild(row);
    });

    const hidden = c.rows.length - rows.length;
    if (onlyDiffer && hidden > 0) {
      const note = document.createElement("div");
      note.className = "gameRecordMore";
      note.textContent = f("gameRecord.othersMatch", { n: hidden }, "{n} other skills match the game on every number.");
      tableEl.appendChild(note);
    }
    copyBtn.hidden = !c.compared;
  };

  const render = () => {
    if (!current) return;
    syncView();
    summary();
    coverage();
    table();
  };

  const fillPick = () => {
    pickEl.innerHTML = "";
    records.forEach((r, i) => {
      const opt = document.createElement("option");
      opt.value = String(i);
      opt.textContent = `${clock(r.startMs)} – ${clock(r.endMs)}`;
      pickEl.appendChild(opt);
    });
    pickEl.hidden = records.length < 2;
  };

  const hide = () => {
    seq++;
    records = [];
    current = null;
    section.hidden = true;
    panel.classList.remove("showGameRecord");
  };

  // A saved fight opened in Details: show its game records, if it has any.
  const show = async (fight) => {
    hide();
    const mine = seq;
    const id = fight?.id;
    if (!id) return;
    let list = [];
    try {
      list = (await window.javaBridge?.gameRecordDetails?.(id)) || [];
    } catch {
      list = [];
    }
    if (mine !== seq || !Array.isArray(list) || list.length === 0) return;
    records = list;
    current = records[records.length - 1];
    fillPick();
    pickEl.value = String(records.length - 1);
    view = current.compared ? "both" : "game";
    onlyDifferEl.checked = false;
    section.hidden = false;
    render();
  };

  switchBtns.forEach((b) => b.addEventListener("click", () => {
    if (b.disabled) return;
    view = b.dataset.view;
    render();
  }));
  pickEl.addEventListener("change", () => {
    current = records[Number(pickEl.value)] || current;
    render();
  });
  onlyDifferEl.addEventListener("change", table);
  copyBtn.addEventListener("click", async () => {
    if (!current?.report) return;
    try {
      await navigator.clipboard.writeText(current.report);
      copyBtn.textContent = t("gameRecord.copied", "Copied");
    } catch {
      copyBtn.textContent = t("gameRecord.copyFailed", "Could not copy");
    }
    setTimeout(() => { copyBtn.textContent = t("gameRecord.copy", "Copy for a bug report"); }, 1500);
  });

  hide();
  return { show, hide };
};

window.gameRecordUI = createGameRecordUI();
