# Contributing

Read `docs/ARCHITECTURE.md` first. The section "Rules that must hold" lists faults that were found and fixed. Do not break those rules.

## Build and test

- Rust tests: `cd src-tauri && cargo test --lib`.
- Arch Linux package: `cd packaging/arch && makepkg -s`. Build in a clean environment (for example the `archlinux` container). A binary built with a CPU-specific toolchain can fail on other machines.
- The JavaScript has no tests. Check syntax with `node --check <file>`.

## Check a change against real fights

A unit test proves that the code does what the author thinks. A unit test does not prove that the game agrees. Every change to parsing, storage or calculation needs a check against a real fight.

1. Turn on Settings, Diagnostics, "Save raw packets".
2. Fight a training dummy alone. Note the clock time at the start and at the end.
3. Turn on the game's own meter (Ctrl+X) before the first hit. The game saves each result as a file, `record_<ticks>.dat`, in `AppData/Local/AION2/Saved_Steam/PersistentDownloadDir/DamageAnalyzer/<account>/` (on Linux inside the game's Proton prefix).
4. Run the replay report on the capture:

   ```
   A2_REPLAY_FILE=<capture> A2_REPLAY_TARGET=<target id> \
   A2_REPLAY_FROM=HH:MM:SS A2_REPLAY_TO=HH:MM:SS \
   cargo test --lib replay_report -- --ignored --nocapture
   ```

   Other variables: `A2_REPLAY_RESET_AT=HH:MM:SS` (press the reset button at that time), `A2_REPLAY_HITS=1` (print every change on the target).
5. Compare the replay with the game's record, skill by skill:

   ```
   A2_RECORD=<record_*.dat> A2_REPLAY_FILE=<capture> \
   cargo test --lib checks::record_check::record_check -- --exact --ignored --nocapture
   ```

   The game shows a DoT inside its skill and joins summon attacks of one name. The replay shows separate rows. The totals must match.

Do not commit captures. A capture contains chat and the names of other players.

## Commits

- One change per commit.
- A short subject in the imperative, about 60 characters or less.
- A body only when the reason is not clear from the change.

## Licence

GPL-3.0. See `LICENSE`. Parts are derived from AION2-DPS-Meter by TK-open-public under the MIT License. Keep both notices.
