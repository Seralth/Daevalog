# How a summon is attributed to its summoner

A summon (pet, totem, aura, spell-effect entity) deals damage under its **own**
entity id. To show that damage on the owner's row, the meter has to answer
"which player does entity N belong to?" — and the answer is not in the damage
record. This is how we get it, in the order the code tries.

Everything below was derived from, and measured against, two live captures:

| capture | why it matters |
|---|---|
| `packets_20260815_183732.txt` | 10 min, 3.8 MB, 172 spawn packets → **81 summon/owner pairs with a ground-truth answer** |
| `packets_20260818_112931.txt` | 40 s, 67 KB, **zero** spawn packets → the hard case |

Entity ids are session-scoped and are reissued on every zone change. "Who is
entity N" is only a question with an answer *within one zone*.

---

## 1. `parent_key` in the spawn packet — the authoritative source

The `41 36` spawn record declares its parent behind a mask bit:

```
41 36
  id          varint
  mask        u16
  <subtree>   ×3        variable length, must be walked
  [mask&0x0001] u8
  [mask&0x0002] u8
  [mask&0x0004] u8
  [mask&0x0008] bit
  [mask&0x0010] u32      parent_key  <- the owner's entity id
```

The low byte of `mask` doubles as the entity kind:

| kind | meaning | parent_key is… |
|---|---|---|
| `0x0C` / `0x0D` | NPC | absent (bit 4 clear) |
| `0x5F` | summon / pet | **the owner** |
| `0x1C` | transient skill-effect entity | the skill's **target**, *not* the caster — never treat as owner |

The three subtrees are impractical to walk, so `find_spawn_parent_key` anchors on
the owner block that immediately follows the field instead:

```
<parent_key u32> <legion_id u32> <u16 = 0> <u16 server_id> <len u8> <utf8 legion name>
```

Six constraints have to hold at once (parent in range, parent != self, the u16
pad is zero, a plausible world id, a length that fits, decodable UTF-8). That
pinned the owner on **81 of 81** summons with no false positives, including a
Spiritmaster's 54 pets whose owner had not been named at the time they spawned.

**Code:** `capture/stream_processor/spawn.rs` → `parse_summon_spawn_at`,
`find_spawn_parent_key`, `parse_spawn_owner_block`.

**Known gap:** the anchor is the owner's *legion* record. A summoner with **no
legion** would zero that block and fail validation. Every summoner in the
reference capture was in a legion, so this is untested — if summons stop linking
for one particular player, suspect this first.

---

## 2. The spawn's inline name

`41 36` sets bit 0 of the first subtree's mask byte when it carries a name
string, and for a summon **that name is the owner's**, not the summon's. If the
name resolves to a known entity, that is the owner.

A `0x1C` skill-effect entity is a monster's. Its name, when present, is a
player's, nearly always the player it hits. The end of its spawn record names
the monster, and its hits on players count as damage taken from that monster
(ARCHITECTURE.md, Parsing). In the check kit's 14 captures (2026-10-07),
1,019 of 1,020 `0x1C` spawns carried an NPC code, every one listed in the NPC
table as no player's summon; the other carried neither code nor name. 117
carried the name of a player the meter knew. 36 of them hit a player, 35 of
those the named one (the other, a Gas Rock, hit another player). The NPC code
check below keeps every one of them from linking to the named player. A `0x1C` spawn
uses its name only when its code is on the game data's list of player summons,
so an effect whose code no table lists yet (a monster new since the last data
update) links to no one.

**Code:** `parse_summon_spawn_at`, the `spawn_name` fallback.

A monster's summon carries a name too: the player it targets. A Blazing Totem
(NPC 2920063) named the player it burned, and its Burn ticks on that player
counted as the player's healing. So no spawn path links a spawn whose own NPC
code the NPC table names but not as a player's summon (NpcData
`RelationshipEntity` other than `PC_Summon`; the player summons are listed in
`src/data/player_summon_npcs.json`). Only the code in that spawn record counts:
an entity id keeps the code of the last entity under it, and other players'
spirits spawn with no code at all. In the check kit's captures this dropped 63
links (Blazing Totems, Kromede's Desire, Gas Rocks, Zumion's Energy and three
more) and kept every other one (2026-10-06).

---

## 3. Friendly-effect target

A summon that fires a *friendly* effect fires it at its owner. Take every
`04 38` / `05 38` record where the summon is the source and the target is a
player: the most-targeted player is the owner.

Measured on ground truth: **60 correct, 0 wrong**, 21 summons never fire one.

Not currently implemented — it is the highest-precision idea still on the table
and needs no spawn packet. Worth adding if attribution gaps show up.

---

## 4. Power scalar — for summons with no spawn packet at all

Some summons are **never announced**. A Cleric's Divine Aura is created, ticks
for ~8 seconds, and expires with no `41 36` record anywhere in the stream —
verified exhaustively on the Aug-18 capture, where all four aura entities appear
*only* as `04 38` `sourceID` fields, with logging running six seconds before the
first one was cast. Mechanisms 1–3 cannot fire, by construction.

Every damage record carries a **power scalar**, and a summon inherits its
owner's:

```
04 38
  targetID   varint
  mask       u16
  sourceID   varint
  skillCode  u32
  …mask-gated detail fields…
  <8 bytes>            per-hit id
  power_scalar varint  <- the actor's damage multiplier, 1/100 %
  damage       varint
  01 00
```

Mobs read `10000` (= 100.00%); geared players run 16000–22000. It moves with
buffs, so compare against the **set** of values an actor has been seen with, not
a single reading.

> This field was already being parsed — the old comment called it "a fixed marker
> field (e.g. `E6 6F` = 14310)". It is not fixed and not a marker.

Ground truth: a summon's scalar matched a value its owner also showed in
**81/81** cases (69 exactly; the other 12 at exactly owner − 1000, a buff state
the owner is separately observed in).

The scalar alone is **not** sufficient — it is a stat, so two players can share
one. A rule built on it needs all of:

- same class (skill-code prefix), and
- scalar sets intersect, and
- the candidate shows **≥3× as many distinct skills** — a player runs a rotation,
  a summon spams one or two abilities, and
- exactly one candidate survives.

Measured with that rule: **43 correct, 0 wrong**, 38 undetermined on the Aug-15
pairs, and **zero** false merges of a real player into another. On Aug-18 it
attributes all four Divine Auras to the (unnamed) Cleric.

**Code:** none. The meter does not merge by power scalar, and by class only as
section 5 says: both are shared between players, and such guesses put mobs,
party members and the player into other rows.

---

## 5. No link at all

An actor that deals damage with class skills, has no link, and shows no sign of
a player is a summon or effect. Signs of a player: a name, any `45 36` record
(with or without a name in it), being you, owning a linked summon, or damage
with 5 or more distinct class skills. In the check kit's 11 captures
(2026-10-06), none of 1,022 linked summons and effects had a `45 36` record or
more than 4 distinct skills; 82 unnamed actors with no `45 36` record used 5 to
19, most of them players in a world boss crowd.

Such an actor goes to the party's one member of its class when the actor used
skills of one class only, the party has exactly one member of it, that member is
named, and the players of that class who fought the targets the actor hit are
that member alone (`entities::party_owner`). Checked against every linked
summon in the captures: 105 right, 0 wrong. Without the check on who fought
there, 114 strangers' summons would have gone to the party member.

Everything else shares one row, `UNATTRIBUTED_ID`, labelled "Unattributed
summons and effects". A saved fight keeps its damage in the details but not in
`actors`, so uploads and History do not count it as a player. A link that
arrives later still takes all the actor did (`combat/data_storage/entities.rs`,
`owners`).

---

## Regression tests

`src-tauri/tests/capture_replay.rs`, skipped unless the captures are supplied:

```
A2_REPLAY_CAPTURE=…/packets_20260815_183732.txt \
cargo test --test capture_replay -- --nocapture
```

- `resolves_party_identities_and_summon_owners` — all 5 party members identified,
  ≥40 pets on the Spiritmaster, ≥15 spell entities on the Sorcerer.
- `meter_rows_are_all_named_with_combat_power` — exactly 5 rows, none unnamed.
