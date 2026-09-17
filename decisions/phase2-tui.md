# Phase 2 — `tui` (DESIGN.md §7a)

Branch `phase2/tui`, based on `gate-1b`. Owns `crates/fmonero/src/tui/**` and
`tests/tui_tmux.sh`; this file is its evidence (§8's preamble — Phase 3 concatenates the four
`decisions/phase2-*.md` into `DECISIONS.md`).

Every capture below was taken on a **real pty** through `tmux capture-pane`, against
`fmonero-mock` over `tests/fixtures/` — **never a real daemon and never a real wallet**, so
§4b's classes hold: the rendering and the request shapes are AMBER, nothing here says anything
about a live `monerod`. §2b.2 is why: a real wallet's balance, primary address and
counterparties would otherwise be pasted into a decisions log.

Scenario for every capture: `fmonero-mock --role node/--role wallet --auth over tests/fixtures/ (no real daemon, no real wallet)`

## What was built

`src/tui/{mod,theme,screens,send,subs}.rs` plus `src/tui/tests.rs`:

- **mod.rs** — the `Model`: the 1 s `status` poll (dropped while one is in flight, §2d), the
  120 ms spinner tick that touches no daemon, `Cmd::task` for every short read, one
  `ActionSub` at a time for everything that may take a minute, the key router, the four
  modals' state and the exit-code rule (`Ok(0)`, or the last failed action's own §4a code).
- **theme.rs** — fproton's Omarchy loader verbatim, plus `Theme::badge` (§7a's badge colours)
  and `Theme::qr`. **The only literal colours in `src/tui/**` are in this file.**
- **screens.rs** — the chrome (tab strip, pending banner, status line), the Node, Wallet and
  Log screens, the modals, the help overlay, the four resize tiers and the formatters.
- **send.rs** — the send state machine and the confirm modal.
- **subs.rs** — `Action` (neither `Clone` nor `Debug`, with a hand-written redacting `Debug`),
  `execute` (the body `ActionSub::run` and the `ProgramSimulator` test share), the
  cancellation watcher thread, and the rule that a **relay ignores the stop signal** (§7a).
- **tests.rs** — the render tests, the empty states and the `ProgramSimulator` send test.

The TUI calls `fmonero-core` in process and never spawns `fmonero`. It never calls
`build_transfer`, `build_sweep_all` or `relay_tx` and never writes `pending.json`: the money
path is `send::build` + `send::relay` (§2a.3). `u`/`d` go through `node_session` and the open
modal through `wallet_session`, so the §2i single-writer precondition and the `set_daemon`
step exist in exactly one place (§5b).

## Gate

| Gate line (§8, Phase 2 "tui") | Result |
|---|---|
| clippy clean | **pass** — `cargo clippy --all-targets -- -D warnings`, exit 0 |
| ≤ 1 800 lines | **FAIL — 4330 lines** in the five named files (see "The line budget" below) |
| `fmonero tui` opens and quits on a real pty | **pass** — `tests/tui_tmux.sh`, 10 checks |
| every screen renders with the mock up | **pass** — `tests/tui_tmux.sh --capture`, 13 checks; captures below |
| every screen renders with both daemons absent | **pass** — `tests/tui_tmux.sh` and the tier test |
| the four resize tiers have render tests | **pass** — `every_screen_renders_at_every_tier_with_both_daemons_absent` (4 screens × Wide/Medium/Narrow/Tiny) |
| the `ProgramSimulator` send test: one `transfer` + one `relay_tx`, identical metadata | **pass** — `the_send_screen_builds_once_and_relays_that_exact_transaction` |
| the `Action` redaction test | **pass** — `debugging_an_open_never_prints_the_password` |
| no `println!` | **pass** — grep below, and `tests/tui_tmux.sh` asserts it |
| no literal colour outside `theme.rs` | **pass** — grep below |
| a capture of each screen, scenario named | **pass** — below |

### clippy

```text
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.10s
exit 0
```

### `cargo test -p fmonero tui`

```text
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/main.rs (target/debug/deps/fmonero-7deb80b216015a31)

running 34 tests
test tui::screens::tests::an_address_wraps_rather_than_eliding_when_it_is_being_confirmed ... ok
test tui::screens::tests::the_resize_tiers_are_the_ones_the_design_names ... ok
test tui::send::tests::the_confirm_modal_needs_the_word_not_just_enter ... ok
test tui::send::tests::the_picker_yields_a_literal_address_never_a_label ... ok
test tui::send::tests::the_account_selector_stays_inside_the_accounts_the_wallet_has ... ok
test tui::subs::tests::an_error_keeps_its_own_exit_code_and_hint ... ok
test tui::send::tests::a_zero_amount_and_a_bad_address_are_both_refused_before_anything_is_sent ... ok
test tui::subs::tests::debugging_an_open_never_prints_the_password ... ok
test tui::send::tests::a_good_form_becomes_a_send_request_without_a_sweep_field ... ok
test tui::subs::tests::every_action_but_a_relay_is_cancellable ... ok
test tui::subs::tests::labels_name_what_is_running ... ok
test tui::tests::mock::tests::auth_params_splits_quoted_and_bare_values ... ok
test tui::send::tests::all_is_refused_inline_and_names_the_sweep_command ... ok
test tui::tests::mock::tests::digest_matches_rfc2617_and_differs_from_md5_sess ... ok
test tui::tests::mock::tests::md5_matches_rfc1321_edges ... ok
test tui::tests::a_zero_area_frame_renders_nothing_and_does_not_panic ... ok
test tui::tests::quitting_while_an_action_runs_takes_two_presses ... ok
test tui::screens::tests::human_numbers_and_short_forms_read_like_the_cli_ones ... ok
test tui::tests::the_narrow_tier_collapses_the_tab_strip ... ok
test tui::tests::the_help_overlay_lists_the_bindings_the_model_implements ... ok
test tui::tests::node_mutations_are_refused_when_fmonero_does_not_own_the_node ... ok
test tui::tests::the_offline_node_screen_says_what_to_press_and_never_shows_a_zero_height ... ok
test tui::tests::the_offline_wallet_screen_offers_the_open_key ... ok
test tui::tests::the_open_wallet_modal_masks_the_password_and_lists_the_wallet_dir ... ok
test tui::theme::tests::every_badge_state_has_its_own_colour ... ok
test tui::theme::tests::hex_parsing_tolerates_the_hash_and_trailing_alpha ... ok
test tui::theme::tests::the_qr_is_dark_on_light_whatever_the_theme_is ... ok
test tui::tests::the_confirm_modal_owns_the_keys_even_on_another_screen ... ok
test tui::tests::the_qr_modal_draws_the_address_and_the_code ... ok
test tui::tests::the_send_screen_refuses_all_inline_and_never_reaches_the_daemon ... ok
test tui::tests::every_screen_renders_at_every_tier_with_both_daemons_absent ... ok
test tui::tests::the_send_screen_builds_once_and_relays_that_exact_transaction ... ok
test tui::tests::every_screen_renders_against_the_mock ... ok
test tui::tests::an_unresolved_pending_record_banners_every_screen ... ok

test result: ok. 34 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 0.26s

exit 0
```

### `tests/tui_tmux.sh` — a real pty, both daemons absent (§4b GREEN)

```text

== build
  ok    cargo build --release

== the TUI on a pty (both daemons absent)
  ok    fmonero tui opened on a pty
  ok    Node screen renders (Node)
  ok    Wallet screen renders (Balance)
  ok    Send screen renders (Priority)
  ok    Log screen renders (Log)
  ok    help overlay renders (Keys)
  ok    q quits cleanly (exit 0)
  ok    no print macro under src/tui
  ok    the TUI wrote nothing to stderr

== 10 passed, 0 failed
exit 0
```

### `tests/tui_tmux.sh --capture` — a real pty against `fmonero-mock` (§4b AMBER)

```text

== build
  ok    cargo build --release

== the TUI on a pty (fmonero-mock --role node/--role wallet --auth over tests/fixtures/ (no real daemon, no real wallet))
  ok    fmonero tui opened on a pty
  ok    Node screen renders (Node)
  ok    Wallet screen renders (Balance)
  ok    Send screen renders (Priority)
  ok    Log screen renders (Log)
  ok    help overlay renders (Keys)
  ok    receive QR renders (Receive)
  ok    the send confirm modal shows the built fee and wants the word
  ok    nothing was relayed (the capture run never broadcasts)
  ok    q quits cleanly (exit 0)
  ok    no print macro under src/tui
  ok    the TUI wrote nothing to stderr

  captures in /tmp/claude-1000/-home-oliver--config-Claude-scratch-workspaces-eff1ae0e-d485-4f42-9ffb-620b5efc18d5-813ee62c-1eed-4c69-bc1f-0b109cdbca45-scratch-2026-09-16-3ded94/4901fef8-2906-4bc0-b226-799715c2e661/scratchpad/tui-cap

== 13 passed, 0 failed
exit 0
```

### The two greps §7a asks for

```console
$ grep -rnE "PackedRgba|rgb\(|Rgb|#[0-9a-fA-F]{6}" crates/fmonero/src/tui --include=*.rs \
    | grep -v "^crates/fmonero/src/tui/theme.rs"
$ echo $?
1
$ grep -rnE '\b(println!|print!|eprintln!|eprint!|dbg!)' crates/fmonero/src/tui
$ echo $?
1
```

`scripts/check.sh`'s own §7a step agrees:

```console
== §7a  the TUI's Action derives neither Clone nor Debug
  ok    Action carries no derived Clone or Debug
```

### Line counts

```text
  1597 crates/fmonero/src/tui/mod.rs
  1456 crates/fmonero/src/tui/screens.rs
   772 crates/fmonero/src/tui/send.rs
   304 crates/fmonero/src/tui/subs.rs
   585 crates/fmonero/src/tui/tests.rs
   201 crates/fmonero/src/tui/theme.rs
  4915 total
```

## Screen captures

Each is `tmux capture-pane -p` on a 120×40 pane, trailing blank rows dropped.

### Node

```text
[ 1 Node ]  2 Wallet    3 Send    4 Log                                                                 mainnet omarchy
╭ Node ──────────────────────────────────────────────────────╮╭ Net ───────────────────────────────────────────────────╮
│ ● Synchronized                                             ││ ↓ ↑        0 B/s   0 B/s                               │
│ sync       ████████████ 100.0%  3541122 / 3541122          ││ limits     ↑2048 ↓8192 kB/s                            │
│ last block 13h 03m ago                                     ││                                                        │
│ peers      16 out · 3 in                                   │╰────────────────────────────────────────────────────────╯
│ version    0.18.5.1-release · fork v16 · mainnet           │╭ Mempool · fees ────────────────────────────────────────╮
│                                                            ││ pool       14 txs · 20.9 kB · oldest 13h 06m ago       │
│                                                            ││ pool fees  0.000427 XMR · 0 failing · 0 double spends  │
│                                                            ││ fee/byte   1:0.00000002  2:0.00000008  3:0.00000032    │
│                                                            ││ 4:0.000004                                             │
╰────────────────────────────────────────────────────────────╯╰────────────────────────────────────────────────────────╯
╭ Peers  3 · b: bans ────────────────────────────────────────────────────╮╭ Warnings ──────────────────────────────────╮
│ HOST                   │STATE         │HEIGHT   │PRUNE │AVG ↓/↑        ││ monero_not_installed monerod is not        │
│ 203.0.113.17           │normal        │3541122  │0     │12 B/2 B       ││ installed                                  │
│ 198.51.100.42          │synchronizing │3541100  │384   │0 B/0 B        ││   hint: omarchy pkg add monero             │
│ 192.0.2.201            │normal        │3541122  │0     │1 B/31 B       ││ node_low_disk the node reports 96000000000 │
│                                                                        ││ bytes free, under the 120 GB floor for a   │
│                                                                        ││ pruned chain — a figure that is an         │
│                                                                        ││ ESTIMATE, not measured on this machine     │
│                                                                        ││   hint: fmonero doctor --only storage      │
│                                                                        ││ auth_missing the node accepted fmonero     │
│                                                                        ││ with no credential at all                  │
│                                                                        ││   hint: fmonero setup --units --yes        │
│                                                                        ││                                            │
│                                                                        ││                                            │
│                                                                        ││                                            │
│                                                                        ││                                            │
│                                                                        ││                                            │
│                                                                        ││                                            │
│                                                                        ││                                            │
│                                                                        ││                                            │
│                                                                        ││                                            │
│                                                                        ││                                            │
│                                                                        ││                                            │
│                                                                        ││                                            │
│                                                                        ││                                            │
│                                                                        ││                                            │
╰────────────────────────────────────────────────────────────────────────╯╰────────────────────────────────────────────╯
● node up 3541122 16↑3↓ ● wallet up 1.234567891234 XMR                                                            ? help
```

The gauge, `height / target_height`, the peers table, the mempool, the node's own `fees[]` and
the net limits all come from the fixtures; `monero_not_installed` is real (§1.1 — no Monero on
this machine), which is exactly what the Warnings pane is for.

### Wallet

```text
  1 Node  [ 2 Wallet ]  3 Send    4 Log                                                               ⠙ mainnet omarchy
╭ Balance ─────────────────────────────────────────────────────────────────────────────────────────────────────────────╮
│ 1.234567891234 XMR   unlocked 1.234567891234 XMR                                                                     │
╰──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯
╭ Subaddresses — Enter copies · a new · l label ─╮╭ History  6 rows · t: all ──────────────────────────────────────────╮
│ #  │ADDRESS ✓=used        │BALANCE             ││ / filter by txid, address or note                                  │
│ 0  │42ey1afD…fJJQAWDm ✓   │1.234567891234      ││ WHEN         TYPE            AMOUNT   CONF LOCKED est.           F │
│ 1  │8AsN91rz…N1EX5Mtm ✓   │—                   ││ 09-16 14:01  pending         0.1000      0 locked         0.000030 │
│ 2  │86kKnBKF…GRsdGfNk     │—                   ││ 09-15 13:20  failed          0.0200      0 locked         0.000030 │
│                                                ││ 09-16 10:00  out             0.2500    122                0.000030 │
│                                                ││ 09-15 00:40  in              0.5000   1122                0.000030 │
│                                                ││ 09-13 15:20  block           0.6000   2122                    0.00 │
│                                                ││ 09-12 06:00  in              0.0750   3122 locked         0.000030 │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
╰────────────────────────────────────────────────╯╰────────────────────────────────────────────────────────────────────╯
● node up 3541122 16↑3↓ ● wallet up 1.234567891234 XMR                                                            ? help
```

`—` in the BALANCE column is `get_balance.per_subaddress` holding only the **funded**
subaddresses (§5): a zero there would read as "empty". `CONF`/`LOCKED est.` come from
`history::confirmations` / `history::locked` against the **wallet's** height (§1b.4) and are
never recomputed in the TUI.

### Send

```text
  1 Node    2 Wallet  [ 3 Send ]  4 Log                                                               ⠙ mainnet omarchy
╭ Send ──────────────────────────────────────────────────────────────╮╭ Preview ───────────────────────────────────────╮
│ To      address, or @label from the address book                   ││ To                                             │
│                                                                    ││                                                │
│ Amount  0.5   ·   250000000000p                                    ││ Amount  —                                      │
│                                                                    ││ Fee     known only after the build (§2a.2)     │
│ Note    optional note, kept in the wallet                          ││ Account 0                                      │
│                                                                    ││ Unlocked 1.234567891234                        │
│ Priority 0=1  1 0.00000002  2 0.00000008  3 0.00000032  4 0.000004 ││                                                │
│                                                                    ││                                                │
│ enter a recipient and an amount                                    ││                                                │
│ account 0 · Tab moves · @ picks from the address book · Esc leaves ││                                                │
│ the form                                                           ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
╰────────────────────────────────────────────────────────────────────╯╰────────────────────────────────────────────────╯
● node up 3541122 16↑3↓ ● wallet up 1.234567891234 XMR                                                            ? help
```

### Send — the confirm modal, after a real build against the mock

Driven on the pty: the address and `0.01` typed, `Enter` → one `validate_address`, then
`send::build` with `do_not_relay`. The fee, the total and the weight are the built
transaction's own (§2a.2). `Esc` discarded it; the run asserts the mock recorded **no**
`relay_tx`.

```text
  1 Node    2 Wallet  [ 3 Send ]  4 Log                                                                 mainnet omarchy
╭ Send ──────────────────────────────────────────────────────────────╮╭ Preview ───────────────────────────────────────╮
│ To      DwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQAWDm  ││ To                                             │
│                                                                    ││ 42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVd │
│ Amount  0.01                                                       ││ FCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJQA │
│                                                                    ││ WDm                                            │
│ Note    optional note, kept in the wallet                          ││ Amount  0.0100 XMR                             │
│                                                                    ││ Fee     known only after the build (§2a.2)     │
│ Priority 0=1  1 0.00000002  2 0.00000008  3 0.00000032  4 0.000004 ││ Account 0                                      │
│                                                                    ││ Unlocked 1.234567891234                        │
│ mainnet standard · 0.0100 XMR · Enter builds it                    ││                                                │
│ account 0 · Tab moves · @ picks from the address book · Esc leaves ││                                                │
│ the form                                                           ││                                                │
│                    ╭ Confirm — this broadcasts a transaction ───────────────────────────────────╮                    │
│                    │                                                                            │                    │
│                    │ To                                                                         │                    │
│                    │ 42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRVdFCSoHnv7KPbBeGpzJBzHRCAs9Uxq │                    │
│                    │ eoyFQMYbqSWYTfJJQAWDm                                                      │                    │
│                    │ Amount 0.1000 XMR  (100000000000 pico)                                     │                    │
│                    │ Fee    0.00003072 XMR  (30720000 pico)                                     │                    │
│                    │ Total  0.10003072 XMR  (100030720000 pico)                                 │                    │
│                    │ Account 0  Priority 2  Weight 1531                                         │                    │
│                    │                                                                            │                    │
│                    │ Type "send" and press Enter to broadcast · Esc to discard                  │                    │
│                    │                                                                            │                    │
│                    ╰────────────────────────────────────────────────────────────────────────────╯                    │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
│                                                                    ││                                                │
╰────────────────────────────────────────────────────────────────────╯╰────────────────────────────────────────────────╯
● node up 3541122 16↑3↓ ● wallet up 1.234567891234 XMR │ built — nothing is broadcast yet                         ? help
```

### Log

```text
  1 Node    2 Wallet    3 Send  [ 4 Log ]                                                               mainnet omarchy
╭ Log  /tmp/tmp.4mw0vXkoAE/bitmonero.log  follow on ───────────────────────────────────────────────────────────────────╮
│ / search the log                                                                                                     │
│ 2026-09-16 14:02:09.114 I Monero 'Fluorine Fermi' (v0.18.5.1-release) started                                        │
│ 2026-09-16 14:02:10.002 I Synced 3541122/3541122                                                                     │
│ 2026-09-16 14:02:11.771 I [127.0.0.1:18080 OUT] NEW BLOCK 3541122                                                    │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
╰──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯
● node up 3541122 16↑3↓ ● wallet up 1.234567891234 XMR │ form unfocused — 1..4 switch screens, Enter returns      ? help
```

### The `?` overlay — which is the `KEYS` table, so help and behaviour cannot drift

```text
  1 Node    2 Wallet    3 Send  [ 4 Log ]                                                               mainnet omarchy
╭ Log  /tmp/tmp.4mw0vXkoAE/bitmonero.log  follow on ───────────────────────────────────────────────────────────────────╮
│ / search the log                                                                                                     │
│ 2026-09-16 14:02:09.114 I Monero 'Fluorine Fermi' (v0.18.5.1-release) started                                        │
│ 2026-09-16 14:02:10.002 I Synced 3541122/3541122                                                                     │
│ 2026-09-16 14:02:11.771 I [127.0.0.1:18080 OUT] NEW BLOCK 3541122                                                    │
│                                                                                                                      │
│                                                                                                                      │
│         ╭ Keys — any key closes ───────────────────────────────────────────────────────────────────────────╮         │
│         │                                                                                                  │         │
│         │ q  Ctrl+C             quit (twice while an action runs: cancel, then quit)                       │         │
│         │ ?                     this help                                                                  │         │
│         │ Tab  ⇧Tab  1 2 3 4    Node · Wallet · Send · Log                                                 │         │
│         │ r                     refresh now                                                                │         │
│         │ Esc                   close this overlay, else cancel the running action                         │         │
│         │ u  d                  Node: start · stop the node (confirm modal)                                │         │
│         │ b                     Node: peers ⇄ bans                                                         │         │
│         │ L                     cycle the daemon log level (confirm modal)                                 │         │
│         │ o  c                  Wallet: open (modal) · close                                               │         │
│         │ a  l                  Wallet: new subaddress · label the selected one                            │         │
│         │ n                     Wallet: edit the selected transfer's note                                  │         │
│         │ R  Q                  Wallet: refresh · show the address as a QR                                 │         │
│         │ /  t                  Wallet: filter rows · cycle the type filter                                │         │
│         │ ←→  Enter             Wallet: swap panes · copy an address · open a row                          │         │
│         │ Send: Tab  Enter      next field · build  (Esc leaves the form, Enter returns to it)             │         │
│         │ Send: p  Ctrl+P  @    cycle priority (on its row, or from anywhere) · address book               │         │
│         │ Send: ←→              on the Priority or Account row: change it (accounts come from the wallet)  │         │
│         │ Send: sweep           not in the TUI — run: fmonero wallet sweep <addr> --dry-run                │         │
│         │ Send: type "send"     the confirm modal's --yes; Esc discards, and a relay cannot be cancelled   │         │
│         │                                                                                                  │         │
│         ╰──────────────────────────────────────────────────────────────────────────────────────────────────╯         │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
│                                                                                                                      │
╰──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯
● node up 3541122 16↑3↓ ● wallet up 1.234567891234 XMR │ form unfocused — 1..4 switch screens, Enter returns      ? help
```

### Receive — the QR on an `ftui_extras` `Canvas` in `Mode::HalfBlock`

```text
  1 Node  [ 2 Wallet ]  3 Send    4 Log                                                                 mainnet omarchy
╭ Balance ─────────────────────────────────────────────────────────────────────────────────────────────────────────────╮
│ 1.234567891234 XMR   unlocked 1.234567891234 XMR                                                                     │
╰──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯
╭ Subaddresses — Enter copies · a new · l label ─╮╭ History  6 rows · t: all ──────────────────────────────────────────╮
│ #  │ADDRESS ✓=used        │BALANC╭ Receive — Esc closes ─────────────────────────╮e                                  │
│ 0  │42ey1afD…fJJQAWDm ✓   │1.2345│                                               │UNT   CONF LOCKED est.           F │
│ 1  │8AsN91rz…N1EX5Mtm ✓   │—     │ ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀ │000      0 locked         0.000030 │
│ 2  │86kKnBKF…GRsdGfNk     │—     │ ▀▀ ▄▄▄▄▄ ▀▀▀ ▀ ▄▀▀▄▄ ▄ ▀ ▄▄▀ ▀▀▀ ▀▀▀ ▄▄▄▄▄ ▀▀ │200      0 locked         0.000030 │
│                                  │ ▀▀ ▀   ▀ ▀▀▀▀▄▄▀▀▀▀▀ ▄▄▀▀▀▄▄▀▄▄ ▄▄▄▀ ▀   ▀ ▀▀ │500    122                0.000030 │
│                                  │ ▀▀ ▀▄▄▄▀ ▀▀▀▀▄▄ ▀▀ ▀ ▀▀  ▀▀▀▄▄▄▀   ▀ ▀▄▄▄▀ ▀▀ │000   1122                0.000030 │
│                                  │ ▀▀▄▄▄▄▄▄▄▀▄▀▄▀▄▀▄▀▄▀ ▀ ▀ ▀▄▀▄▀▄▀▄▀ ▀▄▄▄▄▄▄▄▀▀ │000   2122                    0.00 │
│                                  │ ▀▀ ▀▀▄▀▄▄▀▄ ▄  ▀▀▄▀▀▄▄▀▀▄▀▄▄▀▄▀▀▀ ▀ ▀▄▀▀▀▀▀▀▀ │750   3122 locked         0.000030 │
│                                  │ ▀▀▀▀▀▀▀▀▄  ▄ ▀▄ ▀▄▀▀ ▄ ▀▄▀▀▀  ▀▄▀ ▄▀ ▀   ▀▀▀▀ │                                   │
│                                  │ ▀▀ ▀▀▄ ▀▄▄▀▀▀ ▀ ▄ ▄▀▄▄▀▀   ▀▀ ▀▄▀▀  ▀▀  ▀▀▀▀▀ │                                   │
│                                  │ ▀▀▀▀▀▀▀▀▄ ▄▀▀▀   ▀▀▄▄▀▄ ▀▀ ▀▀▄▄   ▀▀▄  ▄▄ ▀▀▀ │                                   │
│                                  │ ▀▀ ▄ ▀▀▄▄▀▀ ▄▄▀ ▄▀▄▀▀▄▄▀▀ ▄▀▀▀▀▀▀▀  ▀▄▀ ▄ ▀▀▀ │                                   │
│                                  │ ▀▀▄▀ ▄▄ ▄ ▀▀  ▄▀▀▄ ▀ ▄▀▄▀▀▀▀▄▀▀ ▀▀ ▄▄   ▄▀▀▀▀ │                                   │
│                                  │ ▀▀▀▀▄▀▀▀▄▀   ▀ ▀ ▄▀▄ ▄▀▀▀  ▀▀ ▄▀▀ ▀▀ ▄▀▀ ▀▀▀▀ │                                   │
│                                  │ ▀▀▄▀  ▄▀▄▀ ▄▀▀  ▀▀ ▀▄▄ ▀▀▀▄▄▀ ▀▄▀▄▀ ▄▀▀▄ ▀▀▀▀ │                                   │
│                                  │ ▀▀▄ ▀▀ ▄▄▀▀▀▀ ▄ ▄▀▀▄▀  ▀  ▀▀▀ ▄▄▀▄ ▀▀▀▄▀▄▀▀▀▀ │                                   │
│                                  │ ▀▀   ▄▄▀▄▄▄▀   ▀ ▀▄▄▀  ▀▄ ▄▀ ▀▄▀ ▀▀▀▀▄ ▀▄▀▀▀▀ │                                   │
│                                  │ ▀▀▀  ▄▀▀▄▀▄▀▀▀▀ ▀▄▀▀▀▀▄▀▄▀▄▄▀▀ ▀▀  ▀▀▀▄▄▀▀▄▀▀ │                                   │
│                                  │ ▀▀▄▀▀▀▀ ▄▄▀ ▀▄▄  ▀▀ ▄   ▀▀▄▀▄▀ ▀▄ ▄▀ ▄▀▀  ▀▀▀ │                                   │
│                                  │ ▀▀▄▀▀▀▀▀▄▄ ▀▀  ▀▄▀▀▀▄▄▀ ▄▀ ▄▀▀ ▄▀  ▄▄▄ ▀  ▀▀▀ │                                   │
│                                  │ ▀▀ ▄▄▄▄▄ ▀▀▀  ▀▄ ▀  ▀▀▄▀▀▀ ▀▄▀ ▀▀▀ ▀▄▀  ▀▀▀▀▀ │                                   │
│                                  │ ▀▀ ▀   ▀ ▀▀▀▀ ▀▄▀▀ ▀▀  ▀▄▄▄▄▀ ▄▀▀▄▄▄ ▄ ▀ ▀▀▀▀ │                                   │
│                                  │ ▀▀ ▀▄▄▄▀ ▀▀▀▀▀▀▀▀▀▀ ▀▀▀▀▀▀ ▀▄▀▀▀▀▀ ▀▀▀▄▀▄▀▄▀▀ │                                   │
│                                  │ ▀▀▄▄▄▄▄▄▄▀▄▄▄▀▀▀▀▄▀▀▀▀▄▀▀▄▄▄▄▄▀▄▄▀▀▀▀▄▄▄▀▄▀▀▀ │                                   │
│                                  │ ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀ │                                   │
│                                  │ 42ey1afDFnn4886T7196doS9GPMzexD9gXpsZJDwVjeRV │                                   │
│                                  │ dFCSoHnv7KPbBeGpzJBzHRCAs9UxqeoyFQMYbqSWYTfJJ │                                   │
│                                  │ QAWDm                                         │                                   │
│                                  │                                               │                                   │
│                                  ╰───────────────────────────────────────────────╯                                   │
│                                                ││                                                                    │
│                                                ││                                                                    │
│                                                ││                                                                    │
╰────────────────────────────────────────────────╯╰────────────────────────────────────────────────────────────────────╯
● node up 3541122 16↑3↓ ● wallet up 1.234567891234 XMR │ form unfocused — 1..4 switch screens, Enter returns      ? help
```

## The line budget — the one gate line that fails

§7a budgets **1 800 lines** across `src/tui/{mod,theme,screens,send,subs}.rs`. The real
counts are 1597 + 201 + 1456 + 772 + 304 = **4330** (`tests.rs`, which §7a does not name, is a
further 585). The overrun is reported rather than closed, and it is not closed
by deleting tests or features:

- Every screen, key, modal and state §7a specifies is implemented. Cutting to 1 800 means
  cutting §7a's own content — the bans view, the label/note prompts, the detail pane, the
  account selector, the QR, or the four tiers — which §2h and §2j forbid more strongly than
  §8 forbids a long file.
- The estimate looks systematically low rather than this implementation looking bloated:
  fproton's TUI, the one §7a scales from ("1 800 rather than fproton's 1 200 because there are
  four screens instead of three"), is itself **2 634** lines against that 1 200 — a 2.2×
  overrun, the same ratio as this one. Four screens, a five-state send machine, four modals, a
  QR canvas and 34 tests at fproton's own density land where this landed.
- Two compaction passes were made before reporting: the model's state is grouped into
  `NodeUi`/`WalletUi`/`LogUi` so the constructor is a handful of `Default`s, and every panel
  renders through five shared helpers (`framed`, `overlay`, `kv`, `para`, `table`) instead of
  open-coding `Block`/`Paragraph` per pane. `cargo fmt` is authoritative and it expands
  argument lists, which is worth ~10% on its own.

If a later phase wants the number met, the honest lever is scope, and it belongs to whoever
owns §7a — not to this agent silently dropping a screen.

## Design deviations, with reasons (§2k)

1. **The Send form owns printable keys; `1 2 3 4` switch screens only after `Esc`.** §7a's
   keymap gives `Tab`/`1 2 3 4` to screens and `Tab`/`Enter`/`p`/`@` to the Send form. Both
   cannot hold while a 95-character base58 address is being typed: `1`, `4`, `q`, `r` and `p`
   all occur in addresses. So the form keeps the keys while it is focused, `Esc` leaves it,
   `Enter` returns to it, and both the form's own hint line and the `?` overlay say so. The
   pty test drives that exact sequence rather than asserting around it.
2. **`p` cycles the priority on its own row; `Ctrl+P` cycles it from anywhere.** Same cause:
   a bare `p` inside an address must reach the address. `Priority` and `Account` are rows in
   the Tab cycle, where `←→` and the digits set them — which is also where §7a's "account
   selector" lives.
3. **The plain confirm modal uses `widgets::modal::Dialog`; the send confirm, the open-wallet
   modal, the address-book picker and the QR are hand-drawn overlays.** §1.4 lists `Dialog`
   for "every modal in §7a", but `Dialog` renders exactly **one centred message row** and an
   **unmasked** input (`ftui-widgets-0.7.0/src/modal/dialog.rs:618-731`). It cannot show the
   seven-field send breakdown with an unelided recipient, and it cannot mask a password
   (§2b.4 needs `TextInput::with_mask`). `Dialog::confirm` is used where it fits — node
   up/down and the log level — with `y` mapped to its primary button, as §7a asks.
4. **`b` shows the bans; the TUI has no ban/unban action.** §7a's keymap gives `b` for "bans"
   and nothing for adding one, while §2i speaks of "the TUI's … ban actions" being disabled
   under a foreign layout. The read-only view is what the keymap specifies; banning stays
   `fmonero node ban` (CLI). `u`, `d` and `L` — the mutators the TUI does have — are refused
   under any layout but our own user unit, with the §2i text, before any RPC.
5. **Field hints are drawn by the TUI, not set through ftui's builder for empty-field text.**
   That builder's name is one of the words `scripts/check.sh`'s §2h grep forbids in shipped
   source, and it would appear on every input here. A grep that fires on correct code is the
   grep an agent relaxes (§2j), so `screens::hinted_input` draws the hint over an empty
   input's cells instead and the gate stays as Phase 1b wrote it.
6. **The Node screen's peers table spans the full width under the two panel columns.** §7a
   asks for host, state, height, pruning seed and rates at the Wide tier; inside a 52 %
   column at 120 cols those five columns cannot all be drawn, and a cut column is worse than
   a moved panel.
7. **The daemon log level is a TUI-side 0→4→0 pointer.** `set_log_level` has no reader —
   `get_info` does not report the level — so the cycle advances when the call is sent. The
   confirm modal names the level it is about to set, and nothing claims to know the daemon's.

8. **The confirm modal owns the keys wherever it is drawn.** A build is not cancelled by
   leaving the Send screen, so `Msg::ActionDone(Built)` can arrive over the Wallet screen; the
   key router therefore sends every key to the modal while one exists. A visible confirm modal
   whose `send`/`Esc` went to the screen behind it is the worst state the money surface could
   be in, and `the_confirm_modal_owns_the_keys_even_on_another_screen` pins it.
9. **The pending banner takes a second row below 120 columns.** It has to carry the txid *and*
   both commands (§7a); on a narrower terminal one row cuts `clear-pending --yes`, which is the
   half a reader needs.

## Two things a reviewer should check next

- The `ProgramSimulator` test counts only the mock's `status: 200` log lines. Every
  `Cmd::task` builds its own `WalletClient`, so each also produces one 401 handshake line
  (§2c); counting those would have made "exactly one `transfer`" read as two and the central
  §2a.3 assertion vacuous. The filter is in `World::count`.
- `zero()` overwrites the password `TextInput`'s buffer on close, which is best-effort inside
  safe Rust: what actually carries §2b.4 is that the `String` never outlives the modal and is
  never copied, because `Action` is neither `Clone` nor `Debug`.
