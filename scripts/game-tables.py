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
- i18n/dungeons/<lang>.json: dungeon names, for the languages already there.
- skill_groups.json: the id the game's Damage Analyzer reports a skill under.
- resource_restore_skills.json: skills that restore MP or another resource, never HP.
- open_world_maps.json: overworld maps and their world layers.

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

        path = DATA / "i18n/dungeons" / f"{lang}.json"
        if path.exists():
            table = load(path)
            for dungeon in dungeons:
                name = text.get(f"String_{dungeon['Title']['Key']}_body")
                if named(name):
                    table.setdefault(str(dungeon["ID"]["Value"]), {})["name"] = name
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

    overworld = {m["ID"]["Value"] for m in maps if m["MapType"] in ("EMapType::General", "EMapType::Starter")}
    open_world = overworld | {m["ID"]["Value"] for m in maps if value(m["BaseMapId"]) in overworld}
    (DATA / "open_world_maps.json").write_text(json.dumps({
        "source": f"{source}: Map table, overworld maps (MapType General/Starter) and their "
                  "world layers (BaseMapId is an overworld map)",
        "maps": sorted(open_world),
    }) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
