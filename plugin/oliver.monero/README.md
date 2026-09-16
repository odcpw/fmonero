# oliver.monero

The Monero bar widget for Omarchy 4, and the front end of [fmonero](../../README.md). It draws
the Monero mark in the bar and opens a keyboard-driven panel; everything it knows comes from
`fmonero status --json`, and everything it does is an `fmonero` command run as argv.

The QML holds no business logic: no wallet2, no JSON-RPC, no shell strings, and no arithmetic on
money. Status JSON already carries every amount twice, as an integer of piconero and as a
pre-rendered string, so the panel formats and never divides.

## Install

The widget comes with fmonero:

```bash
fmonero setup --plugin        # symlink, enable, place before omarchy.tailscale
```

By hand:

```bash
ln -sfn "$PWD/plugin/oliver.monero" ~/.config/omarchy/plugins/oliver.monero
omarchy plugin enable oliver.monero
omarchy bar put oliver.monero --before omarchy.tailscale
```

`fmonero` must be on the shell's PATH, which includes `~/.local/bin` and not `~/.cargo/bin`.
The panel says so, and stops polling until the next time it is opened, when `which` cannot find
the binary.

## Developing

A symlinked plugin directory is discovered and loaded, but it does not hot-reload: the registry
watches with `inotifywait -r`, which does not descend symlinks. So after every QML edit:

```bash
omarchy-shell shell rescanPlugins            # the documented reload (DESIGN.md §1.5, §6.6)
journalctl --user -t omarchy-shell -f        # real failures read: Plugin widget <id> failed: …
```

Note what the sibling project measured on this same machine: `rescanPlugins` is the cold-load
path, and it did **not** re-read the QML of an already-loaded symlinked plugin (the registry's
reload is guarded by `Qt.clearComponentCache`). There, `omarchy restart shell` was the thing
that picked up an edit. If a change does not appear after a rescan, restart the shell before
you go looking for the bug in your QML.

`omarchy-plugin-validate` refuses any symlink, so validate the repository directory and never
the installed link:

```bash
omarchy-plugin-validate plugin/oliver.monero
tests/plugin-smoke.sh                        # validate, manifest checks, greps, rescan, journal
```

One benign line appears on every reload and the smoke test whitelists it:
`QML IpcHandler … Handler was registered but will not be used`.

## Files

| File | Budget | Contents |
|---|---|---|
| `manifest.json` | ~55 | id, kinds, `entryPoints.barWidget`, and the Setup > Plugins form (`barWidget.defaults` / `schema`) |
| `Service.qml` | ≤ 380 | the state model: one poll process, the serialized action queue |
| `Panel.qml` | ≤ 700 | the bar button and the panel content, including the keyboard state machine |
| `MoneroIcon.qml` | ≤ 80 | the Monero mark as `QtQuick.Shapes` + `PathSvg`; no image asset |
| `Model.js` | ≤ 150 | pure formatters: `parseStatus`, `shortAddr`, `age`, `rate`, `syncLabel`, `pendingLabel` |

The budgets are `DESIGN.md` §6.1 and total 1 310 lines; the plugin gate records the real counts
against them in `DECISIONS.md`.

## Settings

Omarchy never merges a manifest's `barWidget.defaults` into the plugin's `settings`: enabling
the widget writes `{"id":"oliver.monero"}` into `shell.json`, and the widget receives only the
keys someone typed in the Setup form. Every setting therefore has an in-code fallback that
repeats the manifest value verbatim, and the smoke test asserts that each manifest default has
one.

| Setting | Default | Meaning |
|---|---|---|
| `binary` | `fmonero` | resolved with `which` on first load |
| `refreshIntervalSec` | 3 (1-60) | poll interval while the panel is open |
| `idleRefreshIntervalSec` | 8 (2-600) | poll interval while it is closed |
| `showBalance` | `on` | `off` keeps the balance, the pending amount and the destination off the bar |
| `tuiCommand` | `omarchy-launch-or-focus-tui --app-id=fmonero fmonero tui` | how the footer opens the TUI |

The two intervals differ because the two situations differ: 3 s while you are watching a sync
percentage tick, 8 s while the widget only has to pick an icon state. Each poll is a process
spawn, and that spawn is the expensive part.

## Using it

Bar icon: left click toggles the panel, right click toggles the node, middle click refreshes the
poll. First match wins for the icon state: fmonero unreachable is dimmed and crossed with a
warning badge; any `error`-severity warning (a send that may be in flight, an armed stall
breaker, Monero not installed) turns it urgent; a node that is down is dimmed and crossed; a
node that is not synchronized gets a percent ring; an open wallet fills the glyph; otherwise it
is the outline. The tooltip reads `Monero · <height> · <peers> peers · <sync>`, plus the balance
when a wallet is open and `showBalance` is `on`.

In the panel, sections in cursor order: header (node state and a toggle), node detail (sync bar,
peers in and out, mempool, last block age, net rates), the current warning with its hint,
wallet (balance, unlocked, locked-for-N-blocks, the shortened primary address with a copy
action, a refresh row), the pending-send row, and the footer (Open TUI, Send, Doctor).

Keys: `j`/`k` or the arrows move, Enter or Space activates, Esc closes, Tab moves to the next
bar panel. Shortcuts: `n` toggles the node, `r` refreshes the poll, `t` opens the TUI, `c`
copies the primary address, `d` opens the doctor. `h j k l` are consumed by the key catcher and
are not shortcuts.

Under Arch's system `monerod.service`, `node up` and `node down` exit 3 without issuing an RPC.
The panel renders that refusal's `hint` like any other failure, and its toggle stays bound to
the real `node.reachable`, so the widget never appears to have done something it did not.

## Safety

The widget cannot move money, and that is enforced by greps rather than by care:

- No `wallet open`, `create`, `send`, `sweep`, `transfer` or `relay` argv anywhere, and no
  `password` string in any QML file. `tests/plugin-smoke.sh` matches on whole argv elements, so
  the constant literal `--send` that opens the TUI's Send screen passes while
  `[bin, "wallet", "send", …]` cannot. It shows a balance only when `status.wallet.open` is
  already true, and it has no password field.
- No `parseFloat`, no `Number(`, no `/ 1e`. Amounts come out of Status JSON as pre-rendered
  strings, including the pending total, which Rust sums with a checked add precisely so the QML
  never has to.
- `bar.run()` is never used: it is `bash -lc <string>`. Every action is
  `Quickshell.execDetached([...])` with an argv array.
- `omarchy-launch-or-focus-tui`, `omarchy-launch-or-focus` and
  `omarchy-launch-floating-terminal-with-presentation` all end up feeding a string to `eval` or
  `bash -c`, so only constant literals reach them: never an address, an amount, a label, or
  anything derived from Status JSON. `omarchy-notification-send` is genuinely argv-shaped and is
  the only one that may carry computed text.
- The doctor opens under app-id `fmonerodoctor`, not `fmonero-doctor`:
  `omarchy-launch-or-focus` matches `\b<pattern>\b` against the window class and the title, so a
  hyphenated id would also match the TUI window.
- The one value that travels from Status JSON into an argv is the primary address, into
  `["wl-copy", "--", <address>]`, which is argv-shaped and came from our own JSON.

## Status

`tests/plugin-smoke.sh` is what verifies this widget: the manifest, the required files, the
greps above with their own positive fixtures, a rescan, and the journal read afterwards. The
numbers it displays come from `fmonero status --json` against real daemons, and those are
unverified: see the Status section of the [main README](../../README.md#status).

## Credits

The panel's chrome and its keyboard handling follow Omarchy's first-party Tailscale plugin.
`MoneroIcon.qml` draws the Monero mark as paths, so it takes the theme foreground instead of the
brand colours.
