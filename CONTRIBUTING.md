# Contributing

Read `docs/ARCHITECTURE.md` first. The section "Rules that must hold" lists faults that were found and fixed. Do not break those rules.

## Build and test

- Rust tests: `cd src-tauri && cargo test --lib`.
- Arch Linux package: `cd packaging/qml/arch && makepkg -s`. Build in a clean environment (for example the `archlinux` container). A binary built with a CPU-specific toolchain can fail on other machines.
- Front end tests: `cd src-qml && cargo test`. They need Qt 6. `QT_QPA_PLATFORM=offscreen` runs them without a display.

## Check a change against real fights

A unit test proves that the code does what the author thinks. A unit test does not prove that the game agrees. Every change to parsing, storage or calculation needs a check against a real fight.

1. Turn on Settings, Diagnostics, "Enable packet logging".
2. Fight a training dummy alone. Note the clock time at the start and at the end.
3. Turn on the game's own meter (Ctrl+X) before the first hit. The game saves each result as a file, `record_<ticks>.dat`, in `AppData/Local/AION2/Saved_Steam/PersistentDownloadDir/DamageAnalyzer/<account>/` (on Linux inside the game's Proton prefix).
4. Run the replay report on the capture:

   ```
   A2_REPLAY_FILE=<capture> A2_REPLAY_TARGET=<target id> \
   A2_REPLAY_FROM=HH:MM:SS A2_REPLAY_TO=HH:MM:SS \
   cargo test --lib replay_report -- --ignored --nocapture
   ```

   Other variables: `A2_REPLAY_RESET_AT=HH:MM:SS` (press the reset button at that time), `A2_REPLAY_HITS=1` (print every change on the target), `A2_REPLAY_TAKEN=1` (print the damage players took, per player and skill).

   `A2_REPLAY_FLAGS=1` prints every hit as a `hit_flags` line: capture ms, actor, target, skill, `damage`, hit type, layout, the flags and angle bytes, `multi` (additional hits), `multi_dmg` (their damage, which `damage` leaves out), the restored HP and `scalar`. The scalar is the actor's combat speed, 10000 plus the CombatSpeed stat (282); it is `-` when the record has none. `A2_REPLAY_TIMELINE=1` prints each fight of the local player as JSON: the hits with these fields, the DoT ticks (`dots`: the ticks the game's records count, so none of a spirit that has left; from the window's start when `A2_REPLAY_FROM` is given, else from 15 s before the first hit, never the fight before's on that target; up to 15 s after the last hit), the buffs and the stats.
5. Compare the replay with the game's record, skill by skill:

   ```
   A2_RECORD=<record_*.dat> A2_REPLAY_FILE=<capture> \
   cargo test --lib checks::record_check::record_check -- --exact --ignored --nocapture
   ```

   The game shows a DoT inside its skill and joins summon attacks of one name. The replay shows separate rows. The totals must match. After the rows, the check sets the record's damage taken beside the meter's, over the record's own window; every number must match.

Do not commit captures. A capture contains chat and the names of other players.

## Commits

- One change per commit.
- A short subject in the imperative, about 60 characters or less.
- A body only when the reason is not clear from the change.

## What you submit

Code is code, however it was written: by hand, with an AI tool, or both. Every change is judged the same way:

- **It works.**
- **The proof is factual and repeatable:** test results, replays of real fights, before and after numbers that anyone can reproduce.
- **It breaks nothing else.**

A tool cannot act on its own. Whoever starts, prompts or runs it answers for what it produces. Everything you submit, code, pull request descriptions, replies and logs, is treated as coming from you, and you are fully responsible for it. How you use your tools is up to you, but you must either be able to explain the change yourself or give proof that stands on its own.

Changes that are neither explained nor proven will be closed. Every pull request is also run against our private replay check, made from real fight captures that contain other players' names, before it can merge; changes that alter results must say why. The maintainer's own work meets the same bar.

Code copied from elsewhere, by you or by a tool, is your responsibility.

## Licence

AGPL-3.0-only. See `LICENSE`, which also lists the parts under other licences. New files in `src-qml/` start with `SPDX-License-Identifier: AGPL-3.0-only`.

By opening a pull request you agree that your contribution is licensed under AGPL-3.0-only, the same licence as the rest of the project. You keep the copyright in your work. Do not submit code you did not write unless its licence allows it to be released under AGPL-3.0-only, and say where it came from.
