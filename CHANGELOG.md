# Changelog

## 2.0 r3 (2026-10-25)

### Character window

- A new Character window, in the meter's menu under History. It shows your character as the game last showed it to the meter:
  - Gear: every item with its enhance level, manastones and stats, in its grade colour.
  - Character sheet: your stats, grouped and ordered like the game's stat window.
  - Skills: skill levels, Specialty picks and stigma tiers.
  - Daevanion: each board with the nodes you have taken. Hover a node to see what it gives.
- Play more than one character? Pick one by name, class, level and server.
- Your characters are saved as small files in the meter's data folder, under `characters/`, and never leave your computer. Item and stat names come from the game's own data in all eight languages; anything the data cannot name shows as `#` and its number.

### Meter

- A steadier time to kill. In dungeons it now looks at the whole fight, so stagger bursts, cooldowns and untargetable phases no longer make it jump around. If your group's damage changes for good, say someone leaves, it starts over from there. On world bosses, in the Abyss and at field events, where players keep arriving, it follows the last 30 seconds instead.
- Fit to rows now stays on. The meter's height grows and shrinks with its rows, so a player who joins or the healers under DPS are never cut off, and dragging an edge only changes the width. Turn it off in the meter's menu to set the height yourself; double-click an edge to turn it back on. It is on by default and remembered across restarts.
- The meter is less see-through by default: 72% opacity instead of 60%, so names stay readable on every class's bar over bright ground like snow. An opacity you set yourself stays as it is.
- Boss health works on the biggest bosses. Any boss over 100,000,000 HP showed 0 for most of the fight, dungeon bosses included.
- Killing a boss no longer opens an empty 0:00 fight on the dead boss.
- On KDE Plasma, Hyprland and Sway, the meter comes back after you unplug a monitor or switch to a text console. It used to stay gone. A hidden meter also stops redrawing.

### Notice tray

- Abyss Boss notices said "Field Boss" in r2. They now say "Abyss Boss", in red. Spacetime Rifts are purple now, and Gartua's and the commanders' countdowns have colours of their own.
- Countdowns for Eternal Gartua and the commanders, 30, 15 and 5 minutes before they appear. The times come from the world map, so open it once after you log in.
- A fourth notice no longer gets cut in half at the bottom of the meter. The tray shows up to three, and "N more" under them takes you to the next ones ("Back to top" returns). The mouse wheel moves one notice at a time, and the newest are always on the first page.
- The tray grows with the Meter size setting.
- Changing your server time zone moves the countdowns already in the tray right away, without a second notice for the same spawn.

### Details

- The timeline's stat tracks now get your full stat sheet. A full sheet has 132 stats, and the meter misread any sheet that long, so it never had a whole one.

## 2.0 r2 (2026-10-21)

### Meter

- A notice tray on the meter's edge. A thin bar shows an icon for each kind of notice waiting. The chevron pulls the card out: the notices, newest first, each with its own ✕, and Clear all. A new notice only adds its icon, so nothing opens over the game. Settings > Notifications > Notification tray puts it on the top or the bottom edge. Auto, the default, takes the top, or the bottom when the meter is too near the top of the screen for the open card.
- Pet soul notices in the tray. Each soul a pet takes shows how many souls it now holds toward its next level, and how many it needs to the top. Each pet has one line, updated with every soul, so farming does not flood the tray. Registering a pet or reaching a new level also shows the genus Insight it gave. The counts start from the pet list the game sends when you log in or come back from Select Character, and when you enter or leave a party dungeon or the Abyss. Before that, a pet's count shows as unknown.
- Field Boss and Spacetime Rift countdowns in the tray. A Field Boss shows 30, 15 and 5 minutes before it appears, with every boss of that time and its map. A Spacetime Rift shows 5 minutes before it opens. Each counts down to its time and leaves the tray 10 minutes after it. The times come from the game's own schedule, so a patch that moves them moves the countdowns too.
- A Notifications page in Settings: turn the countdowns off, and set your server's time zone. Pick your server region (in the game's own names), or type any time zone. Until you pick one, the meter uses your computer's time zone. Check one countdown against the server time shown in game.
- A Power Shard warning in the tray. When your hits used Power Shards and three casts in a row land without them, one notice says Power Shards are off, or that they ran out when your bag has none left. It goes away when your hits use shards again. If you never use shards, it never shows. Turn it off in Settings > Notifications.
- Meter size in Settings > Appearance: 100%, 115%, 130% or 150%. The meter's text, rows, header and footer grow together, so the meter keeps its shape and its text stays sharp. The other windows keep their size.
- In Slim, the boss health shows as a thin line without text. 2.0.0 showed no health bar in Slim.

### Linux desktop

- The Arch package installs layer-shell-qt, so on KDE Plasma, Hyprland and Sway the meter is the overlay above a fullscreen game without a second install. Without it, Hyprland and Sway tiled the meter across the screen.
- Each package needs at least the Qt version it was built with. A meter installed on an older Qt stopped at start.

## 2.0.0 (2026-10-16)

Daevalog 2.0 is a new meter. Its windows are drawn with Qt 6 instead of a web page, and the code behind them is new. On its first start, 2.0 copies your 1.x saved fights and the settings that still exist. Your a2tools.app sign-in does not come over, so sign in again.

Support for 1.x ends with 2.0.0. The legacy package `daevalog-dps-meter-legacy` (r607) is the last 1.x build: it gets no more updates, apart from fixes for security problems.

### Meter

- Heal and Tank tabs, and a split view with the healers in a second table under the damage rows.
- Pick the columns for each tab, as many as you like, in every mode. The meter widens to fit them.
- After the boss name: an enrage countdown, then the time to kill. After the time to kill: a green ✓ while the kill comes before the enrage, a red ✗ when not.
- Where the group damage shows is now its own setting: in the footer or on its own line.
- Low-share colouring is now a setting: off, on the number, or on the whole row.
- The sort button moved into the ⋯ menu as Lowest first.
- Fit to rows: double-click an edge of the meter, or pick it in the ⋯ menu.
- The reset button is now Save & Clear in the ⋯ menu. It clears the meter and saves the fight to History.
- The fight clock runs between hits while the fight is open. 1.x stopped it 3 seconds after the last hit.
- Pause replaces Suspend. It freezes the screen, fight time included, and keeps capturing. Suspend stopped reading the game, so anything in that time was lost.
- In BOSS, the boss fight stays on screen after a wipe and revive, until the next boss pull. 1.x cleared it when the zone changed.
- A boss fight ends when the game says the boss left combat. A wipe and the next pull are two fights, with their own fight time, DPS and saved fight, even when the next pull comes within seconds. 1.x joined them unless 30 seconds passed without a hit.
- The enrage countdown runs to the enrage time the game sends when the boss enters combat, instead of counting from the first hit.
- In TARGET, targets hit at the same moment break the tie the same way every time, so the meter no longer flips between them.
- The row limit is 5, 8, 10 or 12, 10 by default (1.x: 4 to 12, 6 by default).
- A number with nothing behind it shows "-" instead of 0. On world bosses the game sends only damage over time for other players, so their Max Hit, HIT and CRIT show "-", and a hover on them says why. Details does the same for hit rates, the lowest, average and highest hit, and a tab with no records; History for its CRIT, HIT and average columns. The Heal and Tank tabs, and History's Tank table, show "-" for a player with no heal or no attack received; a tank who blocked every attack keeps 0.

### Details

- Daevalog's own skill icons, in the skill table, the hit list and the timeline. 1.x loaded the game's icons from the internet.
- A TAKEN tab beside DMG and HEAL, and a mechanics view: which boss skill hit whom, how often and for how much.
- Click a skill to see its hits: normal and critical counts, the lowest, average and highest hit, and the hit types. A damage over time skill counts ticks instead of hits.
- The timeline has buff, debuff and stat tracks beside the skills. Debuffs on you have their own section.
- HPS and DTPS charts beside the DPS chart, and who each player healed.
- Share bars beside the targets. The DMG, HEAL, TAKEN and mechanics tables each sort by their column heads.

### History

- A saved fight shows the party DPS, your DPS and your place, and DPS, Heal and Tank tables.
- History marks a fight that is still going on as In progress, not Live.

### Look

- Four themes, Daevalog, Classic, Nord and System; all but System have four accent colours. They replace the ten 1.x themes.
- Bar strength replaces Meter bar opacity: how strongly the class colour fills each row, 10 to 50%, 40 by default.

### Linux desktop

- One meter at a time: a second start shows the running meter.
- With the tray icon on, the close button hides the meter to the tray, and Quit is in the ⋯ menu.

### Capture

- Game data that arrives twice or out of order is read once, in the right order.
- Network devices that come up or come back are read without stopping the others. A picked device is kept as a preference, and the meter follows the game when its connection moves.
- Big downloads by other programs no longer make the meter lose game packets.

### Reporting problems

- Prepare debug log saves a copy of `debug.log` with character names, your home folder and your user name taken out.

### Fixed

- Some critical hits counted the wrong number as their damage.
- The boss health bar and the time to kill work on bosses with more than 100 million health, such as the world bosses Watcher Kaira and Argo. 1.x showed their full health less the damage it saw. A boss with more than about 2.1 billion health shows no bar instead of a wrong one.

### Changed

- UI text is reworded in many places, in all eight languages of the global game client. Game terms use the game's own words. Chinese is no longer included.
- The licence is now the GNU Affero General Public License, version 3 only.

### Removed

- Send logs to a2tools.app.
- Target time range, which did nothing.
- The status line, the dungeon name and the rank number on the meter.
- The DPS / DMG / BOTH button in the footer.
- The Beta and Classic layouts. 2.0 has one layout, Normal or Slim.
- Panel background in Details.
- The tip links and the A2Tools footer in Settings.

### Not yet

- Windows: the build workflow makes a Windows installer (see [docs/windows.md](docs/windows.md)), but the meter on Windows is not yet checked in live play.
