// The game's own Damage Analyzer numbers (Ctrl+X in game) beside the meter's,
// in Details for a saved fight. The backend matches records to fights and
// replays the fight's packets over the record's window (game_record_details);
// details.js draws the numbers in its standard skill table (setGameView).
const createGameRecordUI = () => {
  const panel = document.querySelector(".detailsPanel");
  const section = panel?.querySelector(".gameRecordSection");
  if (!section) return null;

  const switchBtns = [...section.querySelectorAll(".gameRecordViewBtn")];
  const pickEl = section.querySelector(".gameRecordPick");
  const meterTotalEl = section.querySelector(".gameRecordMeterTotal");
  const gameTotalEl = section.querySelector(".gameRecordGameTotal");
  const meterTotalBox = meterTotalEl?.closest(".gameRecordTotal");
  const meterTakenEl = section.querySelector(".gameRecordMeterTaken");
  const gameTakenEl = section.querySelector(".gameRecordGameTaken");
  const meterTakenBox = meterTakenEl?.closest(".gameRecordTotal");
  const verdictEl = section.querySelector(".gameRecordVerdict");
  const onlyDifferEl = section.querySelector(".gameRecordOnlyDiffer input");
  const onlyDifferLabel = section.querySelector(".gameRecordOnlyDiffer");
  const coverageEl = section.querySelector(".gameRecordCoverage");
  const copyBtn = section.querySelector(".gameRecordCopy");
  const footEl = section.querySelector(".gameRecordFoot");
  const bodyEls = [...section.querySelectorAll(".gameRecordBody")];

  const t = (key, fallback) => window.i18n?.t?.(key, fallback) ?? fallback;
  const f = (key, vars, fallback) =>
    window.i18n?.format?.(key, vars, fallback) ??
    Object.entries(vars).reduce((s, [k, v]) => s.replaceAll(`{${k}}`, v), fallback);
  const num = (v) => Number(v || 0).toLocaleString("en-US");
  // A record's start and end, to the second, in Settings' Display Time format.
  const clock = (ms) => clockText(ms, { seconds: true });

  let records = [];
  let current = null;
  let fight = null;
  let api = null;
  let view = "both";
  let seq = 0;

  // The player the record is about, and their class for icons and colours.
  const player = () => {
    const actors = Array.isArray(fight?.actors) ? fight.actors : [];
    const id = Number(current?.actorId) || Number(actors[0]?.actorId) || 0;
    const actor = actors.find((a) => Number(a.actorId) === id);
    return { actorId: id, job: actor?.job || "" };
  };

  const syncView = () => {
    const compared = !!current?.compared;
    if (view === "both" && !compared) view = "game";
    switchBtns.forEach((b) => {
      const v = b.dataset.view;
      b.classList.toggle("isActive", v === view);
      b.setAttribute("aria-pressed", v === view ? "true" : "false");
      b.disabled = v === "both" && !compared;
    });
    bodyEls.forEach((el) => { el.hidden = view === "meter"; });
  };

  const summary = () => {
    const c = current;
    gameTotalEl.textContent = num(c.gameTotal);
    meterTotalBox.hidden = !c.compared;
    meterTotalEl.textContent = num(c.meterTotal);
    // The damage the player received over the record's window.
    gameTakenEl.textContent = num(c.gameTaken?.damage);
    meterTakenBox.hidden = !c.compared;
    meterTakenEl.textContent = num(c.meterTaken?.damage);
    onlyDifferLabel.hidden = !c.compared || view !== "both";
    footEl.classList.toggle("isHiddenForRecord", !c.compared);
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

  // Hand the numbers to the standard table, or give it back to the meter.
  const table = () => {
    if (!api?.setGameView) return;
    if (!current || view === "meter") {
      api.setGameView(null);
      return;
    }
    const both = view === "both" && current.compared;
    api.setGameView({
      mode: both ? "both" : "game",
      rows: current.rows,
      names: current.names,
      gameTotal: current.gameTotal,
      taken: both ? current.meterTaken : current.gameTaken,
      onlyDiffer: both && onlyDifferEl.checked,
      ...player(),
    });
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
    api?.setGameView?.(null);
    api = null;
  };

  // A saved fight opened in Details: show its game records, if it has any.
  // `detailsApi.setGameView` draws them in the standard table.
  const show = async (record, detailsApi) => {
    hide();
    const mine = seq;
    const id = record?.id;
    if (!id) return;
    let list = [];
    try {
      list = (await window.javaBridge?.gameRecordDetails?.(id)) || [];
    } catch {
      list = [];
    }
    if (mine !== seq || !Array.isArray(list) || list.length === 0) return;
    fight = record;
    api = detailsApi || null;
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

  // Display Time changed in Settings: the record list and its window again.
  window.addEventListener?.(CLOCK_FORMAT_EVENT, () => {
    if (!current) return;
    fillPick();
    pickEl.value = String(records.indexOf(current));
    coverage();
  });

  hide();
  return { show, hide };
};

window.gameRecordUI = createGameRecordUI();
