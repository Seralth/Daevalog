#!/usr/bin/env python3
"""Rebuild the meter's game data files from an export of the game client.

The export is the JSON that CUE4Parse writes for AION2/Content/Data/Table and
AION2/Content/L10N/Text (FModel layout). Run after each game patch:

    scripts/game-tables.py <export dir> <steam build id>

Writes, under src/data:
- i18n/npcs/<lang>.json: name, isBoss, isDummy for every NPC the game names.
  Fields the game data does not hold (category, tier, dungeonId) and NPCs it
  no longer has are kept as they are.
- i18n/skills/<lang>.json: the name of every skill the game names, and of
  the heals and damage over time that come from no skill (NO_SKILL_EFFECTS).
- i18n/dungeons/<lang>.json: each dungeon's name, kind, difficulty and party
  tier, and the game's words for that difficulty. Instance maps without a
  dungeon row of their own get their map title as the name. zh-Hans and
  zh-Hant keep their names and get the English words.
- skill_groups.json: the id the game's Damage Analyzer reports a skill under.
- resource_restore_skills.json: skills that restore MP or another resource, never HP.
- open_world_maps.json: overworld maps and their world layers (not the Daeva
  Hunter recon sites).
- instance_maps.json: the dungeon of each instance map a fight is filed under
  without a party roster (sealed, quest, daily, Ascension and Ascension Trial
  dungeons, Nightmare, the Abyss and the Daeva Hunter recon sites), and the
  map of each dungeon whose id is not its map's.

zh-Hans and zh-Hant are not in the global client and are left alone.
"""

import json
import re
import sys
from pathlib import Path

LANGS = {"de-DE": "de", "en-US": "en", "es-ES": "es", "fr-FR": "fr",
         "ja-JP": "ja", "ko-KR": "ko", "pt-BR": "pt", "ru-RU": "ru"}

# Training dummies: English names, and the game's internal names for them.
DUMMY_NAMES = ("Training Scarecrow", "Punching Bag")
DUMMY_INTERNAL = re.compile(r"TraDummy|Sandbag", re.IGNORECASE)

# A heal or damage tick carries the id of the abnormal effect behind it, and
# the meter files it under that id // 100: the skill that owns the abnormal.
# These abnormals belong to no skill, so their code takes the abnormal's name.
# 19000013 is Restore HP, a full heal (seen in the captures of 2026-10-04/05).
# 12000101 to 12000181 are Poison, Bleed and Burn, shared by many skills,
# nearly all of them monsters' (on players in the captures of 2026-10-04/05).
NO_SKILL_EFFECTS = {
    1900001: 19000013,
    1200010: 12000101, 1200011: 12000111, 1200012: 12000121,
    1200014: 12000141, 1200015: 12000151, 1200016: 12000161,
    1200018: 12000181,
}

DATA = Path(__file__).resolve().parent.parent / "src" / "data"


def rows(export, table):
    path = export / "AION2/Content/Data/Table" / f"{table}.json"
    return json.loads(path.read_text(encoding="utf-8"))["Properties"]["Data"]


def strings(export, culture):
    path = export / "AION2/Content/L10N/Text" / culture / "L10NString.json"
    return json.loads(path.read_text(encoding="utf-8"))["Entries"]


def value(v):
    return v.get("Value") if isinstance(v, dict) else v


def enum(v):
    return v.split("::")[-1]


# Where each kind of dungeon finds the words for its difficulty, by its
# sub-type or else its type. Other kinds have no difficulty.
DIFFICULTY_TEXT = {
    "Party": "String_STR_DUNGEONDIFFICULTY_{}_body",
    "Raid": "String_UI_PARTYDUNGEON_RAID_DIFFICULTY_{}_body",
    "Awaken": "String_UI_AWAKEN_DIFFICULTY_{}_body",
    # Subjugation, the Matching rows with Easy to Hell.
    "Suppression": "String_UI_SUPPRESSION_{}_body",
}
TIER_TEXT = "String_UI_CONTENTS_UNLOCK_PARTYDUNGEON{}TIER_body"
# Instances a fight is filed under by their map: sealed, quest, daily,
# Ascension and Ascension Trial (Awaken) dungeons, Nightmare, and the Abyss
# (Reshanta, entered from its menu for a timed stay).
OWN_MAP_TYPES = ("Seal", "Quest", "Daily", "Ascension", "Awaken", "BossChallenge", "Abyss")
# Daeva Hunter recon sites ("Watcher Krache's Recon Site"): boss arenas from
# Duty quest scrolls. The table types them InstanceLayer on an overworld
# base map, but each is an instance of its own, left for home (ExitType Home,
# which no other layer has).
OWN_MAP_PREFIX = "DaevaHunter_"
CONQUEST_HARD_TEXT = "String_UI_PARTYDUNGEON_CONQUER_DIFFICULTY_ADVANCED_body"
# A name that holds its variant in brackets: "Nightmare Altar (Easy)".
VARIANT_IN_NAME = re.compile(r"[(\[（【].*[)\]）】]")
WORD = re.compile(r"\w+")


# The words of a name or label, less short Latin ones ("de" in "Grotte de
# Krao" and "Conquête de rang 1" says nothing twice).
def words(text):
    return {w for w in WORD.findall(text.casefold()) if len(w) > 2 or not w.isascii()}


def load(path):
    return json.loads(path.read_text(encoding="utf-8")) if path.exists() else {}


def save(path, data):
    text = json.dumps(dict(sorted(data.items())), ensure_ascii=False, indent=2)
    path.write_text(text + "\n", encoding="utf-8")


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    export, build = Path(sys.argv[1]), sys.argv[2]
    source = f"Game data, Steam build {build}"

    npcs = rows(export, "NpcData")
    skills = rows(export, "Skill")
    dungeons = rows(export, "Dungeon")
    maps = rows(export, "Map")
    abnormals = {a["ID"]["Value"]: a for a in rows(export, "SkillAbnormal")}
    english = strings(export, "en-US")

    # A boss is a HeroMonster that can be attacked; the unattackable ones are
    # mostly mechanics (invisible walls and floor checks). Some of those take
    # damage all the same (Ultimate Berk's skill entity, Afflicted Bakarma), so
    # a boss flag already in the table is never removed.
    def is_boss(npc):
        return npc["NpcSubType"].endswith("::HeroMonster") and npc["bCanBeAttacked"]

    def named(name):
        return name and name != "???"

    overworld = {m["ID"]["Value"] for m in maps if m["MapType"] in ("EMapType::General", "EMapType::Starter")}
    recon_sites = {d["ID"]["Value"] for d in dungeons if d["Name"].startswith(OWN_MAP_PREFIX)}
    open_world = (overworld | {m["ID"]["Value"] for m in maps if value(m["BaseMapId"]) in overworld}) - recon_sites
    map_by_id = {m["ID"]["Value"]: m for m in maps}
    dungeon_ids = {d["ID"]["Value"] for d in dungeons}

    # A dungeon's kind, difficulty and party tier, as the game's own fields
    # say; "None" leaves a field out.
    def dungeon_fields(dungeon):
        fields = {"type": enum(dungeon["DungeonType"]).lower()}
        difficulty = enum(dungeon["DungeonDifficulty"])
        if difficulty != "None":
            fields["difficulty"] = difficulty.lower()
        tier = re.fullmatch(r"PartyDungeon_(\d+)Tier", enum(dungeon["PartDungeonTier"]))
        if tier:
            fields["tier"] = int(tier.group(1))
        return fields

    # The game's words for a dungeon's difficulty, never repeating a word.
    # A party tier's words ("Conquest Tier 3") hold the difficulty already,
    # so they stand alone, with the game's Conquest "Hard" for an Advanced
    # row. A name that holds its variant gets no words.
    #
    # The Expedition menu's Conquest tab lists Krao Cave, Draupnir, Urugugu
    # Canyon, Vakron Sky Island, Fire Temple and Ferocious Horn Den with 1, 1,
    # 2, 2, 3 and 3 stars: their "_Hard" rows, difficulty None and that party
    # tier (in-game, 2026-10-06). So a tier alone is enough for the words.
    def difficulty_label(dungeon, name, text):
        difficulty = enum(dungeon["DungeonDifficulty"])
        tier = re.fullmatch(r"PartyDungeon_(\d+)Tier", enum(dungeon["PartDungeonTier"]))
        if (difficulty == "None" and not tier) or not name or VARIANT_IN_NAME.search(name):
            return None
        if tier:
            label = text.get(TIER_TEXT.format(tier.group(1)))
            hard = text.get(CONQUEST_HARD_TEXT)
            if difficulty == "Advanced" and named(label) and named(hard):
                label = f"{label} · {hard}"
        else:
            key = DIFFICULTY_TEXT.get(enum(dungeon["DungeonSubType"])) or DIFFICULTY_TEXT.get(enum(dungeon["DungeonType"]))
            label = text.get(key.format(difficulty.upper())) if key else None
        if not named(label) or words(label) & words(name):
            return None
        return label

    def set_dungeon(table, dungeon, text):
        entry = table.setdefault(str(dungeon["ID"]["Value"]), {})
        for field in ("type", "difficulty", "tier", "label"):
            entry.pop(field, None)
        entry.update(dungeon_fields(dungeon))
        label = difficulty_label(dungeon, entry.get("name"), text)
        if label:
            entry["label"] = label

    # A dummy players train on (by its English name) counts as a boss, so
    # BOSS mode shows it; test sandbags are dummies only.
    def dummy_kind(npc):
        name = english.get(f"String_{npc['Desc']['Key']}_body", "")
        if any(d in name for d in DUMMY_NAMES):
            return "player"
        return "test" if DUMMY_INTERNAL.search(npc["Name"]) else None

    english_dungeons = load(DATA / "i18n/dungeons/en.json")
    for culture, lang in LANGS.items():
        text = english if lang == "en" else strings(export, culture)

        path = DATA / "i18n/npcs" / f"{lang}.json"
        table = load(path)
        for npc in npcs:
            name = text.get(f"String_{npc['Desc']['Key']}_body")
            if not named(name):
                continue
            key = str(npc["ID"]["Value"])
            dummy = dummy_kind(npc)
            if key in table:
                entry = table[key]
                entry["isBoss"] = entry.get("isBoss", False) or is_boss(npc)
            else:
                entry = table[key] = {"isBoss": is_boss(npc) or dummy == "player"}
            entry["name"] = name
            if dummy:
                entry["isDummy"] = True
            table[key] = {"name": entry.pop("name"), **entry}
        save(path, table)

        # Many enemy skills share one generic name ("Attack", key
        # SkillString_NPC_Attack); a better name already in the table stays.
        path = DATA / "i18n/skills" / f"{lang}.json"
        table = load(path)
        for skill in skills:
            name = text.get(f"SkillString_{skill['SkillString_Key']}_skill_name")
            key = str(skill["ID"]["Value"])
            generic = skill["SkillString_Key"] == "SkillString_NPC_Attack"
            if named(name) and not (generic and key in table):
                table[key] = name
        skill_ids = {skill["ID"]["Value"] for skill in skills}
        for code, abnormal in NO_SKILL_EFFECTS.items():
            name = text.get(f"SkillAbnormalString_{abnormals[abnormal]['SkillAbnormalString_Key']}_desc_name")
            if named(name) and code not in skill_ids:
                table[str(code)] = name
        save(path, table)

        # Every id a fight can be filed under gets a name: a dungeon's title,
        # or its map's where it has none, and the title of an instance map
        # without a dungeon row of its own.
        path = DATA / "i18n/dungeons" / f"{lang}.json"
        table = load(path)
        for dungeon in dungeons:
            name = text.get(f"String_{dungeon['Title']['Key']}_body")
            own_map = map_by_id.get(dungeon["ID"]["Value"])
            if not named(name) and own_map and value(dungeon["MapId"]) == dungeon["ID"]["Value"]:
                name = text.get(f"String_{own_map['Desc']['Key']}_body")
            if named(name):
                table.setdefault(str(dungeon["ID"]["Value"]), {})["name"] = name
            if str(dungeon["ID"]["Value"]) in table:
                set_dungeon(table, dungeon, text)
        for m in maps:
            name = text.get(f"String_{m['Desc']['Key']}_body")
            if m["ID"]["Value"] not in open_world | dungeon_ids and named(name):
                table.setdefault(str(m["ID"]["Value"]), {})["name"] = name
        # Ids the game no longer has, or has no name for here, keep the English.
        for key, entry in english_dungeons.items():
            table.setdefault(key, dict(entry))
        save(path, table)

    groups = {str(s["ID"]["Value"]): value(s["DamageAnalyzerSkillIdOverride"]) for s in skills
              if value(s["DamageAnalyzerSkillIdOverride"]) not in (0, None, s["ID"]["Value"])}
    (DATA / "skill_groups.json").write_text(json.dumps({
        "source": f"{source}: Skill table DamageAnalyzerSkillIdOverride, the id the game's "
                  "Damage Analyzer reports a skill under (only ids that differ)",
        "groups": dict(sorted(groups.items(), key=lambda kv: int(kv[0]))),
    }, separators=(",", ":")) + "\n", encoding="utf-8")

    # Skills that restore only MP (or another resource), never HP. Their
    # records look like heals (a Water Spirit's attack sends 20 MP to its
    # Spiritmaster), so the meter must not count them as healing.
    effects = {}
    for e in rows(export, "SkillEffect"):
        effects.setdefault(value(e["SkillEffectGroupId"]), set()).add(e["EffectType"])
    not_hp = re.compile(r"ESkillEffectType::(Mp|Sp|Dp|Op|Fp|AP)Heal")
    restores = []
    for s in skills:
        groups_of = [value(t["SkillEffectGroupId"]) for t in s["SkillEffectTimeDataList"]]
        types = set().union(*(effects.get(g, set()) for g in groups_of if g))
        if types and all(not_hp.match(t) for t in types):
            restores.append(s["ID"]["Value"])
    (DATA / "resource_restore_skills.json").write_text(json.dumps({
        "source": f"{source}: Skill and SkillEffect tables, skills whose every effect restores "
                  "MP, SP, DP, OP, FP or AP (none restores HP)",
        "skills": sorted(restores),
    }) + "\n", encoding="utf-8")

    # Not in the global client: their names stay, the rest is the English.
    for lang in ("zh-Hans", "zh-Hant"):
        path = DATA / "i18n/dungeons" / f"{lang}.json"
        if path.exists():
            table = load(path)
            for dungeon in dungeons:
                if str(dungeon["ID"]["Value"]) in table:
                    set_dungeon(table, dungeon, english)
            save(path, table)

    (DATA / "open_world_maps.json").write_text(json.dumps({
        "source": f"{source}: Map table, overworld maps (MapType General/Starter) and their "
                  "world layers (BaseMapId is an overworld map), less the Daeva Hunter recon sites",
        "maps": sorted(open_world),
    }) + "\n", encoding="utf-8")

    # These are entered without a party roster to name them: the dungeon row
    # on the map is the content. Most share the map's id; the Abyss rows do
    # not (Chaotic Lower Reshanta is dungeon 21 on map 20).
    own = {}
    for d in dungeons:
        if ((enum(d["DungeonType"]) in OWN_MAP_TYPES or d["Name"].startswith(OWN_MAP_PREFIX))
                and value(d["MapId"]) in map_by_id and value(d["MapId"]) not in open_world):
            if own.setdefault(str(value(d["MapId"])), d["ID"]["Value"]) != d["ID"]["Value"]:
                sys.exit(f"map {value(d['MapId'])} holds two dungeons a fight could be filed under")
    # A party's queue applies at the load into its dungeon's map.
    dungeon_maps = {str(d["ID"]["Value"]): value(d["MapId"]) for d in dungeons
                    if value(d["MapId"]) != d["ID"]["Value"]}
    (DATA / "instance_maps.json").write_text(json.dumps({
        "source": f"{source}: Dungeon table. ownMaps: the dungeon on each map of DungeonType "
                  f"{', '.join(OWN_MAP_TYPES)}, or of the {OWN_MAP_PREFIX}* recon sites. "
                  "dungeonMaps: the MapId of each dungeon whose id differs",
        "ownMaps": dict(sorted(own.items(), key=lambda kv: int(kv[0]))),
        "dungeonMaps": dict(sorted(dungeon_maps.items(), key=lambda kv: int(kv[0]))),
    }) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
