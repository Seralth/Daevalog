// Class colours and class names shared by the front-end scripts.
const jobColorMap = {
  정령성: "#E06BFF",
  Spiritmaster: "#E06BFF",
  궁성: "#41D98A",
  Ranger: "#41D98A",
  살성: "#7BE35A",
  Assassin: "#7BE35A",
  수호성: "#5F8CFF",
  Templar: "#5F8CFF",
  마도성: "#9A6BFF",
  Sorcerer: "#9A6BFF",
  호법성: "#FF9A3D",
  Chanter: "#FF9A3D",
  치유성: "#F2C15A",
  Cleric: "#F2C15A",
  검성: "#4FD1C5",
  Gladiator: "#4FD1C5",
  권성: "#E85D5D",
  Brawler: "#E85D5D",
  Fighter: "#E85D5D",
};

// Map from the Korean class name stored in fight records → stable enum key used for i18n
const JOB_KEY_MAP = {
  "검성": "GLADIATOR",
  "수호성": "TEMPLAR",
  "궁성": "RANGER",
  "살성": "ASSASSIN",
  "마도성": "SORCERER",
  "치유성": "CLERIC",
  "정령성": "ELEMENTALIST",
  "호법성": "CHANTER",
  "권성": "FIGHTER",
};

// Class-filter icons that exist as assets (Korean class names). Guarding on
// this set avoids requesting a missing file on every render (which spammed
// "asset not found" for classes without an icon).
const CLASS_ICON_JOBS = new Set(["검성", "궁성", "마도성", "살성", "수호성", "정령성", "치유성", "호법성", "권성"]);
