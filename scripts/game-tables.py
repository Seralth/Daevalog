#!/usr/bin/env python3
"""Rebuild the meter's game data files from an export of the game client.

The export is the JSON that CUE4Parse writes for AION2/Content/Data/Table and
AION2/Content/L10N/Text (FModel layout). Run after each game patch:

    scripts/game-tables.py <export dir> <steam build id>

Writes, under src/data:
- i18n/npcs/<lang>.json: name, isBoss, isDummy for every NPC the game names.
  Fields the game data does not hold (category, tier, dungeonId) and NPCs it
  no longer has are kept as they are.
- i18n/skills/<lang>.json: the name of every skill the game names.
- i18n/dungeons/<lang>.json: for the languages already there, each dungeon's
  name, kind, difficulty and party tier, and the game's words for that
  difficulty. Instance maps without a dungeon row of their own get their map
  title as the name. zh-Hans and zh-Hant keep their names and get the English
  words.
- skill_groups.json: the id the game's Damage Analyzer reports a skill under.
- resource_restore_skills.json: skills that restore MP or another resource, never HP.
- open_world_maps.json: overworld maps, their world layers and the Abyss.
- solo_instance_maps.json: sealed and quest dungeons, whose map id is their
  dungeon id.

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


# Where each kind of dungeon finds the words for its difficulty. Other kinds
# have none in the game's strings (Matching: Easy to Hell) and get no label.
DIFFICULTY_TEXT = {
    "Party": "String_STR_DUNGEONDIFFICULTY_{}_body",
    "Raid": "String_UI_PARTYDUNGEON_RAID_DIFFICULTY_{}_body",
    "Awaken": "String_UI_AWAKEN_DIFFICULTY_{}_body",
}
TIER_TEXT = "String_UI_CONTENTS_UNLOCK_PARTYDUNGEON{}TIER_body"


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
    # The Abyss (Reshanta) is a large zone with sieges and world bosses, not
    # a dungeon a party enters.
    abyss = {value(d["MapId"]) for d in dungeons if enum(d["DungeonType"]) == "Abyss"}
    open_world = overworld | abyss | {m["ID"]["Value"] for m in maps if value(m["BaseMapId"]) in overworld}
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

    # The game's words for a dungeon's difficulty, and its party tier with it.
    def difficulty_label(dungeon, text):
        key = DIFFICULTY_TEXT.get(enum(dungeon["DungeonType"]))
        difficulty = enum(dungeon["DungeonDifficulty"])
        label = text.get(key.format(difficulty.upper())) if key and difficulty != "None" else None
        if not named(label):
            return None
        tier = re.fullmatch(r"PartyDungeon_(\d+)Tier", enum(dungeon["PartDungeonTier"]))
        tier_label = text.get(TIER_TEXT.format(tier.group(1))) if tier else None
        return f"{label} · {tier_label}" if named(tier_label) else label

    def set_dungeon(table, dungeon, text):
        entry = table.setdefault(str(dungeon["ID"]["Value"]), {})
        for field in ("type", "difficulty", "tier", "label"):
            entry.pop(field, None)
        entry.update(dungeon_fields(dungeon))
        label = difficulty_label(dungeon, text)
        if label:
            entry["label"] = label

    # A dummy players train on (by its English name) counts as a boss, so
    # BOSS mode shows it; test sandbags are dummies only.
    def dummy_kind(npc):
        name = english.get(f"String_{npc['Desc']['Key']}_body", "")
        if any(d in name for d in DUMMY_NAMES):
            return "player"
        return "test" if DUMMY_INTERNAL.search(npc["Name"]) else None

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
        save(path, table)

        # Every id a fight can be filed under gets a name: a dungeon's title,
        # or its map's where it has none, and the title of an instance map
        # without a dungeon row of its own.
        path = DATA / "i18n/dungeons" / f"{lang}.json"
        if path.exists():
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
        "source": f"{source}: Map table, overworld maps (MapType General/Starter), their "
                  "world layers (BaseMapId is an overworld map) and the Abyss (Dungeon table, "
                  "DungeonType Abyss)",
        "maps": sorted(open_world),
    }) + "\n", encoding="utf-8")

    solo = [d["ID"]["Value"] for d in dungeons
            if enum(d["DungeonType"]) in ("Seal", "Quest") and value(d["MapId"]) == d["ID"]["Value"]
            and d["ID"]["Value"] in map_by_id and d["ID"]["Value"] not in open_world]
    (DATA / "solo_instance_maps.json").write_text(json.dumps({
        "source": f"{source}: Dungeon table, sealed and quest dungeons (DungeonType Seal/Quest) "
                  "on a map of the same id",
        "maps": sorted(solo),
    }) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
