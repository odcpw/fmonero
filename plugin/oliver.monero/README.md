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
| `Service.qml` | ≤ 360 | the state model: one poll process, the serialized action queue |
| `Panel.qml` | ≤ 885 | the bar button and the panel content, including the keyboard state machine |
| `MoneroIcon.qml` | ≤ 112 | the Monero mark as `QtQuick.Shapes` + `PathSvg`; no image asset |
| `Model.js` | ≤ 340 | pure formatters: `parseStatus`, `shortAddr`, `age`, `rate`, `syncLabel`, `pendingLabel` |

The budgets are `DESIGN.md` §6.1 and allow 1 725 lines including the manifest; the plugin gate records the real counts
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
spawn, and that spawn is the expensive part. Both poll variants request a bounded node check;
core shares and throttles it with the TUI, so the remote height stays fresh while the panel is
closed while fmonero is active. While the app is closed, only `app status` checks local
activity; it makes no wallet reads or public-node requests. Each active status operation has a two-second deadline and the overall watchdog allows sixty
seconds for the full sequence. A slow wallet read is shown as busy, not as a closed wallet.

## Using it

The compact header follows the torrent widget: **Open wallet**, **More in the TUI**,
**Refresh**, and **Power**. Hover a control for its label. The widget appears only while
fmonero is open or one of its owned services is running. Launch fmonero from the menu or terminal
to bring it back after closing. Left click opens the panel, right click refreshes, and middle
click opens the TUI.

- **Open wallet / Switch wallet** shows the wallet list directly in the panel. Choose a wallet,
  enter its password in the masked field, and press Enter or **Open wallet**. The service starts
  automatically if needed. A wrong-password message stays visible, and the field is cleared.
- **Close wallet** saves and closes the open wallet while leaving the service running.
- **Power** saves and closes fmonero: it stops the wallet service and fmonero's own local node,
  closes existing TUI windows, and removes the icon from the bar. It preserves pending-payment
  guards and does not control separately managed node services.
- Balances, sync progress, address copying, payment checks, and pending-payment warnings stay
  in the panel. Creating/restoring wallets, sending payments, node selection, and diagnostics
  remain available in the TUI.

Keys: arrows or `h j k l` move the cursor; Enter/Space activates; Esc backs out of the picker
or closes the panel; Tab switches bar panels. `w` opens the wallet list, `q` is power, `t` opens
the TUI, `r` refreshes, `c` copies the receiving address, `n` opens node selection, and `d` opens
the doctor. While entering a password, typing belongs exclusively to the field; Tab moves to
its Open/Back buttons and Esc returns to the wallet list.

## Passwords and payments

The widget cannot construct or broadcast payments. The static checks still forbid send, sweep,
create, restore, transfer, and relay command arguments, and arithmetic on monetary values.

Opening a wallet is now explicitly supported. The password is transient: the field is masked,
cleared after submission or dismissal, and passed to `wallet open --password-stdin` through a
private process-input pipe. It is never put in arguments, environment variables, clipboard,
settings, logs, or a temporary file. The receiving command requires a complete line within ten
seconds and bounds its size. Wallet names are separate argv elements from the app's wallet list.

Open, create, close, and stop operations honor the payment lock and pending-payment interlock.
The power button calls `fmonero app stop --yes`; the widget never invokes systemctl directly.
Closing a panel does not cancel an opening operation already submitted to Monero.

## Status

`tests/plugin-smoke.sh` is what verifies this widget: the manifest, the required files, the
greps above with their own positive fixtures, a rescan, and the journal read afterwards. The
numbers it displays come from `fmonero status --json` against real daemons, and those are
unverified: see the Status section of the [main README](../../README.md#status).

## Credits

The panel shares Omarchy controls with the sibling torrent and FProton widgets.
`MoneroIcon.qml` draws the Monero mark as paths, so it takes the theme foreground instead of the
brand colours.
