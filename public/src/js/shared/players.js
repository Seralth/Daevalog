// How a player is shown. Every view names players through playerLabel, so
// "Hide other players' names" reaches a new screen without more work.
const playerNames = {
  // Settings > Appearance. When on, other players show as their class and a
  // number, never by name, so a screenshot names no one. Display only: saved
  // fights and uploads keep their names.
  hideOthers: false,
};

// Class names as the backend and saved fights give them (Korean, or English
// in older records) -> the key of the class's name in each language.
const PLAYER_CLASS_KEYS = (() => {
  const keys = { ...JOB_KEY_MAP };
  for (const key of Object.values(JOB_KEY_MAP)) keys[key.charAt(0) + key.slice(1).toLowerCase()] = key;
  keys.Spiritmaster = "ELEMENTALIST";
  keys.Brawler = "FIGHTER";
  return keys;
})();

const playerClassLabel = (job) => {
  const key = PLAYER_CLASS_KEYS[String(job ?? "").trim()];
  if (!key) return "";
  const fallback = key.charAt(0) + key.slice(1).toLowerCase();
  return window.i18n?.t?.(`classes.${key}`, fallback) || fallback;
};

// A player a view has no number for gets one after every number it has seen,
// so two players never share one.
const fallbackPlayerNumbers = new Map();
let highestPlayerNumber = 0;

const playerNumber = (id, number) => {
  const n = Math.trunc(Number(number));
  if (n > 0) {
    highestPlayerNumber = Math.max(highestPlayerNumber, n);
    return n;
  }
  const key = String(id ?? "");
  if (!fallbackPlayerNumbers.has(key)) fallbackPlayerNumbers.set(key, ++highestPlayerNumber);
  return fallbackPlayerNumbers.get(key);
};

// `player`: { id, name, job, number, isUser, isIdentifying }. `number` is the
// backend's for a live fight (the same in every window) and the order of
// first hit for a saved one. You are always shown as yourself.
const playerLabel = (player) => {
  const id = player?.id ?? "";
  if (isUnattributedActor(id)) return unattributedLabel();
  const name = String(player?.name ?? "").trim();
  if (player?.isUser || !playerNames.hideOthers) {
    if (name && !player?.isIdentifying && name !== String(id)) return name;
    if (id === "") return name || "-";
    return window.i18n?.format?.("meter.identifyingPlayer", { id }, `#${id}`) || `#${id}`;
  }
  const n = playerNumber(id, player?.number);
  const cls = playerClassLabel(player?.job);
  return cls
    ? window.i18n?.format?.("meter.hiddenPlayer", { class: cls, n }, `${cls} ${n}`) || `${cls} ${n}`
    : window.i18n?.format?.("meter.hiddenPlayerNoClass", { n }, `Player ${n}`) || `Player ${n}`;
};

// A saved fight's numbers: each player by their first hit, then by id. You
// and the unattributed row get none.
const savedFightNumbers = (record, isUser) => {
  const first = new Map();
  const note = (id, ts) => {
    if (!Number.isFinite(id) || id <= 0 || isUnattributedActor(id)) return;
    const prev = first.has(id) ? first.get(id) : Infinity;
    first.set(id, Math.min(prev, Number.isFinite(ts) ? ts : Infinity));
  };
  const details = record?.details || {};
  for (const skill of [...(details.skills || []), ...(details.healSkills || [])]) {
    const times = Array.isArray(skill?.hitTimestamps) ? skill.hitTimestamps : [];
    note(Number(skill?.actorId), times.reduce((min, ts) => Math.min(min, Number(ts)), Infinity));
  }
  for (const actor of Array.isArray(record?.actors) ? record.actors : []) note(Number(actor?.actorId), Infinity);
  const order = [...first.entries()]
    .filter(([id]) => !isUser?.(id))
    .sort((a, b) => (a[1] - b[1]) || (a[0] - b[0]));
  return new Map(order.map(([id], i) => [id, i + 1]));
};
