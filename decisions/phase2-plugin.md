# Phase 2 — plugin (`oliver.monero`)

Agent: Phase 2 plugin. Branch `phase2/plugin`, based on `gate-1b`, worked in the **main checkout**
(DESIGN.md §8: the Omarchy symlink must resolve to this checkout, and a worktree's QML edits would
be invisible to the running shell). Owned files: `plugin/oliver.monero/**` except `README.md`,
`tests/plugin-smoke.sh`, and this file. Nothing else was touched.

Date: 2026-09-17. Machine: Omarchy 4.0.3, Hyprland, Quickshell (`omarchy-shell`), theme
`colors.toml` is a greyscale one (`red = #a4a4a4`), which matters for one observation below.

---

## 1. Line counts against the §6.1 budgets

Measured with `wc -l` on the committed files, after the last edit:

| File | Budget (§6.1) | Real | |
|---|---|---|---|
| `manifest.json` | ~55 | **32** | under |
| `Service.qml` | ≤ 380 | **287** | under |
| `Panel.qml` | ≤ 700 | **700** | at budget |
| `MoneroIcon.qml` | ≤ 80 | **80** | at budget |
| `Model.js` | ≤ 150 | **142** | under |
| **Total QML + JS** | **≤ 1 310** | **1 209** | under by 101 |
| Total including `manifest.json` | — | 1 241 | |

```
$ wc -l plugin/oliver.monero/*.qml plugin/oliver.monero/*.js plugin/oliver.monero/manifest.json
   80 plugin/oliver.monero/MoneroIcon.qml
  701 plugin/oliver.monero/Panel.qml     <- trimmed to 700 in the following commit
  287 plugin/oliver.monero/Service.qml
  142 plugin/oliver.monero/Model.js
   32 plugin/oliver.monero/manifest.json
 1242 total

$ cat plugin/oliver.monero/*.qml plugin/oliver.monero/*.js | wc -l
1209
```

**§6.1's own arithmetic does not close, and the smaller number was taken as binding.** The per-file
budgets sum to 55 + 380 + 700 + 80 + 150 = **1 365**, while the sentence under the table says
"Total QML under **1 310** lines, the sum of the per-file budgets above". Both are satisfied here
(1 209 of QML + JS), and every per-file budget is met individually, so nothing had to be decided —
it is recorded because a later agent adding one screen to the panel will hit the per-file cap first
and should know the two numbers disagree by 55.

---

## 2. Gate transcript

### 2.1 `omarchy-plugin-validate` on the repository directory

Validated the repo copy, never the installed link — the validator refuses any symlink inside a
plugin folder (§1.5).

```
$ omarchy-plugin-validate plugin/oliver.monero
omarchy-plugin-validate exit=0
```

### 2.2 `tests/plugin-smoke.sh` — full run, with the plugin linked and enabled on this machine

```
$ ./tests/plugin-smoke.sh
plugin-smoke: /home/oliver/Projects/odcpw/fmonero/plugin/oliver.monero
  ok    omarchy-plugin-validate
  ok    manifest id is oliver.monero
  ok    schemaVersion is 1
  ok    kinds contains bar-widget
  ok    entryPoints.barWidget = Panel.qml
  ok    defaultSection = right
  ok    allowMultiple = false
  ok    manifest.json exists
  ok    Panel.qml exists
  ok    Service.qml exists
  ok    MoneroIcon.qml exists
  ok    Model.js exists
  skip  README.md is not here yet (owned by the docs agent, DESIGN.md §6.1)
  ok    qmllint: no syntax errors
  ok    Panel.qml sets manageIpc: false
  ok    no execDetached(["bash" …])
  ok    self-test: execDetached bash fires on its own fixture
  ok    no bar.run()
  ok    self-test: bar.run fires on its own fixture
  ok    every manifest default has an in-code fallback
  ok    no money argv element
  ok    self-test: money argv fires on its own fixture
  ok    self-test: money argv leaves its negative fixture alone
  ok    no wallet secret named in QML
  ok    self-test: secret name fires on its own fixture
  ok    no float arithmetic on money
  ok    self-test: parseFloat fires on its own fixture
  ok    self-test: Number() fires on its own fixture
  ok    self-test: pico / 1e12 fires on its own fixture
  ok    rescanPlugins
  ok    journal carries no load failure for oliver.monero
  ok    widget is loaded (cursor: {"section":"header","index":0,"active":false,"opened":false,
        "sections":["header","footer"],"alive":false,"binaryMissing":true,"nodeUp":false,
        "synced":false,"walletOpen":false,"iconCase":0,"iconColor":"#a5a5a5"})

plugin-smoke: 31 passed, 0 failed
exit=0
```

Two lines in that transcript deserve reading rather than skimming:

- **`skip README.md`.** The plugin's README is the docs agent's file (§6.1) and lands on
  `phase2/docs`. It is not loaded by the shell, so the script reports its absence instead of
  failing on a file this agent may not create (§2k). After the Phase 3 merge it will read `ok`.
- **`binaryMissing: true`, `iconCase: 0`.** No `fmonero` is installed at `~/.local/bin` on this
  machine — `scripts/install.sh` is the docs agent's file and §4b reserves the one real install for
  the Phase 4 verifier. That is the widget's honest empty state: the bar glyph is dimmed, crossed
  and badged, and the panel's first line is
  `fmonero is not installed or not on PATH (~/.local/bin).` It was verified deliberately, and so
  was the opposite state (§2.4 below).

### 2.3 The two greps and their fixtures

Both greps of §6.7 pass over the plugin **and** fire on a fixture that contains the forbidden
thing. `tests/plugin-smoke.sh` carries the harness (`forbid` / `fires` / `silent`), mirroring
`scripts/check.sh`'s rule that a grep which cannot fail is worse than no grep.

| Grep | Pattern | Positive fixture (must fire) | Negative fixture (must not) |
|---|---|---|---|
| money argv (§6.7, §2b.5) | `"(send\|sweep\|open\|create\|restore\|transfer\|relay)"` | `Quickshell.execDetached(["fmonero","wallet","send"])` | `Quickshell.execDetached(["fmonero","tui","--send"])` |
| no float money (§2a.5) | `parseFloat\|Number\(\|/ 1e` | `parseFloat(status.pending.txs[0].amount_xmr)`, `Number(status.wallet.balance_xmr) + 1`, `status.wallet.balance_pico / 1e12` | — |
| wallet secret (§2b.5) | `password` | `property string password: ""` | — |
| shell string (§2f, §1.5) | `execDetached\(\["(ba)?sh` · `bar\.run\(` | `execDetached(["bash","-lc",…])` · `bar.run("fmonero node up")` | — |

The money grep matches a **whole quoted argv element**, not a substring, which is why the constant
literal `"--send"` that opens the TUI's Send *screen* passes while `["fmonero","wallet","send"]`
cannot. A substring grep would fail on this design's own widget (§6.3's argv table contains
`"--send"`), and a grep that fails on correct code is the grep the first agent to hit it relaxes —
hence the third column.

The plugin has **no money argv at all**: the whole mutating surface is `node up`, `node down --yes`
and `wallet refresh`, plus `wl-copy` and three `omarchy-launch-or-focus-tui` launches built from
constant literals (§1.5). There is no address, amount or label in any argv that reaches a launcher
script's `eval`.

### 2.4 Live render against a full Status document

The empty state proves the widget loads; it does not prove the wallet, pending and warning sections
render. So the widget was pointed for the duration of the gate at a **test double** in the
scratchpad — a one-line script printing a complete §5 Status document — through the widget's own
`binary` setting, and the panel was driven over IPC and with `wtype`. Both machine changes were
reverted afterwards (§4).

Cursor state with that document (all five sections present, from the `cursor` IPC hook):

```
{"section":"header","index":0,"active":false,"opened":true,
 "sections":["header","warnings","wallet","pending","footer"],
 "alive":true,"binaryMissing":false,"nodeUp":true,"synced":false,"walletOpen":true,
 "iconCase":1,"iconColor":"#a4a4a4"}
```

Keyboard navigation, one `wtype` keypress at a time, reading the cursor back after each:

```
j j j        -> {"section":"warnings","index":1, …}
j j j j      -> {"section":"pending","index":0, …}
j            -> {"section":"footer","index":0, …}
l l          -> {"section":"footer","index":2, …}     # horizontal inside the footer
k            -> {"section":"pending","index":0, …}    # back up a section
c            -> clipboard := 4AdUndXHHZ6cfufTMvppY6JwXNouMBzSkbLYfpAV5Usx3skxNgYeYTRj5Uzqt…
Escape       -> {"opened":false, …}
```

What the panel drew (screenshot inspected, not pasted here): hero
`Node · 1 490 011` / `SYNCING 42%` / a `mainnet` pill / the node `ToggleSwitch` on; the sync gauge
`42%  1 490 011 / 3 541 122`; `Peers ↑ 16 out · ↓ 3 in`, `Mempool 14 tx`, `Last block 47s ago`,
`Net ↓ 375.0 KiB/s ↑ 20.5 KiB/s`; three warnings each with its runnable hint; `WALLET` with
`Balance 1.234567891234 XMR`, `Unlocked 0.5000 XMR`, `↑ locked for 7 blocks`, `4AdUndXH…684Rge
copy (c)` and `Refresh the wallet · 61 blocks behind`; and the pending row
`Send in flight: 12.5000 XMR → 4AdUndXH…684Rge · 1 of 3 relayed`. The journal carried no
`oliver.monero` line for the whole session but the two benign ones §1.5 names.

Every amount there is the pre-rendered `*_xmr` string from Status JSON, copied verbatim into the
label. Nothing in the plugin adds, divides or parses a monetary value (§2a.5).

`Model.js` was additionally exercised as a pure module against a synthetic Status document, since
formatter output is the part a screenshot cannot show precisely:

```
syncLabel       : Syncing 42%           heightLabel     : Node · 1 490 011
peersLabel      : ↑ 16 out · ↓ 3 in     peers restricted: not available on a restricted node
age 47/4000/90061/null : 47s / 1h 06m / 1d 1h / —
rate 384000 / null     : 375.0 KiB/s / —
shortAddr       : 4AdUndXH…684Rge       worstSeverity   : error
pending on      : Send in flight: 12.5000 XMR → 4AdUndXH…684Rge · 1 of 3 relayed
pending off     : Send in flight · 1 of 3 relayed
pending done    : Send in flight: 12.5000 XMR → 4AdUndXH…684Rge
safeArgv ok     : ["omarchy-launch-or-focus-tui","--app-id=fmonero","fmonero","tui"]
safeArgv evil   : []                    # "fmonero tui; rm -rf ~" is dropped, not split
parseStatus junk/err : {"ok":false} / {"ok":false}
syncLabel none/down/dead/synced : No node configured / Node is not running /
                                  fmonero is not answering / Synced
```

`pending off` is §6.5's rule made visible: with `showBalance` off the pending row carries neither
the amount nor the destination, only the relayed counter. The setting exists so an amount can be
kept off a bar other people can see, and pending is the row most likely to be on screen while
someone is looking over a shoulder.

---

## 3. Three defects the live shell found that no static check could

Recorded because each one is invisible to `qmllint`, to `omarchy-plugin-validate` and to every
grep, and each was caught by a different half of the gate.

1. **`ScrollBar is not a type`.** `Panel.qml` used QQC2's `ScrollBar` attached property without
   `import QtQuick.Controls`; `qmllint` reported no syntax error and the validator was happy, but
   the widget failed to instantiate:
   `WARN qml: Plugin widget oliver.monero failed: …/Panel.qml:300:9: ScrollBar is not a type`.
   The journal half of `tests/plugin-smoke.sh` is what surfaced it. Fixed in `d2324fc`.
2. **An invisible thousands separator.** `Model.grouped` joined digit groups with U+2009 THIN
   SPACE, which the bar's monospace fontconfig alias renders at zero width: the panel showed
   `Node · 1490011`. Only a screenshot could catch that. Now an ordinary space.
3. **A symlinked plugin does not hot-reload, and `rescanPlugins` is not enough.** §1.5 says to run
   `omarchy-shell shell rescanPlugins` after every QML edit. On this machine that reloads the
   registry but does **not** re-read the QML of an already-loaded symlinked plugin: after the
   `QtQuick.Controls` fix, two `rescanPlugins` runs still produced the old error from the old
   bytes, and only `omarchy restart shell` picked the edit up. fproton's smoke test records the
   same observation. The gate therefore restarts the shell before its final run, and
   `tests/plugin-smoke.sh` says so in a comment where the next agent will read it.

---

## 4. Machine state this agent changed, and what it left behind

| Change | State now | Why |
|---|---|---|
| `~/.config/omarchy/plugins/oliver.monero` → this checkout's `plugin/oliver.monero` | **left in place** | the gate requires it (§8), and the doctor's `plugin_linked` check expects it (§7.1) |
| `oliver.monero` enabled and put on the bar in `~/.config/omarchy/shell.json` | **left in place** | the gate requires "enabled on this machine" |
| `binary` setting pointed at a canned-Status test double | **reverted** | a gate harness, not a configuration (§2.4) |
| `~/.local/bin/fmonero` symlinked to `target/release/fmonero` | **removed** | installing the binary is `scripts/install.sh`'s job (docs agent) and §4b reserves the one real install run for the Phase 4 verifier; leaving a symlink to a build tree would also make the widget follow whatever any agent last compiled |
| clipboard | overwritten by the `c` key test, with the test double's example address | it was empty beforehand |

Consequence to expect: until `scripts/install.sh` runs, the bar widget correctly reports
`fmonero is not installed or not on PATH (~/.local/bin).` That is the §4b GREEN empty state, and it
is what the final gate transcript in §2.2 shows.

---

## 5. Deviations from DESIGN.md, and one defect outside this agent's files

### 5.1 `setup --plugin` does not rescan between the symlink and the enable — **a real defect**

`fmonero setup --plugin --yes` ran on this machine at the gate and produced:

```json
{"actions":[
 {"what":"CreateSymlink -> …/plugin/oliver.monero","status":"written"},
 {"what":"RunArgv omarchy plugin enable oliver.monero: omarchy-plugin-enable: plugin 'oliver.monero'
          is not known; run: omarchy-shell shell rescanPlugins","status":"skipped"},
 {"what":"RunArgv omarchy bar put oliver.monero --before omarchy.tailscale: omarchy-bar:
          oliver.monero is not a known widget; run 'omarchy plugin list'","status":"skipped"}],
 "dry_run":false,"ok":true,"schema":1}
```

The symlink lands, but `omarchy plugin enable` and `omarchy bar put` both fail on a **first**
install, because the running shell has not yet noticed the new directory. §4.2 lists the three
steps without a rescan; §6.6's manual sequence has `omarchy-shell shell rescanPlugins` **between**
the symlink and the enable, and that is the order that works. Run by hand in that order, all three
steps succeeded:

```
$ omarchy-shell shell rescanPlugins && omarchy plugin list | grep monero
oliver.monero                    disabled  third-party bar-widget         Monero
$ omarchy plugin enable oliver.monero
Enabled oliver.monero
$ omarchy bar put oliver.monero --before omarchy.tailscale
oliver.monero is on the bar
```

`setup --plugin` still exits **0** with `ok: true` while two of its three actions were skipped, so
a scripted `setup --all` on a fresh machine reports success and leaves the widget installed but
neither enabled nor on the bar — and the doctor's `plugin_enabled` check (§7.1) is what would
eventually say so. `crates/fmonero/src/setup.rs` is Phase 1b's file and this agent does not own it
(§2k), so the fix is reported rather than made: **`setup --plugin` should run
`omarchy-shell shell rescanPlugins` after creating the symlink and before `omarchy plugin enable`**,
and should treat a skipped enable as a partial result rather than a success.

### 5.2 `MiniBar` is a local component, not a `qs.Ui` type

§6.5 asks for the node detail to draw "sync `MiniBar` with percent". `MiniBar` is an **ftui** widget
name (§1.4, the TUI's progress bar); `qs.Ui` has no such component — its nearest relative is
`PanelSlider`, which is an interactive control and a cursor target, which this row must not be. So
`Panel.qml` defines a local `component MiniBar` — a track, a fill and the `42%  height / target`
caption, 20 lines, not a cursor target. The design text stays literally true and no new file was
added.

### 5.3 The percent number is drawn inside the ring, and only on a tall bar

§6.4 asks for "a percent ring around the glyph at `syncPct`, plus the number when `barSize`
allows". `BarIconButton` loads its `iconComponent` into a fixed optical canvas inside a fixed slot
(`Style.bar.iconCanvas` inside `Style.bar.iconSlot`), so a number drawn *beside* the glyph would
overflow the slot and collide with the neighbouring widget. `MoneroIcon` therefore takes a
`percentText` property and draws the number **inside** the ring in place of the M, gated on
`bar.barSize >= 30` — below that, two digits are not legible at this icon size and only the ring is
drawn. Same information, inside the space the host actually gives a bar widget.

### 5.4 The Monero mark is constructed geometry, not traced artwork

§6.1 asks for "the Monero ⓜ glyph as `QtQuick.Shapes` + `PathSvg`; no image asset".
`MoneroIcon.qml` draws a ring (`PathAngleArc`) and the M as one stroked `PathSvg` polyline whose
coordinates are defined in that file, rather than a path copied from the brand SVG. It renders the
same silhouette at bar size, it takes the theme foreground like every other bar glyph, and it
carries no third-party artwork or licence question. The sync ring is a second `PathAngleArc` at a
larger radius, and `filled` (a wallet is open) fills the glyph ring at 30 % alpha rather than
knocking the M out in a background colour, which would have to guess at what is behind a
transparent bar.

### 5.5 `showBalance` also gates the panel's balance rows

§6.4 gates the tooltip on `showBalance` and §6.5 gates the pending row on it; the panel's own
`Balance` / `Unlocked` rows are not mentioned either way. They are gated too — they render `•••••`
with the setting off — because the setting's whole purpose is keeping an amount off a screen other
people can see, and a setting that hides the balance on the bar while showing it two rows down does
not mean what its name says. The manifest label states it: *Show amounts on the bar and in the
panel (on/off)*.

### 5.6 Nothing else

Every other §6 requirement is implemented as written: the manifest keys and their in-code fallbacks
(§6.2, asserted by the smoke test), `Service.qml`'s property model verbatim (§6.3), the 3 s / 8 s
timers with the in-flight skip and the 8 s watchdog (§6.3, §2d), the `ok === true` gate on every
rendered value, the argv table (§6.3), the six-row first-match-wins icon table (§6.4), the section
order, the keyboard map and `PanelKeyCatcher` owning only dispatch (§6.5), and
`Quickshell.execDetached(argv)` everywhere with constant literals for the three launcher scripts
(§1.5). `bar.run()` appears nowhere.
