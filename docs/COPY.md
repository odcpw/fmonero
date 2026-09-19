# fmonero — the words on the screen

Every string a person can read, with what it says today and what it should say. The rule in §1
is the text that becomes **DESIGN.md §2o (v5.6)**; §2 is the vocabulary; §3 is the short list of
places a shell command may still appear in the TUI, each with the reason no in-app path exists;
§4 is the rewrite table, one row per string, grouped by surface.

Three complaints from the owner drive this, verbatim in substance:

- *"The text in the widget isn't clear or useful."*
- *"Most of the text in the TUI is slop too. The explanations need to be in plain English."*
- *"I don't want to manage the CLI; that's what the TUI is for."*

The register being removed, in one list, because naming it is how the next agent recognises it:
internal identifiers shown as words (`remote_node`, `wallet_idle`, `txid_mismatch`); implementation
names in prose (`monero-wallet-rpc`, `monerod`, daemon, unit, systemd, RPC, `get_info`);
architecture explanations where the user's situation belongs (*"fmonero does not talk to this node
— monero-wallet-rpc does"*); section numbers (`§2m.6`); and hints that are shell commands pasted
into a panel the user cannot type into.

The captures this document was written against are real: `fmonero` and `fmonero-mock` at HEAD,
under an `FMONERO_*` sandbox, on a 120x40 pty for the TUI and a pty for the CLI. Nothing in the
owner's configuration was read or changed.

---

## 1. The rule — the exact text of DESIGN.md §2o

> ## 2o. Plain language (v5.6)
>
> **Every string a person can read follows this section.** That is the TUI, the bar widget, the
> CLI's human output, every hint, every warning text, every doctor title and detail, and the
> README's screenshots. `--json` is **exempt and unchanged**: ids, `reason`, `code_name`, warning
> ids and key names are the agent contract (§4, §5, §5c), and agents branch on them.
>
> 1. **Short sentences, one fact each.** A panel row is not a paragraph. If a line needs a
>    semicolon it is two lines.
> 2. **Say what the situation is, then what to do next.** Never how the program is built.
> 3. **Second person is fine.** "You" is shorter than "the user" and clearer than the passive.
> 4. **No section numbers.** `§`-references belong in code comments and in this document.
> 5. **No internal identifier as a word.** A warning id (`wallet_idle`), a reason (`remote_node`),
>    an `Action` name, a unit name, a `code_name`, an RPC method (`get_info`) is never shown as
>    prose. Ids appear in `--json` and nowhere else. A **settings key** (`tui.mouse`,
>    `node.remote`) is not an internal identifier: it is a name the user types, and it may appear.
> 6. **No implementation name except inside a command the user is meant to run.**
>    `monero-wallet-cli --wallet-file …` is a command. "monero-wallet-rpc talks to the node" is
>    prose, and is out.
> 7. **These words do not appear in user-facing prose at all:** daemon · unit · RPC · loopback ·
>    restricted · probe · layout · ssl · systemd · build (of a transaction) · relay · broadcast ·
>    mutator · argv · breaker · nettype · snapshot · high water.
> 8. **One vocabulary** — the table in §2o.2. One name per thing, in every surface.
> 9. **Amounts always carry the unit.** `1.23 XMR`, never `1.23`. A block number reads
>    `block 3 541 122`, never a bare integer in a sentence.
> 10. **An error names the thing the user typed and what a good one looks like.**
>     *"That is not a Monero address. Addresses are 95 characters and start with 4 or 8."*
> 11. **A hint is the last line and never the sentence.** In the CLI it reads `Try: fmonero …`.
>     In the TUI and the widget it names an in-app action (§2o.1).
> 12. **Em-dashes are not prose punctuation here.** Use a period or a comma. `·` stays as a field
>     separator inside dense one-line rows, which is structure rather than prose.
>
> ### 2o.1 No command-line management from the TUI or the widget
>
> The owner: *"I don't want to manage the CLI; that's what the TUI is for."*
>
> **Every hint the TUI or the widget shows names something the user can do where they are** — a
> key, a chip, a button: `Press o to open a wallet`, `[Choose node…]`. Never
> `fmonero wallet open <name>`.
>
> **Anything a user does more than once is an in-app action**, and the list is exhaustive for
> v0.1: start or stop the wallet service · open, create and close a wallet · choose a public node ·
> go back to your own node · get a fresh node list · check a node · check for payments · rescan ·
> change how much your node logs · add an address · rename one · note a payment. A refusal that
> would otherwise print a command instead states the situation and names the key or chip.
>
> A shell command may appear in the TUI **only where no in-app path exists at all**. Those cases
> are listed in `docs/COPY.md` §3 with the reason for each, are phrased `In a terminal: …`, and
> are the last line of their panel. There are four, and three of them are outside fmonero's reach
> by construction (installing a package; reading a seed, which §2b never lets fmonero display;
> restoring from a seed, which §0 cuts from v0.1). The fourth, `wallet sweep`, is a scope decision
> rather than an impossibility, and the line goes when sweep enters the TUI.
>
> **The CLI's own human output keeps its commands.** Its audience is scripts, agents and a person
> already at a prompt, and §9's one-runnable-hint rule is what makes an error actionable there.
> §2o applies to its *prose*; its `Try: fmonero …` lines are the point of it.
>
> ### 2o.2 The vocabulary
>
> | The thing | Say | Never |
> |---|---|---|
> | `monerod` | **node**, and **your node** when it is this machine's | daemon, monerod (outside a command) |
> | a node someone else runs | **public node** | remote node, remote, the remote |
> | `node.mode = local` | **your own node** | local, local mode, `user_unit` |
> | `node.mode = remote` | **using a public node** | remote mode, remote layout |
> | `monero-wallet-rpc` | **the wallet service**, explained once where it first matters: *"the wallet service runs in the background and talks to the node"* | monero-wallet-rpc, the wallet daemon, the unit |
> | a systemd user unit | never named; it is part of *the wallet service* / *your node* | unit, service file, systemd, `.service` |
> | `get_info` against a node | **checked** | probe, probed, sweep-probe |
> | the age of that measurement | **checked 2m ago** | probed 2m ago, last probe |
> | `synchronized` | **synced** | synchronized, in sync, up to date |
> | restricted RPC | **this node only answers basic questions** | restricted, restricted-RPC, gated method |
> | the mempool | **waiting to be mined**, column `WAITING` | txpool, mempool, pool |
> | a transfer row | **payment** | transfer, tx (outside a txid) |
> | `txid` | **id**, or `payment id` where Monero means that | txid in prose |
> | `unlock_time` / confirmations | **confirmations**, `CONFIRMS` in a header | conf |
> | `get_address_book` | **address book** | the book |
> | `do_not_relay` build step | **check**, *"checking the fee"* | build, dry build |
> | `relay_tx` | **send**, *"sent"*, *"the payment has gone out"* | relay, broadcast, on the wire |
> | priority 0..4 | **fee level**: cheapest · normal · fast · fastest | priority, 0=1, per-byte figures |
> | the open wallet | **this wallet** | the wallet2 instance, the open wallet handle |
> | `trusted` / `untrusted` | never shown as a flag; say what it means: *"fmonero will not tell this node which amounts your wallet holds"* | trusted, untrusted |
> | `ssl_support` | never shown | ssl, autodetect, TLS |
> | a subaddress | **address**, and `#N` where the index matters | subaddress |
> | `piconero` | **piconero**, and only beside the XMR figure in the send confirm | pico |
> | `state.json`, `config.toml` | their paths, in the doctor only | state, the settings file |

---

## 2. Where a shell command may still appear in the TUI

Four places, and each one is here because fmonero has no code that could do the job from inside the
app. Every one is phrased `In a terminal: …` and is the last line of its panel.

| Where | The command | Why no in-app path exists |
|---|---|---|
| Node screen, when Monero is missing | `omarchy pkg add monero` | Installing a package is not something fmonero does, and it must never run a package manager on the user's behalf. |
| Seed-backup modal, after a create | `monero-wallet-cli --wallet-file <dir>/<name> --offline`, then `seed` | §2b forbids fmonero from ever displaying a seed: a seed in a terminal's scrollback, a tmux buffer or a screen recording is a total, irreversible loss. The modal says the omission is deliberate. |
| Wallet screen, empty state footnote | `monero-wallet-cli --restore-deterministic-wallet --wallet-file <dir>/<name>` | §0 cuts `wallet restore` from v0.1, so there is no fmonero code to call. |
| Send screen, `all` in the Amount field | `fmonero wallet sweep <address> --dry-run` | **The one exception whose cause is a scope decision, not an impossibility.** §0 keeps sweep out of the TUI in v0.1 and the Send screen prepares exactly one payment. When sweep enters the TUI this line goes. |

Two things §2o.1 implies that v5.6 does **not** build, recorded here so the next agent does not have
to rediscover them:

- **the address book has no in-app "add"**. `wallet book add` is CLI-only, so the empty address-book
  modal states the fact (`Your address book is empty.`) and names no command. An `[a] Add` on that
  modal is the follow-up the rule asks for.
- **`node ban` / `node unban` have no in-app key**. The bans pane therefore reads `No banned peers.`
  and stops there, rather than printing `fmonero node ban <host>` at a user who cannot type it.

**Round 5 closes the other two the same list implied, and both were worse than a printed command,
because both were refusals with no route at all** (S8, S10):

| Where | What it said | What it does now |
|---|---|---|
| Send screen, with an unresolved payment record | one line — *"An earlier payment may still be on its way: 1 of 1 payments were prepared and fmonero never saw them go out."* — no key, no chip, no next step. The only resolution in the product was `fmonero wallet clear-pending --yes`, which §2 above had removed from the banner two rows higher | the refusal names `P` and the Wallet screen; `P` there opens a confirm that states the risk in plain words and runs the same core sequence `wallet clear-pending` runs. The chip `[P] Clear that record` is drawn whenever there is a record, and drawn first |
| Node screen, local mode, no units written | `Press u to start it.`, and `u` answered *"Another program on this machine runs this node. fmonero can read it but not change it."* Nothing ran it, and nothing in the app could write the units | `fmonero has not set up your node yet. Press S to set it up.` — the hero and all three keys, from one sentence — and `S` runs `units::install` |

---

## 3. Rewrite table

`file:line` is where the string lives at HEAD (`52746ba`). "why" is the rule from §1 that the
current text breaks. Rows marked **delete** remove a string outright; rows marked **new** add one
the redesign needs.

### 3.1 TUI — chrome, status bar, help overlay

| file:line | now | becomes | why |
|---|---|---|---|
| `screens.rs:66` | `fmonero — terminal too small (need 40x10)` | `Window too small. Make it at least 40 by 10.` | Rule 12 (em-dash), rule 2: say what to do. |
| `screens.rs:241` | `SEND IN FLIGHT {x} XMR → {dest} {n}/{m} · tx {txid} · fmonero wallet history --limit 5 · fmonero wallet clear-pending --yes` | `SENDING {x} XMR to {dest} · {n} of {m} sent · id {txid} · press 2 for the wallet` | §2o.1: two shell commands in a banner the user cannot type into. |
| `mod.rs:566-578` node badge | `down` · `error` · `remote` · `up` · `syncing` | `not answering` · `problem` · `public node` · `synced` · `syncing` | Rule 8: one vocabulary; `down` is not a word for a node that may be fine and simply unreachable. |
| `mod.rs:592-614` wallet badge | `error` · `idle` · `busy` · `starting` · `down` · `up` · `syncing` | `problem` · `closed` · `scanning` · `starting` · `not running` · `open` · `catching up` | Rule 8. `idle` and `busy` are the program's words for the user's *closed* and *scanning*. |
| `mod.rs:620` | `reading status…` | `checking…` | Rule 5: "status" is the name of a command and a JSON document. |
| `mod.rs:623` | `Remote node · {url}` | `Public node · {addr}` | Vocabulary. |
| `mod.rs:626` | `Node is not running` | `Your node is not running` | Says which node, in the one place a user in remote mode would misread it. |
| `mod.rs:629` | `Synchronized` | `Synced` | Vocabulary. |
| `mod.rs:630` | `Syncing — {p:.1}%` | `Syncing {p:.0}%` | Rule 12; one decimal of a sync percentage is noise on a status line. |
| `mod.rs:658` | `{warning.text} — {warning.hint}` | `{warning.text}  {warning.action}` | §2o.1: the one-line warning rendered the CLI hint. It now renders the in-app action (new `Warning::action`, §5). |
| `screens.rs:1081` | `{w.id} {w.text}` | `{w.text}` | Rule 5: the id was printed as a word in front of every warning. |
| `screens.rs:1085` | `  hint: {w.hint}` | `  {w.action}` | §2o.1. |
| `screens.rs:1056` | pane title ` Warnings ` | ` Notes ` | Two of the three severities are `info`; the pane was shouting at a correct machine. |
| `screens.rs:1062` | `none` | `Nothing to report.` | Rule 1: a word is not a sentence. |
| `mod.rs:139` KEYS | `screens · quit — always, in a field, a modal, the overlay or a running action (quit twice while one runs: cancel, then quit)` | `screens · quit. Always: in a field, in a pop-up, in this help, or while something is running. Press quit twice while something runs and it stops first.` | Rules 7 (modal, action) and 12. |
| `mod.rs:144` KEYS | `the same two — but only when no field has focus: a field keeps every printable key, and Esc unfocuses it` | `the same two, but only when no field has focus. A field keeps every key you type. Esc leaves the field.` | Rule 12, rule 1. |
| `mod.rs:149` KEYS | `refresh now` | `check now` | Vocabulary: `refresh` is the RPC's name; the user is asking fmonero to look. |
| `mod.rs:151` KEYS | `close this overlay, else unfocus the field, else cancel the running action` | `close this help, else leave the field, else stop what is running` | Rule 7 (overlay, action). |
| `mod.rs:155` KEYS | `click tabs, rows, fields and every [key] chip · wheel scrolls the list under the cursor · right-click is Esc` | `click tabs, rows, fields and every [key] chip. The wheel scrolls the list under the pointer. Right-click is Esc.` | Rule 1. |
| `screens.rs:2125` | `off — fmonero settings set tui.mouse true` | `off — turn on tui.mouse in your settings` | §2o.1; rule 5's settings-key carve-out keeps the name. |
| `mod.rs:158` KEYS | `Node: start · stop the node (confirm modal; remote mode: disabled)` | `Node: start · stop your node. Not available while you use a public node.` | Rules 7 (modal) and 8. |
| `mod.rs:160` KEYS | `cycle the daemon log level (confirm modal)` | `change how much your node writes to its log (it asks first)` | Rule 7 twice. |
| `mod.rs:162` KEYS | `Node: probe the listed nodes · refresh the list from monero.fail` | `Node: check the listed nodes · get a fresh list` | Vocabulary (probe). |
| `mod.rs:166` KEYS | `Node: use the selected node (confirm) · add an address · back to the previous node` | `Node: use the selected node (it asks first) · add an address · go back to the last one` | Rule 7. |
| `mod.rs:170` KEYS | `Wallet: open a wallet (modal) · create a wallet (modal) · close the open wallet` | `Wallet: open a wallet · create one · close the open wallet` | Rule 7 (modal); the parentheses said nothing a user needed. |
| `mod.rs:174` KEYS | `Wallet: start the wallet daemon (or stop it when idle and nothing is open)` | `Wallet: start the wallet service, or stop it when nothing is open` | Vocabulary (daemon, idle). |
| `mod.rs:177` KEYS | `Wallet: new subaddress · label the selected one` | `Wallet: new address · rename the selected one` | Vocabulary (subaddress, label). |
| `mod.rs:178` KEYS | `Wallet: edit the selected transfer's note` | `Wallet: edit the note on the selected payment` | Vocabulary (transfer). |
| `mod.rs:179` KEYS | `Wallet: refresh · show the address as a QR` | `Wallet: check for payments · show the address as a QR code` | Vocabulary; `QR` alone is an abbreviation with no noun. |
| `mod.rs:180` KEYS | `Wallet: filter rows · cycle the type filter` | `Wallet: filter rows · change which kinds show` | Rule 2. |
| `mod.rs:186` KEYS | `next/previous field · build  (Tab no longer moves between fields; Esc unfocuses, Enter from outside returns to the form)` | `next/previous field · check it. Tab leaves the form. Esc leaves the field; Enter goes back into it.` | Vocabulary (build), rule 1, rule 7 (unfocus). |
| `mod.rs:190` KEYS | `move between a modal's controls · advance, then submit on the last one` | `move between a pop-up's fields · next, then confirm on the last one` | Rule 7 (modal, submit). |
| `mod.rs:194` KEYS | `cycle priority (on its row, or from anywhere) · address book` | `change the fee level (on its row, or from anywhere) · address book` | Vocabulary (priority). |
| `mod.rs:198` KEYS | `on the Priority or Account row: change it (accounts come from the wallet)` | `on the Fee or Account row: change it` | Vocabulary; the parenthetical explained an implementation detail. |
| `mod.rs:202` KEYS | `not in the TUI — run: fmonero wallet sweep <addr> --dry-run` | `not in the TUI. To send everything, in a terminal: fmonero wallet sweep <address> --dry-run` | §2o.1's fourth exception, phrased as one. |
| `mod.rs:206` KEYS | `the confirm modal's --yes; Esc discards, and a relay cannot be cancelled` | `typing the word is how you confirm. Esc discards. Once it is sent it cannot be taken back.` | Rules 5 (`--yes` as a concept), 7 (modal, relay). |
| `mod.rs:219` | `filter by txid, address or note` | `filter by id, address or note` | Vocabulary. |
| `mod.rs:220` | `search the log` | unchanged | Already plain. |
| `screens.rs:2123` | `{meaning}  (remote mode: disabled)` | `{meaning}  (not while you use a public node)` | Vocabulary. |
| `screens.rs:2108` | ` Keys — any key closes ` | unchanged | Already plain. |
| **new** `mod.rs` KEYS | — | `Node: l` → `use your own node instead (it asks first)` | §2o.1: the toast that used to print `fmonero node local --yes` needs an in-app destination. |

### 3.2 TUI — Node screen

| file:line | now | becomes | why |
|---|---|---|---|
| `screens.rs:702` | `● Remote node · {url}` | `● Public node · {addr}` | Vocabulary. |
| `screens.rs:711-714` | `trust      untrusted · ssl autodetect` | **delete** | Two banned words and a row that answers nothing a user asked. What replaces it is §3.2's new block below: fmonero never marks a public node trusted, so there is no choice to display. |
| `screens.rs:734` | `fmonero does not talk to this node — monero-wallet-rpc does. Its own sync state cannot be seen from here.` | **delete** | Rule 2 in its purest form: an architecture lecture in the place the user's answer belongs. |
| `screens.rs:865`, `:942` | `not available with a remote node` (the Net and Mempool panes) | **delete both panes in this mode** | Two panes whose entire content was a sentence about what they cannot show. Replaced by one pane of what *is* known. |
| **new** Node screen, public-node mode | — | pane ` This node `: `block 3 765 346` / `synced` (or `syncing`) / `answered in 240 ms` / `checked 2m ago` / `[f] Choose another node` | The four facts the cached check already holds (§2n.4), plus the way out. This is the whole of decision 3. |
| **new** same pane, never checked | — | `Not checked yet.` + `[p] Check this node` | The honest empty state, with the action beside it. |
| **new** same pane, last check failed | — | `No answer when checked 2m ago.` + `[f] Choose another node` | Rule 2: state, then next step. |
| `screens.rs:721-728` | `wallet     {height} · last refresh {age}` | `this wallet  block {height} · checked {age} ago` | Vocabulary (refresh); rule 9 (a block number says `block`). |
| `screens.rs:749` | `Monero is not installed. omarchy pkg add monero, then fmonero setup --units` | `Monero is not installed on this machine.` + `In a terminal: omarchy pkg add monero` | §3's first exception, phrased as one; the `setup --units` half is fmonero's own work and happens on demand. |
| `screens.rs:751` | `Node is not running — press u to start it` | `Your node is not running. Press u to start it.` | Rule 12; says which node. |
| `screens.rs:768` | row label `sync` | `blocks` | The row shows a bar plus `height / target`; `sync` named the concept, not the row. |
| `screens.rs:772` | row label `eta` | `done in` | Rule 5: an abbreviation of a term of art. |
| `screens.rs:792-797` | `version    {v} · fork v{n} · {net}` | `version    {v} · {net}` | The hard-fork version is an internal number no user acts on. It stays in `node info --json`. |
| `screens.rs:802` | `Node owned by the system unit — read-only (u/d/L disabled)` | `Another program on this machine runs this node. fmonero can read it but not change it.` | Rules 7 (unit), 12, and 2. |
| `screens.rs:809` | `restricted RPC: peers, net stats and bans have no source` | `This node only answers basic questions, so peers and traffic are not shown.` | Vocabulary. |
| `screens.rs:817` | `reading status…` | `checking…` | Rule 5. |
| `screens.rs:871` | `not available on a restricted node ({method})` | `This node does not answer that.` | Vocabulary, and the method name was rule 5. |
| `screens.rs:881` | `reading the node…` | `checking the node…` | Consistency with every other progress phrase. |
| `screens.rs:887` | `no bans · fmonero node ban <host>` | `No banned peers.` | §2o.1; see §2 for the missing in-app path. |
| `screens.rs:906` | `no connections yet` | `No peers yet.` | Vocabulary (connections). |
| `screens.rs:924` | header `HOST STATE HEIGHT PRUNE AVG ↓/↑` | `HOST STATE BLOCK KEEPS AVG ↓/↑` | `HEIGHT` → `BLOCK`; `PRUNE` was the pruning seed, which means nothing to a reader. |
| `screens.rs:960` | `node unreachable` | `Your node is not answering.` | Rule 1. |
| `screens.rs:972` | row label `limits` | `speed limits` | Rule 8: a bare `limits` beside two rates is ambiguous. |
| `screens.rs:986` | `the rx sparkline fills as samples arrive` | `The graph fills in as it measures.` | Rule 6 (`rx`, `sparkline`, `samples`). |
| `screens.rs:1015` | row label `pool`, value `{n} txs · {b} · oldest {age}` | `waiting`, `{n} transactions · {b} · oldest {age}` | Vocabulary (pool, txs). |
| `screens.rs:1024` | row label `pool fees` | `their fees` | Vocabulary. |
| `screens.rs:1037` | `reading the transaction pool…` | `checking what is waiting…` | Vocabulary. |
| `screens.rs:1042-1049` | `fee/byte   1:0.00000002  2:0.00000008  3:0.00000032  4:0.000004` | `fee levels  cheapest 0.00000002 · normal 0.00000008 · fast 0.00000032 · fastest 0.000004  XMR per byte` | The four numbers were labelled by their priority index. Names are what the Send screen offers, so the two screens now agree. |
| `screens.rs:443` | pane title ` Public nodes ` | unchanged | Already right, and it is the one title the picker needed. |
| `screens.rs:456` | `No public-node list yet.` | `No list of public nodes yet.` | Rule 6: a hyphenated compound built out of a field name. |
| `screens.rs:466` | `Fetching tells monero.fail your IP address. Nothing else is contacted.` | `Getting the list tells monero.fail your IP address. Nothing else is contacted.` | Plain verb; the sentence is otherwise exactly right and stays. |
| `screens.rs:487-499` | columns `address health height checked ms node height synced` | `address · reliability · block · last seen · ping · block · synced` | `ms` was a unit with no noun; `node height` and `height` were distinguished only by column order, which the two group headings already do. |
| `screens.rs:496` | group heading `probed here` | `checked here` | Vocabulary. |
| `screens.rs:523` | `{p}% of {c}` | `{p}% of {c} checks` | Rule 9's sibling: a bare count invites the wrong comparison, which is why the sample size is there at all. |
| `screens.rs:511` | marker ` own` | ` yours` | Plainer, and one character shorter. |
| `screens.rs:513` | marker ` web` | **delete**, and drop the column | `web_compatible` is a CORS flag for browser wallets. It decides nothing here and stays in `node list --json`. |
| `screens.rs:559` | probe `reason` cell, e.g. `timeout`, `http_502` | `no answer` · `too slow` · `refused` · `bad answer` | Rule 5: the reason string is the machine discriminator. |
| `screens.rs:648` | `{n} nodes · list {age} · {m} skipped (no port) · {k} bad · source {src}` | `{n} nodes from monero.fail · list {age} old · {m} skipped` | Four numbers, two of which named parse failures. The two skip counts stay in `--json`; one total is what a reader uses. |
| `screens.rs:671` | chip `[p] Probe` | `[p] Check these` | Vocabulary. |
| `screens.rs:674-678` | chips `[f] Refresh list` / `[f] Fetch the list` | `[f] Get a fresh list` / `[f] Get the list` | Vocabulary (refresh), and the two states stay distinguishable. |
| `screens.rs:681` | `[Enter] Use this node` | unchanged | Already right. |
| `screens.rs:682` | `[a] Add a node` | unchanged | Already right. |
| `screens.rs:683` | `[H] Previous node` | `[H] Back to the last one` | Rule 2: "previous node" is a state, not an action. |
| **new** picker chip | — | `[l] Use your own node` | §2o.1: the destination the toast used to name as a command. |
| `screens.rs:697` | `status failed: {e}` | `Could not read the status: {e}` | Rule 1. |
| `screens.rs:698` | `press r to try again` | `Press r to try again.` | Sentence case and a full stop, which every other line now has. |
| `screens.rs:830-831` | chips `[u] Start node` / `[d] Stop node` | `[u] Start your node` / `[d] Stop your node` | Says which node, in the mode where a user has two in mind. |
| `screens.rs:852-855` | ` Bans  {n} · b: peers ` / ` Peers  {n} · b: bans ` | unchanged | Terse, but every token is a real word or a key. |

### 3.3 TUI — Wallet screen

| file:line | now | becomes | why |
|---|---|---|---|
| `screens.rs:1172` | `No wallet is open.` | unchanged | The one line on the old screen that was already right. |
| `screens.rs:1162` | `Open wallet…      pick one of the {n} wallets in {dir}` | `Open wallet…      choose one of the {n} in {dir}` | Shorter; the chip already says "wallet". |
| `screens.rs:1183` | `Create wallet…    a new wallet, on this machine, in that directory` | `Create a wallet    (a new one, kept on this machine)` | Rule 1: three clauses for one fact. X36 then put the chip in one style with `[o] Open a wallet` — an article, no ellipsis, the gloss in brackets — and this row follows the code (round-4 verification). |
| `screens.rs:1188` | `Start the wallet daemon   (not needed first — opening or creating one starts it)` | **`Stop the wallet service  (nothing is open, so nothing is interrupted)`, and nothing at all in the other states** (W15, then round 4 iteration 3: a chip whose own gloss says not to press it is not drawn; `W` stays in the keymap and the `?` overlay explains both halves) | Vocabulary, rule 12 — and the label has to follow the key: §7a.5 says `W` stops the service when nothing is open, and in exactly that state the chip said `Start` and pressing it stopped it. One predicate (`Tui::wallet_daemon_key_stops`) decides both. |
| `screens.rs:1199` | `wallet daemon: {phrase}` | `wallet service: {phrase}` | Vocabulary. |
| `screens.rs:1217` | `starting…` | unchanged | Already plain. |
| `screens.rs:1220` | `reading status…` | `checking…` | Rule 5. |
| `screens.rs:1223` | `up` | `running` | Vocabulary: `up` is a systemd word. |
| `screens.rs:1229` | `idle · starts when you open a wallet` | `not running yet. It starts when you open or create a wallet.` | `idle` is the program's word. The sentence is the whole point of the row, so it gets to be a sentence. |
| `screens.rs:1231` | `not running` | unchanged | Already plain. |
| `screens.rs:1241-1246` | `node: remote · {url} · untrusted` | `node: public node {addr}` | Vocabulary, and the trust flag is deleted everywhere (§3.2). |
| `screens.rs:1250-1255` | `node: local · {url} · {up \| down \| —}` | `node: your own node {addr} · {running \| not running \| —}` | Vocabulary. |
| **new** Wallet screen, wallet behind | — | `Syncing {p}% · {n} blocks left.` and under it `This keeps going in the background. You can close this window.` | Decision 7. The wallet service keeps refreshing after the TUI quits, and the TUI never said so. |
| `screens.rs:1285` | `   ↑ locked for {b} blocks (≈ {m} min)` | `   {x} XMR locked for about {m} min` | Rule 9 (the amount was missing), and blocks are the mechanism, minutes are the answer. |
| `screens.rs:1294` | `   {n} blocks behind` | `   {n} blocks to go` | Rule 2: "behind" is a judgement, "to go" is a count that shrinks. |
| `screens.rs:1305` | ` Subaddresses — Enter copies · a new · l label ` | ` Your addresses — Enter copies · a new · l rename ` | Vocabulary (subaddress, label), rule 12. |
| `screens.rs:1310` | `loading…` | `checking…` | One progress word everywhere. |
| `screens.rs:1360` | header `#  LABEL  ADDRESS ✓=used  BALANCE` | `#  NAME  ADDRESS (✓ used)  BALANCE` | Vocabulary; the legend reads as a caption rather than an assignment. |
| `screens.rs:1420` | ` History  {n} rows · t: {kind} ` | ` Payments  {n} · t: {kind} ` | Vocabulary; `rows` counted the table, not the payments. |
| `screens.rs:1445` | header `WHEN TYPE AMOUNT CONF LOCKED est.` | `WHEN WHAT AMOUNT CONFIRMS LOCKED` | Rule 5 (`CONF`); `est.` was carried by a column whose every value is an estimate. |
| `screens.rs:1452` | `history is unavailable: {e}` | `Could not read your payments: {e}` | Rule 1. |
| `screens.rs:1458` | `No rows. R refreshes the wallet, / filters, t cycles the type.` | `Nothing yet. Press R to check, / to filter, t to change what shows.` | Vocabulary, and the keys are named as keys. |
| `screens.rs:1492` | `cached: confirmations unknown (no daemon call)` | `From the saved copy: confirmation counts are not known.` | Rule 7 (daemon), rule 1. |
| `screens.rs:1515` | ` Transfer — n edits the note · Esc closes ` | ` Payment — n edits the note · Esc closes ` | Vocabulary. |
| `screens.rs:1532` | detail label `txid` | `id` | Vocabulary. |
| `screens.rs:1534` | detail label `type`, value `{t} · height {h} · {when}` | `what`, `{t} · block {h} · {when}` | Rule 9. |
| `screens.rs:1551-1556` | `confirms   {c} · locked (est.) {v}` | `confirms   {c} · locked {v}` | Every locked figure is an estimate; the column header says so once. |
| `screens.rs:1560` | detail label `subaddr`, value `{major}/{minor}` | `address #`, value `{minor} (account {major})` | Vocabulary, and `major/minor` is the wire format. |
| `screens.rs:1563` | detail label `unlock` | `unlocks at` | Rule 2. |
| `screens.rs:1564` | detail label `payment id` | unchanged, and shown only when it is non-empty | It is a real Monero term where it has a value; an always-present empty row taught it as noise. |
| `screens.rs:1565` | detail label `double sp` | `double spend` | Rule 5: a truncation. |
| `screens.rs:1570` | `  — (an incoming transfer has none)` | `  none (this payment came in)` | Vocabulary, rule 12. |
| `screens.rs:1115-1117` | chips `[R] Refresh` `[Q] QR` `[x] Close` | `[R] Check for payments` · `[Q] QR code` · `[x] Close wallet` | Vocabulary; each chip now names its object. |
| `screens.rs:1620` | `no log yet at {path} — the node writes it once it runs` | `No log yet. Your node writes it once it runs.` | Rule 12; the path is already in the pane title. |
| `screens.rs:1589-1593` | ` Log  follow {on \| off}  {path} ` | unchanged | Already plain. |

### 3.4 TUI — Send screen

| file:line | now | becomes | why |
|---|---|---|---|
| `send.rs:35` | `sweep is not available in the TUI — run: fmonero wallet sweep <addr> --dry-run` | `Sweep is not in the TUI yet. To send everything, in a terminal: fmonero wallet sweep <address> --dry-run` | §3's fourth exception, phrased as one; rule 12. |
| `send.rs:101` | `To` / `address, or @label from the address book` | `To` / `an address, or @name from your address book` | Vocabulary (label). |
| `send.rs:102` | `Amount` / `0.5   ·   250000000000p` | `Amount` / `0.5 XMR` | Rule 9. The piconero form stays accepted and stays documented in `--help`; as a placeholder it taught a reader that amounts look like that. |
| `send.rs:103` | `Note` / `optional note, kept in the wallet` | `Note` / `optional, kept with this wallet` | Rule 1. |
| `send.rs:164` | `enter a recipient and an amount` | `Enter who to pay and how much.` | Rule 2, plain nouns. |
| `send.rs:168` | `@ needs an address-book label after it` | `Type a name after @.` | Vocabulary, rule 10. |
| `send.rs:178` | `0 is not an amount to send` | `0 XMR is not an amount.` | Rule 9. |
| `send.rs:180` | `{dest} · {x} XMR · Enter builds it` | `{dest} · {x} XMR · Enter checks the fee` | Vocabulary (build), and it says what Enter will actually do. |
| `send.rs:241` | `form unfocused — 1..4 switch screens, Enter returns` | `You left the form. 1-4 change screens, Enter goes back.` | Rule 7 (unfocused), rule 12. |
| `send.rs:344` | `discarded — nothing was broadcast` | `Discarded. Nothing was sent.` | Vocabulary, rule 12. |
| `send.rs:358`, `:712` | `this plan's total overflows u64 — Esc discards it` | `That total is too large to add up. Esc discards it.` | Rule 6: `u64` is an implementation type. |
| `send.rs:361` | `type "send" to broadcast` | `Type "send" to confirm.` | Vocabulary. |
| `send.rs:466-488` | `Priority 0=1  1 20000p  2 80000p  3 320000p  4 4000000p` | `Fee  [cheapest] [normal] [fast] [fastest]` | The worst line on the money screen: an alias notation and four per-byte piconero figures, none of which a person chooses between. The chips stay individually clickable (§7a.2). |
| `send.rs:504` | ` p · Ctrl+P` | unchanged | Keys. |
| `send.rs:512-519` | `Account ◂ {n} ▸ ←→ changes it` | unchanged | Already plain. |
| `send.rs:542` | `checking the address…` | unchanged | Already right, and it is the model for every other progress line. |
| `send.rs:544` | `building with do_not_relay — nothing is broadcast yet · Esc cancels` | `Checking the fee. Nothing has been sent yet. Esc cancels.` | Rule 6 (`do_not_relay`), vocabulary, rule 12. |
| `send.rs:548` | `broadcasting — this one cannot be cancelled` | `Sending. This cannot be stopped.` | Vocabulary, rule 12. |
| `send.rs:551` | `sent · {txid}` | unchanged | Already right. |
| `send.rs:557-562` | `account {n} · ↑↓ fields · ⇥ screens · @ picks from the address book · Esc leaves the form` | `account {n} · ↑↓ fields · ⇥ screens · @ address book · Esc leaves the form` | Shorter by four words with nothing lost. |
| `send.rs:606` | `known only after the build` | `known after the check` | Vocabulary (build). |
| `send.rs:628` | `the book is empty — fmonero wallet book add <address> <description>` | `Your address book is empty.` | §2o.1, vocabulary (the book). |
| `send.rs:636` | ` Address book — Enter picks ` | unchanged | Already plain. |
| `send.rs:658` | ` Confirm — this broadcasts a transaction ` | ` Confirm — this sends money ` | Vocabulary, and it says what is at stake in three words. |
| `send.rs:700-703` | `Account {n}  Priority {p}  Weight {w}` | `Account {n}  Fee level {name}` | Vocabulary; the transaction's weight is an internal number and stays in `--json`. |
| `send.rs:714` | `Type "send" and press Enter to broadcast · Esc to discard` | `Type "send" and press Enter to send · Esc discards` | Vocabulary. |
| `send.rs:673-696` | `To / Amount / Fee / Total`, each `{x} XMR  ({p} pico)` | unchanged, with `piconero` spelled out | Rule 5 (`pico` is a truncation). The confirm modal is the one place the exact integer earns its room. |

### 3.5 TUI — modals

| file:line | now | becomes | why |
|---|---|---|---|
| `screens.rs:112` | `no wallets in {dir}` | `No wallets in {dir}.` | Sentence. |
| `screens.rs:113` | `fmonero wallet create <name> --yes` (the open modal's empty hint) | `Press c to create one.` | §2o.1: a command inside a modal, for a thing one key does. |
| `screens.rs:120` | ` Open a wallet — Enter · Esc cancels ` | ` Open a wallet ` | `Enter` alone named a key with no verb; W18 then moved the keys out of the title altogether, because `list_modal` states them once in its own last line (round-4 verification: the title the code renders). |
| `screens.rs:1675` | `Enter · Esc cancels` | `Enter chooses · Esc cancels` | Same reason. |
| `screens.rs:1792-1815` | fields `name / password / again / language` | `name / password / repeat / seed language` | `again` is not a label; `language` alone reads as the UI's language. |
| `screens.rs:1826` | `↑↓ fields · Enter next, then create · Esc discards` | unchanged | Already plain. |
| `screens.rs:1830` | `Tab is held here — Esc to discard first` | `Tab is off here. Press Esc to discard first.` | Rule 12; "held" described the dispatcher. |
| `screens.rs:1852` | ` Back up your seed ` | unchanged | Already right. |
| `screens.rs:1873` | ` Use this node ` | unchanged | Already right. |
| `screens.rs:1881` | `untrusted — a public node is never trusted (it would receive this wallet's own unspent amounts inside a request)` | `fmonero will not tell this node which amounts your wallet holds.` | The fact, not the flag and not the mechanism. |
| `screens.rs:1887` | `· the wallet unit will be re-rendered and restarted` | `· the wallet service will restart` | Rules 6 and 7. |
| `screens.rs:1893` | `· {name} is open and will be closed — you must reopen it` | `· {name} will be closed. You will be offered it again.` | Rule 12, and the recovery is real: the open modal comes up afterwards. |
| `screens.rs:1898` | `· this node will see this machine's IP address and the timing of every request` | `· this node will see your IP address and when you ask it anything` | Rule 3; "the timing of every request" is precise and unreadable. |
| `screens.rs:1903` | `y to confirm · Esc cancels` | unchanged | Already plain. |
| `screens.rs:1918-1926` | ` Add a node ` / `address  ` | unchanged | Already plain. |
| `screens.rs:1941` | `[http[s]://]host:port — the port is required` | `host:port, and the port is required` | Rule 12; the optional-scheme grammar is in `--help`. |
| `screens.rs:1945` | `for a node you run yourself: fmonero node remote <addr> --trusted --yes` | `Nodes you add here are treated as public.` | §2o.1. The LAN-trusted case stays a documented CLI capability and is not advertised here. |
| `screens.rs:1950` | `Enter adds it · Esc cancels` | unchanged | Already plain. |
| `screens.rs:1965` | ` Before contacting anyone ` | unchanged | Already right, and it is the best title in the program. |
| `screens.rs:1973` | `Fetching the list tells monero.fail your IP address.` | `Getting the list tells monero.fail your IP address.` | Plain verb. |
| `screens.rs:1977` | `Probing tells each node you probe. Nothing else is contacted.` | `Checking a node tells that node too. Nothing else is contacted.` | Vocabulary. |
| `screens.rs:1982` | `monero.fail's health figures are its measurements from its own vantage point — not a recommendation, and not this machine's.` | `monero.fail's figures are its own measurements. They are not a recommendation, and not yours.` | Rule 12, rule 1. |
| `screens.rs:1987` | `Enter continues · Esc cancels` | unchanged | Already plain. |
| `screens.rs:2018` | ` Receive — Esc closes ` | unchanged | Already plain. |
| `screens.rs:2062` | `terminal too small for a QR (needs {w}x{h}) — the address is below` | `Window too small for a QR code. The address is below.` | Rule 12; the exact size helps nobody who is reading it. |
| `screens.rs:2131` | ` Start the node ` / `systemctl --user start · y confirms` | ` Start your node ` / `y confirms · Esc cancels` | Rule 6 in the one modal that printed a command it was about to run. |
| `screens.rs:2133` | ` Stop the node ` / `stop_daemon then systemctl --user stop · y confirms` | ` Stop your node ` / `y confirms · Esc cancels` | Same. |
| `screens.rs:2137-2143` | ` Daemon log level ` / `set the daemon log level to {n} · y confirms` | ` How much your node logs `, **as an overlay** / `Your node will write level {n} to its log file.` then three lines: `0 is almost silent` · `1 is the usual amount, and is what fmonero sets up` · `4 writes everything, and fills the disk fast` / `y to confirm · Esc cancels` | Vocabulary (daemon), rule 2: the scale was never explained. **v5.6.3 (X4): the one-row form specified here never fitted.** `Dialog::confirm`'s box is a fixed 58 columns and three of the five messages were 60 characters, so `y to confirm · Esc cance` was on screen at 120x32 and at 150x40 alike with six blank rows under the cut. It is an overlay now, like the three confirms W2 and V4 moved, and the scale gets a line each. |
| `screens.rs:2255` | ` Label subaddress {i} ` | ` Name address {i} ` | Vocabulary. |
| `screens.rs:2272` | ` Note for this transfer ` | ` Note for this payment ` | Vocabulary. **Shipped in v5.6.3 (X8)**: this row was written in the v5.6 round and the code kept `transfer` through two verifications, because rule 7's word list never carried `transaction` or `transfer` and only a reading could catch it. |
| `session.rs:94-101` (core, rendered in the seed modal and the CLI footer) | `Back up your seed BEFORE funding this wallet:` / `# close it first — the wallet daemon has it open:  fmonero wallet close` / `monero-wallet-cli --wallet-file {dir}/{name} --offline` / `# then type: seed` | `Back up your seed before you put money in this wallet. fmonero never shows it.` / `With the wallet closed, in a terminal:` / `  monero-wallet-cli --wallet-file {dir}/{name} --offline` / `  then type: seed` | Rule 7 (daemon), §2o.1 for the `fmonero wallet close` line: the TUI prepends `Close this wallet first — press x.` and the CLI prepends `Try: fmonero wallet close`. |

### 3.6 TUI — toasts and inline refusals (`mod.rs`)

| file:line | now | becomes | why |
|---|---|---|---|
| `mod.rs:2120` | `remote node mode — run: fmonero node local --yes` | `You are using a public node. Press f for the list, or l to use your own node.` | The single worst §2o.1 violation: the refusal for four keys was a shell command. |
| `mod.rs:2125` | `this monerod is not fmonero's to mutate — sudo systemctl <verb> monerod` | `Another program runs this node. fmonero can read it but not change it.` | Rules 6, 7 and §2o.1 — and it told the user to run `sudo`. |
| `mod.rs:831` | `{dest} is not valid` | `That is not a Monero address.` | Rule 10; echoing a 95-character string into a toast is unreadable. |
| `mod.rs:836` | `{dest} is a {net} address; this wallet is {net2}` | `That address is for {net}. This wallet is {net2}.` | Rule 1. |
| `mod.rs:872` | `{label} already — Esc cancels it` | `Already {label}. Esc cancels it.` | Rule 12. |
| `mod.rs:907` | `nothing running to cancel` | `Nothing is running.` | Sentence. |
| `mod.rs:912` | `{label} cannot be cancelled — the transaction is already on the wire` | `{label} cannot be stopped. The payment has already gone out.` | Rule 6 (on the wire), rule 12. |
| `mod.rs:916` | `cancelled: {label}` | `Stopped: {label}` | Vocabulary: "cancelled" and "stopped" were used for the same act on neighbouring lines. |
| `mod.rs:972` | `fetching the list from monero.fail…` | `Getting the list from monero.fail…` | Plain verb. |
| `mod.rs:996` | `no nodes to probe — press f to fetch the list` | `No nodes to check. Press f to get the list.` | Vocabulary, rule 12. |
| `mod.rs:999` | `probing {n} nodes · Esc to stop` | `checking {n} nodes · Esc to stop` | Vocabulary. |
| `mod.rs:1009` | `select a node first (↑↓ or click), then Enter` | unchanged | Already right. |
| `mod.rs:1016` | `this is the node you are using` | unchanged | Already right. |
| `mod.rs:1036` | `no previous node in this session` | `No earlier node to go back to.` | Rule 6 (session). |
| `mod.rs:1109`, `:1385` | `Esc to discard first` | unchanged | Already right. |
| `mod.rs:1175` | `wl-copy is not installed — cannot copy` | `Cannot copy: wl-copy is not installed.` | Rule 2: the situation first; rule 12. |
| `mod.rs:1179` | `copied to the clipboard` | `Copied.` | One word does it. |
| `mod.rs:1180` | `wl-copy failed: {e}` | `Could not copy: {e}` | Rule 6. |
| `mod.rs:1193` | `Esc cancels it, or press quit again to cancel and quit` | unchanged | Already plain. |
| `mod.rs:1195` | `a broadcast is on the wire; press quit again to leave it running` | `The payment has already gone out. Press quit again to leave it running.` | Vocabulary, rule 6. |
| `mod.rs:1197` | `an action is running — {extra}` | `Something is running. {extra}` | Rule 7 (action), rule 12. |
| `mod.rs:1561`, `:1673` | `no address to show yet` | unchanged | Already plain. |
| `mod.rs:1662` | `new subaddress {i}` | `new address {i}` | Vocabulary. |
| `mod.rs:1811` | `daemon log level {n}` | `your node now logs at level {n}` | Vocabulary. |
| `mod.rs:1816`, `:2707` | `cancelled` | `Cancelled.` | Sentence. |
| `mod.rs:1834` | `label saved` | `name saved` | Vocabulary. |
| `mod.rs:2002` | `cancelled — the node was not changed` | `Cancelled. The node was not changed.` | Rule 12. |
| `mod.rs:2087` | `a wallet name may not contain \`/\` — the daemon refuses it` | `A wallet name cannot contain "/".` | Rule 7 (daemon): whose refusal it is does not matter to the person typing. |
| `mod.rs:2097` | `{name} already exists in wallet.dir` | `{name} already exists in {dir}.` | Rule 5: `wallet.dir` was printed where its value belonged. |
| `mod.rs:2104` | `an empty password would leave the wallet unencrypted — run: fmonero wallet create <name> --allow-empty-password --yes` | `A password is required. An empty one would leave this wallet unencrypted.` | §2o.1. The escape hatch stays in the CLI, which is the deliberate place for it. |
| `mod.rs:2243`, `:2248` | `no subaddress selected` | `No address selected.` | Vocabulary. |
| `mod.rs:2265` | `no transfer selected` | `No payment selected.` | Vocabulary. |
| `mod.rs:2594` | `{label}: {e.text} ({e.code_name})` | `{label}: {e.text}` | Rule 5: the `code_name` was shown as a word. |
| `mod.rs:2613` | `built — nothing is broadcast yet` | `Checked. Nothing has been sent yet.` | Vocabulary, rule 12. |
| `mod.rs:2642` | `fmonero never displays a seed — a scrollback is not a safe place for one.` | `fmonero never shows your seed. A terminal's scrollback is not a safe place for one.` | Rule 12; "a scrollback" needs its noun. |
| `mod.rs:2650` | `remote mode: the node decided this wallet's restore height — keep it answering until the first refresh finishes.` | `This public node set where the wallet starts looking. Keep it selected until the first check finishes.` | Vocabulary three times, rule 12. |
| `mod.rs:2671` | `probed {n} · {reached} answered` | `checked {n} · {reached} answered` | Vocabulary. |
| `mod.rs:2692` | `now on {address} · {name} was closed — reopen it` | `Now on {address}. {name} was closed. Press o to reopen it.` | §2o.1 (a bare instruction with no key), rule 12. |
| `mod.rs:2709` | `{e.text} ({e.code_name}) — {e.hint}` | `{e.text}` on one line, `{e.action}` on the next | Rule 5 and §2o.1: it rendered both an id and a command. |
| `mod.rs:2211`, `:2609`, `:2621`, `:2632`, `:2662`, `:2697`, `:1723`, `:2052`, `:2084`, `:2100`, `:2231`, `:2235`, `:2019`, `:1840` | `wallet closed` · `{name} is open` · `sent · {first}` · `{name} created and open` · `{n} public nodes` · `now on {address}` · `following` / `follow off` · `nothing was contacted` · `a wallet name is required` · `the two passwords do not match` · `a wallet is open — press x to close it first` · `something is already running` · `added {address}` · `note saved` | unchanged, except `a wallet is open — press x to close it first` → `A wallet is open. Press x to close it first.` | Fourteen strings reviewed; thirteen were already plain, one carried an em-dash. |

### 3.7 TUI — action labels in the status bar (`subs.rs`)

| file:line | now | becomes | why |
|---|---|---|---|
| `subs.rs:141` | `refreshing the wallet` | `checking for payments` | Vocabulary. |
| `subs.rs:142` | `building the transaction` | `checking the fee` | Vocabulary, and it is what the step does. |
| `subs.rs:143` | `broadcasting` | `sending` | Vocabulary. |
| `subs.rs:145` | `starting the wallet daemon` | `starting the wallet service` | Vocabulary. |
| `subs.rs:146` | `stopping the wallet daemon` | `stopping the wallet service` | Vocabulary. |
| `subs.rs:147` | `fetching the node list` | `getting the node list` | Plain verb. |
| `subs.rs:148` | `probing {n} nodes` | `checking {n} nodes` | Vocabulary. |
| `subs.rs:138-140`, `:144`, `:149` | `starting the node` · `stopping the node` · `opening {name}` · `creating {name}` · `switching to {address}` | unchanged | Already plain. |
| `subs.rs:269` | `the node was already running` | `your node was already running` | Says which node. |
| `subs.rs:271` | `node up in {n}s` | `your node started in {n}s` | Rule 7 (`up`). |
| `subs.rs:276` | `node stopped` | `your node is stopped` | Same. |
| `subs.rs:289-294` | `refreshed {n} blocks · received money` | `checked {n} blocks · money came in` | Vocabulary. |
| `subs.rs:351` | `the wallet daemon is up` | `the wallet service is running` | Vocabulary. |
| `subs.rs:353` | `the wallet daemon was already up` | `the wallet service was already running` | Vocabulary. |
| `subs.rs:358` | `the wallet daemon is stopped` | `the wallet service is stopped` | Vocabulary. |

### 3.8 The bar widget

The widget's redesign is DESIGN.md §6 (v5.6); this table is only its strings. The header line and
the per-state row sets are specified there, with a text mock for each state.

| file:line | now | becomes | why |
|---|---|---|---|
| `Model.js:82-88` `remoteLabel` | `remote: {shortAddr} · trusted` | renamed `nodeLabel`: `public node {shortAddr}` in remote mode, `your own node` in local | Vocabulary; the trust suffix is deleted with every other mention of the flag. |
| `Model.js:104-118` `probeLabel` | `node {h} · synced · probed {age} ago` / `not probed yet` / `did not answer ({reason}) · {age} ago` | renamed `checkedLabel`: `block {h} · synced · checked {age} ago` / `not checked yet` / `no answer · checked {age} ago` | Vocabulary (probe), rule 5 (the raw reason), rule 9 (`block`). The age stays welded to the height, which was the one thing this function got right. |
| `Model.js:127-133` `walletStateLabel` | `up` / `idle — starts when you open a wallet` / `not running` | `running` / `starts when you open a wallet` / `not running` | Vocabulary (`up`, `idle`), rule 12. |
| `Model.js:134-146` `syncLabel` | `fmonero is not answering` / `No node configured` / `Node is not running` / `Synced` / `Syncing {p}%` / (remote) `remote: {addr}` | becomes `headerLine(status)`, at most five words, per §6.4: `fmonero not installed` / `No node set up yet` / `Your node is not running` / `Public node · synced · wallet closed` / `Wallet syncing 63%` / `Wallet open · 1.2345 XMR` (v5.6.1: the **short** balance field, V16 — `1.23` is a string this formatter cannot emit) | The owner's complaint about the widget, in one function. The old label described the node and never the thing the user came to see. |
| `Model.js:148-151` `heightLabel` | `Node · {grouped}` | `Block {grouped}` | Rule 9; the word `Node` was carried by the row's own label. |
| `Model.js:154-159` `peersLabel` | `not available on a restricted node` | `not shown by this node` | Vocabulary. |
| `Model.js:169-178` `pendingLabel` | `Send in flight: {x} XMR → {dest} · {n} of {m} relayed` | `Sending {x} XMR to {dest} · {n} of {m} sent` | Vocabulary (in flight, relayed). The `showBalance` gate is unchanged. |
| **new** `Model.js` | — | `walletSyncLabel(status)`: `Syncing {p}% · {n} blocks left` | Decision 7: the widget's headline while a wallet is behind. `p` = wallet height / node height; `n` = node height − wallet height, where the node height is the local `get_info` in local mode and the cached check in public-node mode. |
| `Panel.qml:52` | `Monero · fmonero is not answering` | unchanged | Already plain. |
| `Panel.qml:354-356` hero | `title Model.heightLabel` / `meta Model.syncLabel` / `detail {nettype}` | `title` = `headerLine`, `meta` = the one supporting row for the state, `detail` deleted | The nettype was a third line of chrome on a two-line hero. |
| `Panel.qml:391` | `Stop the node (n)` / `Start the node (n)` | `Stop your node (n)` / `Start your node (n)` | Says which node. |
| `Panel.qml:411-412` | rows `Node` / `Probed` | `Node` / `Checked` | Vocabulary. |
| `Panel.qml:415` | `fmonero does not talk to this node; monero-wallet-rpc does` | **delete** | Rules 2 and 6. It was the widget's half of the Node screen's lecture. |
| `Panel.qml:431-434` | gauge label `{height} / {target}` | unchanged, shown only while syncing your own node | Right, and now it only appears where it has values. |
| `Panel.qml:437` | row `Peers` | unchanged | Already plain. |
| `Panel.qml:439-442` | row `Mempool`, value `{n} tx (lower bound)` | row `Waiting`, value `{n} transactions (at least {n})` | Vocabulary; `lower bound` is a statistics term. |
| `Panel.qml:444` | row `Last block` | unchanged | Already plain. |
| `Panel.qml:446-449` | row `Net` | `Traffic` | Rule 5: `Net` is the JSON key. |
| `Panel.qml:466` | `fmonero is not installed or not on PATH (~/.local/bin).` | `fmonero is not installed.` | Rule 6 (`PATH`); the path was a guess at the fix. |
| `Panel.qml:506` | section header `WALLET` | unchanged | Already plain. |
| `Panel.qml:514-521` | rows `Balance` / `Unlocked` | unchanged | Already plain, and they keep the **exact** `balance_xmr`: the short form is the header's alone (v5.6.1, V16). |
| `Panel.qml:525` | `↑ locked for {n} blocks` | `{x} XMR locked for about {m} min` | Rule 9, matching the TUI's line word for word. |
| `Panel.qml:539` | `no address yet` | unchanged | Already plain. |
| `Panel.qml:540` | `copy (c)` | unchanged | Already plain. |
| `Panel.qml:545` | `Refresh the wallet` | `Check for payments` | Vocabulary, matching the TUI's chip. |
| `Panel.qml:548` | `{n} blocks behind` | `{n} blocks to go` | Matches the TUI. |
| `Panel.qml:553` | `No wallet is loaded — load one in the TUI` | `Wallet closed` | Rule 12, and the button below it is the route, so the sentence does not have to be one. Decision 2: a closed wallet is a state, never a warning. |
| `Panel.qml:554` | `open TUI (t)` | `Open wallet…` | §2o.1 in the widget: the meta named the app, not the act. |
| `Panel.qml:566-568` | `wallet daemon: {state}` / `No wallet state — fmonero is not answering.` | `wallet service: {state}` / `fmonero is not answering.` | Vocabulary, rule 12. |
| `Panel.qml:606` | `Enter opens the TUI` | unchanged | Already plain. |
| `Panel.qml:624-628` | chips `Open TUI` · `Send` · `Open wallet…` · `Choose node…` · `Doctor` | per state, two or three: `Open wallet…` (closed) or `Check for payments` (open) · `Choose node…` · `Start your node` / `Stop your node` (your own node only) | Decision 2. Five buttons on a bar panel is a menu; `Open TUI` stays on `t` and on the hero, and `Doctor` appears only inside a problem row, where it is the action for that problem. |
| `Panel.qml:753` | warning row renders `{hint}` | renders `{action}` | §2o.1: the widget printed the CLI hint, which is where *"fmonero node local --yes"* reached the bar. |

### 3.9 CLI human output (`output.rs`)

This surface keeps its commands (§2o's last paragraph). What changes is its prose and its column
names, so that a person reading `fmonero status` and a person reading the TUI see one vocabulary.

| file:line | now | becomes | why |
|---|---|---|---|
| `output.rs:136` | `  hint: {e.hint()}` | `  Try: {e.hint()}` | Rule 11: `hint:` labelled a field; `Try:` is an instruction. |
| `output.rs:344` | `  hint: {w.hint}` | `  Try: {w.hint}` | Same. |
| `output.rs:280` | node badge `down` | `not answering` | Vocabulary. |
| `output.rs:283` | `node    {badge}  {url}  height {h}  layout {l}` | `node    {badge}  {addr}  block {h}  {your own node\|public node\|none}` | Rule 5 (`layout` and its raw values), rule 9. |
| `output.rs:298` | `        sync {p}%  peers {o} out / {i} in  txpool {t}  db {d}  up {u}` | `        synced {p}%  peers {o} out / {i} in  waiting {t}  disk {d}  running {u}` | Vocabulary four times in one line. |
| `output.rs:311` | wallet badge `down` | `not running` | Vocabulary. |
| `output.rs:323` | `pending {n} of {m} relayed  {x} XMR → {dest}` | `sending {n} of {m} sent  {x} XMR to {dest}` | Vocabulary. |
| `output.rs:361` | `cached: confirmations unknown (no daemon call)` | `from the saved copy: confirmation counts are not known` | Rule 7. |
| `output.rs:365` | `the txpool could not be read (the wallet daemon could not reach the node); every other row is complete` | `what is waiting to be mined could not be read; every other row is complete` | Rules 6 and 7. |
| `output.rs:375`, `:379` | `hint: fmonero wallet create <name> --yes` / `hint: monero-wallet-cli --restore-deterministic-wallet …` | `Try: …` on both | Rule 11. |
| `output.rs:412-415` | `Account {n}   Priority {p}   Weight {w}` | `Account {n}   Fee level {name}` | Vocabulary; weight stays in `--json`. |
| `output.rs:427-429` | `relayed` / `not relayed` | `sent` / `not sent` | Vocabulary. |
| `output.rs:437-441` | `broadcast` / `dry run: nothing was broadcast` / `nothing was broadcast` | `sent` / `dry run: nothing was sent` / `nothing was sent` | Vocabulary. |
| `output.rs:457` | `no transfers` | `no payments` | Vocabulary. |
| `output.rs:461-467` | header `date type amount fee conf locked (est.)` | `date what amount fee confirms locked` | Matches the TUI's table exactly. |
| `output.rs:486` | `{n} row(s)   high water {h}   cache age {a}s` | `{n} payments · saved copy {a}s old` | Rule 7 (`high water`); the height stays in `--json`. |
| `output.rs:508` | `locked    for {b} more block(s) (~{t}s)` | `locked    about {m} min ({b} blocks)` | Minutes are the answer, blocks the mechanism. |
| `output.rs:518` | `funded subaddresses:` | `addresses holding money:` | Vocabulary. |
| `output.rs:568` | `{n} live connection(s)` | `{n} peers connected` | Vocabulary. |
| `output.rs:571` | `  … seed {s} …` (the pruning seed) | dropped from the human row | It decides nothing a reader can act on; it stays in `node peers --json`. |
| `output.rs:583`, `:598` | `connections   — (not available on a restricted node)` | `connections   — (this node does not answer that)` | Vocabulary. |
| `output.rs:585` | `white list` / `gray list` | `known peers` / `other peers` | Rule 5: those are monerod's internal list names. |
| `output.rs:610` | `no public nodes cached yet` | `no list of public nodes yet` | Rule 6 (`cached`). |
| `output.rs:619-620` | group headings `monero.fail says` / `probed here` | `monero.fail says` / `checked here` | Vocabulary. |
| `output.rs:624-630` | columns `address health height checked ms height synced` | `address reliability block last seen ping block synced` | Matches the picker. |
| `output.rs:690` | `{n} of {m} nodes · list {age} old · {a} skipped (no port) · {b} skipped (bad address) · source {src}` | `{n} of {m} nodes · list {age} old · {c} skipped · from monero.fail` | Two parse-failure counts collapse into one; both stay in `--json`. |
| `output.rs:749-764` | `{mode}  {addr}  {trusted\|untrusted} · ssl {s}` | `now using a public node  {addr}` / `now using your own node  {addr}` | Vocabulary; two banned words gone. |
| `output.rs:769` | `monero-wallet-rpc was restarted: it holds --daemon-address in its argv` | `the wallet service was restarted so it picks up the new node` | Rules 6 and 7; the reason a reader needs is *why it mattered*, not which flag. |
| `output.rs:774-775` | `the wallet {name} was closed by that restart` / `hint: fmonero wallet open {name}` | unchanged / `Try: fmonero wallet open {name}` | Rule 11. |
| `output.rs:778` | `nothing changed: this machine was already in that mode` | `nothing changed: this machine was already set that way` | Rule 6 (`mode`). |
| `output.rs:783-785` | `this node learns your IP address and the timing of every request, supplies the fee — bound a send with --max-fee, and never mark a public node --trusted` | `this node sees your IP address and when you ask it anything` / `it also supplies the fee estimate, so bound a send with --max-fee` | One sentence carrying three warnings and an em-dash becomes two lines; the `--trusted` clause goes with the flag. |
| `output.rs:201`, `:292`, `:474` etc. | `—` for null | unchanged | The one convention worth keeping everywhere. |
| `output.rs:97`, `:108`, `:113`, `:119`, `:130`, `:151-208`, `:799-819` | plumbing, colour codes, `written`/`planned`/`skipped`, `dry run: nothing was changed` | unchanged | Reviewed; no prose. |

### 3.10 Core — warning texts (`status.rs`)

Every `id` stays exactly as it is: `status --json`'s `warnings[].id` is the agent contract (§5).
What changes is `text`, and every row gains an **`action`** — the in-app sentence the TUI and the
widget render where they used to render `hint`. `hint` itself is unchanged, and stays the CLI's.

| file:line | id | text now | text becomes | action (new) |
|---|---|---|---|---|
| `status.rs:757` | `monero_not_installed` | `{monerod\|monero-wallet-rpc} is not installed` | `Monero is not installed on this machine.` | `In a terminal: omarchy pkg add monero` |
| `status.rs:772` | `remote_node` | `monero-wallet-rpc talks to the remote node {addr}; it learns this machine's IP address and the timing of every request, and its own sync state cannot be seen from here` | `You are using the public node {addr}. It sees your IP address and when you ask it anything.` | `Press f on the Node screen to choose another.` |
| `status.rs:790` | `remote_node_trusted` | `{addr} is marked trusted, so this wallet sends its own unspent amounts inside requests to it` | `{addr} is set as trusted, so this wallet tells it which amounts it holds.` | `Choose the node again on the Node screen to clear that.` |
| `status.rs:806` | `node_down` | `the node is not answering` | `Your node is not answering.` | `Press u on the Node screen to start it.` |
| `status.rs:813` | `node_restricted` | `this monerod runs with restricted RPC: sync_info, connections, the peer list, bans, net stats and the rate limits have no source, and tx_pool_size is a LOWER BOUND because non-relayed and local transactions are left out` | `This node only answers basic questions. Peers, traffic and bans are not shown, and the count of waiting transactions is a minimum.` | `—` |
| `status.rs:824` | `node_syncing` | `the node is at {p}% and still synchronizing` | `Your node is {p}% synced.` | `It keeps going on its own.` |
| `status.rs:832` | `node_offline` | `the node was started with --offline` | `Your node is set to stay offline.` | `—` |
| `status.rs:843` | `node_no_peers` | `the node has no outgoing connections` | `Your node has no peers yet.` | `—` |
| `status.rs:854` | `node_low_disk` | `the node reports {n} bytes free, under the 120 GB floor for a pruned chain — a figure that is an ESTIMATE, not measured on this machine` | `Only {x} free on the disk your node uses. A pruned copy of the chain needs about 120 GB, which is an estimate.` | `—` |
| `status.rs:864` | `node_update_available` | `the node reports that an update is available` | `Your node says a newer version is out.` | `In a terminal: omarchy pkg upgrade` |
| `status.rs:872` | `foreign_node_running` | `this monerod is Arch's system unit: fmonero uses it read-only and will not start, stop or reconfigure it` | `Another program on this machine runs this node. fmonero reads it and never changes it.` | `—` |
| `status.rs:891` | `wallet_idle` | `the wallet daemon is not running; it starts when you open or create a wallet` | `No wallet is open. The wallet service starts when you open or create one.` | `Press o to open a wallet, or c to create one.` |
| `status.rs:899` | `wallet_down` | `monero-wallet-rpc is not running` | `The wallet service is not running.` | `Press o on the Wallet screen to open a wallet.` |
| `status.rs:909` | `wallet_closed` | `monero-wallet-rpc is running with no wallet open` | `No wallet is open.` | `Press o to open one.` |
| `status.rs:921` | `wallet_behind` | `the wallet is {n} blocks behind the node` | `This wallet has {n} blocks to go.` | `It keeps going in the background, even after you close this window.` |
| `status.rs:932` | `wallet_locked_funds` | `some funds unlock in {n} blocks` | `{x} XMR unlocks in about {m} min.` | `—` |
| `status.rs:955` | `send_may_be_in_flight` | `a send was built and may have been broadcast: {txid}` | `A payment was prepared and may already have gone out: {txid}.` | `Press 2 to look at your payments.` |
| `status.rs:971` | `node_stalled` | `a node call was killed at its deadline; {n}s of backoff left` | `Your node took too long to answer. fmonero will try again in {n}s.` | `—` |
| `status.rs:971` | `wallet_stalled` | `a wallet call was killed at its deadline; {n}s of backoff left` | `The wallet service took too long to answer. fmonero will try again in {n}s.` | `—` |
| `status.rs:983` | `auth_missing` | `the node accepted fmonero with no credential at all` | `Your node accepted fmonero without asking for a password.` | `—` |
| `status.rs:993` | `nettype_mismatch` | `the open wallet is {a} while node.network is {b}: the send path follows the WALLET, never the setting` | `This wallet is a {a} wallet and your settings say {b}. Sending follows the wallet.` | `—` |
| `status.rs:1015` | `history_cache_stale` | `the history snapshot stops {n} blocks behind the wallet` / `the history snapshot is empty while the wallet is at block {h}; it holds no transfers to serve from` | `The saved copy of your payments is {n} blocks behind.` / `The saved copy of your payments is empty.` | `Press R to rebuild it.` |

An `action` of `—` means the TUI and the widget show the text alone: there is nothing the user can do
from inside the app, and inventing a key to point at would be worse than a sentence that stops.

### 3.11 Core — errors and hints

| file:line | now | becomes | why |
|---|---|---|---|
| `address.rs:176` + callers | `{text} is not a Monero address: it is {n} characters; a Monero address is 95 (or 106 with a payment id)` | `"{text}" is not a Monero address. Addresses are 95 characters and start with 4 or 8; this one is {n}.` | The orchestrator's own example. Rule 10: name what a good one looks like, and drop the payment-id aside, which teaches a second concept mid-refusal. |
| `amount.rs:38` | `'all' is not an amount; \`wallet sweep\` moves the whole account and takes no amount` | `"all" is not an amount. To send everything: fmonero wallet sweep <address> --dry-run` | Rule 1; the CLI keeps the command, which is the fix. |
| `amount.rs:45` | `'{s}' is not an amount: only digits and one '.' are allowed (no separators, no sign, no exponent)` | `"{s}" is not an amount. Use digits and at most one dot, like 0.5 for XMR or 250000000000p for piconero.` | Rule 10: three negations became one example of each accepted form. |
| `error.rs:494` | `monero-wallet-rpc failed without a message (usually: it cannot reach the daemon)` | `The wallet service failed without saying why. It probably cannot reach the node.` | Rules 6 and 7. |
| `error.rs:511` | `this monerod is running with restricted RPC, so the method is not available ({message})` | `This node only answers basic questions and refused that ({message}).` | Vocabulary. |
| `units.rs:161` | `fmonero node {verb} needs the local monerod; node.mode is remote ({addr})` | `That needs your own node. You are using the public node {addr}.` | Vocabulary; the settings key is fine in `--json`, and the CLI's hint already names the command. |
| `units.rs:268` | `systemctl --user {verb} {unit} did not finish in time` | `The {node\|wallet} service did not answer in time.` | Rule 6: it printed the command it ran. |
| `units.rs:276` | `systemctl --user {verb} {unit} failed: {stderr}` | `Could not {start\|stop} the {node\|wallet} service: {stderr}` | Same. |
| `units.rs:307` | `{unit} is not installed in {dir}` | `fmonero has not set up the {node\|wallet} service yet.` | Rule 7 (unit); the fix is `setup --units`, which is the hint. |
| `units.rs:314` | `{binary} is not installed, so {unit} would exit 203/EXEC and be restarted every 5s without ever answering` | `Monero is not installed, so the {node\|wallet} service cannot start.` | Rule 6: `203/EXEC` is an exit status of a systemd job. |
| `units.rs:374` | `{unit} started but did not answer within {n}s{stopped}` | `The {node\|wallet} service started but never answered ({n}s). It was stopped again.` | Rule 7; the second sentence was a suffix. |
| `units.rs:728` | `units::install refuses to write a real systemd unit while FMONERO_STATE_DIR is set` | `Refusing to change this machine's setup while FMONERO_STATE_DIR is set.` | Rule 6 (`units::install`, `systemd unit`); the env var is what the reader must act on. |
| `session.rs:436` | `no fmonero node unit is installed` | `fmonero has not set up the node service yet.` | Rule 7. |
| `session.rs:427` | the `system_unit` refusal | `Another program on this machine runs this node. fmonero can read it but not change it.` | One sentence, shared with the TUI's toast so the two cannot drift. |
| `send.rs:252` | `0 is not an amount to send` | `0 XMR is not an amount.` | Rule 9. |
| `send.rs:395` | `relay_tx returned {hash} for the transaction fmonero built as {txid}` | `The node reported a different payment id ({hash}) than the one fmonero prepared ({txid}).` | Rule 6 (`relay_tx`), vocabulary. The `reason: txid_mismatch` is unchanged. |
| `send.rs:472` | `the wallet has no record of {missing}, which fmonero relayed, so a send may still be in flight` | `This wallet has no record of {missing}, which fmonero sent. That payment may still be on its way.` | Vocabulary. |
| `send.rs:531` | `no address-book entry is described '{label}'` | `No address book entry is named "{label}".` | Vocabulary (`described` came from the JSON field). |
| `send.rs:540` | `'{label}' matches {n} address-book entries: {list}` | `"{label}" matches {n} address book entries: {list}` | Vocabulary. |
| `send.rs:558` | `monero-wallet-rpc says {dest} is not a valid address` | `{dest} is not a valid address.` | Rule 6: who says so does not change the fix. |
| `send.rs:571` | `{dest} is a {net} address; this wallet is {net2}` | `{dest} is a {net} address. This wallet is {net2}.` | Rule 1. |
| `send.rs:613` | `the node is not reachable, so a transaction cannot be built safely` | `Your node is not answering, so fmonero will not prepare a payment.` | Vocabulary. |
| `send.rs:642` | `the wallet did not finish a refresh against {addr} within {n} s, so a transaction built now may be rejected` | `The wallet did not finish checking against {addr} within {n}s. A payment prepared now could be refused.` | Vocabulary, rule 1. |
| `send.rs:664` | `the node is at {p}% and not synchronized, so a transaction built now may be rejected` | `Your node is {p}% synced. A payment prepared now could be refused.` | Vocabulary, rule 1. |
| `send.rs:700` | `…; the daemon never answered relay_tx, so whether it received the transaction is unknown` | `…; the node never answered, so it is not known whether it received the payment.` | Rules 6 and 7. |
| `send.rs:727` | `the built fee is {x} XMR, above the --max-fee of {y} XMR` | `The fee came to {x} XMR, above the --max-fee of {y} XMR.` | Vocabulary (`built`); the flag name stays, as a flag. |
| `send.rs:876` | `another fmonero send is building or relaying` | `Another fmonero send is already in progress.` | Vocabulary. |
| `settings.rs:51` | `'{l}' is not a Monero seed language; known: {list}` | `"{l}" is not a seed language. Choose one of: {list}` | Rule 10. |
| `settings.rs:539` | `'{v}' is not true/false for {key}` | `{key} takes true or false, not "{v}".` | Rule 10: the key first, then what it accepts. |
| `settings.rs:561` | `'{v}' is not a number in {lo}..={hi} for {key}` | `{key} takes a number from {lo} to {hi}, not "{v}".` | Rule 6 (`..=` is Rust range syntax). |
| `settings.rs:577` | `'{v}' is not a systemd unit name for {key}` | `{key} takes a service file name ending in .service, not "{v}".` | Rule 7. |
| `settings.rs:871` | `node.mode = remote needs node.remote to be set first` | `Set node.remote to an address before you switch node.mode to remote.` | Rule 2. |
| `settings.rs:888` | `node.remote is in use by node.mode = remote` | `node.remote cannot be cleared while node.mode is remote.` | Rule 2. |
| `settings.rs:912` | `{key} must be 0 (off) or at least 10 seconds` | unchanged | Already plain. |
| `settings.rs` `REMOTE_HINT` | `fmonero node remote <host>:18089 --yes   # host:port, no path, no credential` | unchanged | A CLI hint: a runnable command with a comment, which is what §9 asks for. |
| `nodes.rs:451`, `:459` | `monero.fail's nodes.json has no \`monero\` object` / `… no \`monero.clear\` array` | `monero.fail's list is not in the shape fmonero expects.` | Rule 6: a JSON path is not something the reader can fix. |
| `nodes.rs:621` | `curl is not installed, and public node discovery needs it` | `curl is not installed. fmonero needs it to get the node list.` | Rule 6 (`public node discovery`). |
| `nodes.rs:689` | `cannot fetch {url}: {detail}` | `Could not reach {url}: {detail}` | Plain verb. |
| `prompt.rs:45` | `no wallet password: stdin is not a terminal, and neither --password-file nor $FMONERO_WALLET_PASSWORD was given` | `No password given, and this is not a terminal. Use --password-file or FMONERO_WALLET_PASSWORD.` | Rule 6 (`stdin`), rule 1. |
| `prompt.rs:54`, `:58` | `Wallet password: ` / `Wallet password (again): ` | unchanged | Already right. |
| `prompt.rs:61` | `the two passwords do not match; nothing was created` | unchanged | Already right. |
| `prompt.rs:81` | `{p} is mode {m:04o}: a password file may have no group and no other permission bit` | `{p} can be read by others ({m:04o}). A password file must be 0600.` | Rule 2; the fix is already the hint (`chmod 600`). |
| `error.rs:11-80` `EXIT_CODES` descriptions | `bad arguments, unknown settings key, non-loopback URL, bad amount` etc. | unchanged | Printed by `capabilities` and `robot-docs`, which are machine surfaces by contract (§4). Exempt, and stated here so the next agent does not "fix" them. |

### 3.12 Doctor — titles, details, fix hints (`doctor/checks.rs`, `doctor/report.rs`)

Every `id` is unchanged (`doctor --json` and `doctor capabilities` are the agent contract, §7). The
titles are what a person reads down the left of the table, and twelve of the forty carried a section
number, an RPC method or a unit name.

| file:line | title now | title becomes |
|---|---|---|
| `checks.rs:224` | `monerod is installed and answers --version` | `Monero's node program is installed` |
| `checks.rs:232` | `monero-wallet-rpc is installed and answers --version` | `Monero's wallet program is installed` |
| `checks.rs:240` | `monerod is a 0.18.x build` | `The installed Monero is version 0.18` |
| `checks.rs:249` | `which monerod fmonero is talking to (§2i)` | `Which node fmonero uses` |
| `checks.rs:257` | `the node answers get_info` | `Your node answers` |
| `checks.rs:265` | `a remote node is not marked trusted unless it is private (§2m.6)` | `Public nodes are not set as trusted` |
| `checks.rs:273` | `the node is not in restricted-RPC mode` | `Your node answers more than basic questions` |
| `checks.rs:281` | `the node is synchronized` | `Your node is synced` |
| `checks.rs:289` | `the node has outgoing peers` | `Your node has peers` |
| `checks.rs:297` | `this machine's clock agrees with the daemon's` | `This machine's clock agrees with the node's` |
| `checks.rs:305` | `at most one monerod holds the data directory` | `Only one program is using the chain folder` |
| `checks.rs:314` | `monero-wallet-rpc answers get_version` | `The wallet service answers` |
| `checks.rs:322` | `the wallet daemon's login file is owner-only and works` | `The wallet service's password file is private and works` |
| `checks.rs:330` | `the open wallet is close to the node's height` | `The open wallet is close to the node's block` |
| `checks.rs:338` | `the wallet has the node's rpc-login credential` | `The wallet can sign in to your node` |
| `checks.rs:346` | `the open wallet's network is the configured one` | `The open wallet is on the network your settings name` |
| `checks.rs:354` | `wallet.dir and every ancestor exist at 0700` | `Your wallet folder exists and is private` |
| `checks.rs:362` | `no send is recorded as possibly in flight` | `No payment is left unaccounted for` |
| `checks.rs:371` | `both unit files exist and match units::render_*` | `fmonero's two background services are set up` |
| `checks.rs:379` | `both units bind 127.0.0.1 only` | `Both services listen only on this machine` |
| `checks.rs:387` | `no credential flag in either ExecStart, and --log-level 0` | `No password appears in either service's command line` |
| `checks.rs:395` | `no unit Wants= a monerod around the single-writer check` | `Neither service starts a second node behind fmonero's back` |
| `checks.rs:403` | `both units are disabled (they are started on demand)` | `Neither service starts on its own; fmonero starts them when needed` |
| `checks.rs:411` | `the unit's --wallet-dir is settings.wallet.dir, byte for byte` | `The wallet service uses your wallet folder` |
| `checks.rs:419` | `the unit's --daemon-address, trust flag and --daemon-ssl are the ones in force` | `The wallet service points at the node you chose` |
| `checks.rs:427` | `wallet.port, wallet.url and the unit's --rpc-bind-port agree` | `The wallet service's port matches your settings` |
| `checks.rs:436` | `the data directory's filesystem has room for a pruned chain` | `The chain folder's disk has room` |
| `checks.rs:444` | `monerod.conf prunes when settings.node.prune says so` | `Your node keeps a pruned chain when you asked it to` |
| `checks.rs:461` | `state.json parses` | `fmonero's own saved state is readable` |
| `checks.rs:469` | `config.toml parses and every key is in the §4 table` | `Your settings file is readable and holds only known keys` |
| `checks.rs:477` | `the history snapshot's header matches this wallet and this build` | `The saved copy of your payments belongs to this wallet` |
| `checks.rs:485` | `the public-node list is present and not a day old` | `The list of public nodes is less than a day old` |
| `checks.rs:493` | `the bar widget is linked into ~/.config/omarchy/plugins` | `The bar widget is installed` |
| `checks.rs:501` | `the bar widget is enabled in shell.json` | `The bar widget is switched on` |
| `checks.rs:526` | `the node required and accepted a credential` | `Your node asked fmonero for a password` |
| `checks.rs:534` | `everything fmonero writes is 0600, every directory 0700` | `Everything fmonero writes is private to you` |
| `checks.rs:542` | `who listens on the node and wallet ports` | `Who is listening on the node and wallet ports` |
| `checks.rs:509`, `:452`, `:518` | `fmonero's Omarchy menu entries are present` · `binary_on_path` · `monerod_conf_mode` titles | unchanged, they are already plain |

Details and skip reasons. The first row is the one that appears twelve times in a public-node run,
so it is the single highest-volume string in the program:

| file:line | now | becomes |
|---|---|---|
| `checks.rs:586-593` | `node.mode is remote ({addr}): there is no local monerod to ask, and no wallet-RPC method reports a remote daemon's own state (§1b.9 item 5)` | `You are using the public node {addr}, so there is nothing of your own to check here.` |
| `checks.rs:555-556` | `monerod is not installed, so there is nothing to ask` | `Monero is not installed, so there is nothing to check.` |
| `checks.rs:565-566` | `monero-wallet-rpc is not installed, so there is nothing to ask` | `Monero is not installed, so there is nothing to check.` |
| `checks.rs:600-610` | `{unit} is not written yet` | `fmonero has not set up the {node\|wallet} service yet.` |
| `checks.rs:623` | `not_needed_in_remote_mode` detail | `You are using a public node, so this machine needs no node program.` |
| `checks.rs:629` | `monerod is not on PATH` | `Monero's node program is not installed.` |
| `checks.rs:647` | `monero-wallet-rpc is not on PATH` | `Monero's wallet program is not installed.` |
| `checks.rs:672` | `monerod --version printed nothing` | `Monero's node program printed no version.` |
| `checks.rs:678` | `{line} is {v}.x; fmonero was built against {SUPPORTED}.x (§1b)` | `This is Monero {v}. fmonero is built for {SUPPORTED}.` |
| `checks.rs:682` | `cannot read a version out of '{line}'` | `Could not read a version from "{line}".` |
| `checks.rs:710` | `cannot detect a layout: {e}` | `Could not work out which node fmonero uses: {e}` |
| `checks.rs:714-718` | `user_unit — fmonero's own {unit} at {url}` | `fmonero runs your node at {url}.` |
| `checks.rs:720-724` | the `system_unit` pass detail | `Another program on this machine runs this node. fmonero reads it and never changes it.` |
| `checks.rs:731-741` | `remote — monero-wallet-rpc talks to {addr} ({trusted}, ssl {s}); fmonero itself never does (§2c)` | `You are using the public node {addr}.` |
| `checks.rs:743-746` | `none — no fmonero unit and no system monerod.service: run \`fmonero setup --units\`` | `No node is set up on this machine.` (the fix command is already the `cmd`) |
| `checks.rs:766-773` | `{url} did not answer: {error}` | unchanged |
| `checks.rs:785`, `:813` | `the node did not answer get_info` | `Your node did not answer.` |
| `checks.rs:789-799` | `restricted: …` / `unrestricted: every node pane has a data source` | `This node only answers basic questions.` / `Your node answers everything fmonero asks.` |
| `checks.rs:800` | `get_info carried no \`restricted\` field` | `Your node did not say.` |
| `checks.rs:818` | `synchronized at height {h}` | `Synced at block {h}.` |
| live capture, `wallet_rpc_auth` skip | `{path} is absent, which is what it means for monero-wallet-rpc not to be running (§2c)` | `The wallet service is not running, so its password file is absent.` |
| live capture, `wallet_nettype_matches` skip | `no wallet is open, so there is no nettype to read` | `No wallet is open, so there is no network to read.` |
| live capture, `units_present` warn | `fmonero-monerod.service is absent: run \`fmonero setup --units\`; fmonero-wallet-rpc.service is absent: run \`fmonero setup --units\`` | `Neither background service is set up yet.` |
| live capture, `history_cache_fresh` pass | `no history snapshot yet; \`wallet history\` builds one` | `No saved copy of your payments yet. One is written the first time you look.` |
| live capture, `public_node_cache` pass | `156 nodes, 0 min old` | `156 nodes, checked 0 min ago.` |
| live capture, `state_perms` fail | `{run} is 0755, want 0700` | `{run} can be read by others (0755). It should be 0700.` |
| live capture, `plugin_enabled` pass | `oliver.monero is listed by \`omarchy plugin list\`` | `The bar widget is switched on.` |
| live capture, `menu_entries` pass | `every fmonero menu id is present` | `Every fmonero menu entry is present.` |
| live capture, `settings_valid` pass | `{path} parses and every key is in the §4 table` | `{path} is readable and holds only known keys.` |
| live capture, `state_valid` pass | `{path} parses` | `{path} is readable.` |
| `report.rs` footer | `  next: fmonero doctor --fix --dry-run` | `  Try: fmonero doctor --fix --dry-run` |
| `checks.rs:49-51` class descriptions | `fmonero-owned state; reverts by file backup` etc. | unchanged — `doctor capabilities` is a machine surface |

### 3.13 CLI `--help` and prompts (`main.rs`)

`--help` is read by people and by agents, and §4 makes it plain text by contract. §2o applies to its
prose; the flag names, exit-code list and amount grammar stay exactly as they are.

| file:line | now | becomes | why |
|---|---|---|---|
| `main.rs:33` `ABOUT` | `A fast Monero node + wallet front end for Omarchy.` | unchanged | Already plain. |
| `main.rs:41-42` `LONG_ABOUT` | `\`status\` never starts a daemon and never calls a wallet mutator, so it answers even while monerod is syncing or monero-wallet-rpc is busy.` | `\`status\` starts nothing and changes nothing, so it answers even while the node is syncing or the wallet service is busy.` | Rules 6 and 7 (`mutator`); the guarantee survives intact. |
| `main.rs:44-45` `LONG_ABOUT` | `Money safety: \`wallet send\` and \`wallet sweep\` build the transaction with do_not_relay first, print the REAL fee, and broadcast only with --yes.` | `Money safety: \`wallet send\` and \`wallet sweep\` prepare the payment without sending it, print the real fee, and send only with --yes.` | Rules 6 and 7; shouting REAL was doing the work the sentence now does. |
| `main.rs:101-102` `status` after_help | `Three RPCs with the wallet closed, five with it open, plus two systemctl probes` | `Three questions to the wallet and the node with no wallet open, five with one, plus two checks on the background services` | Rules 6 and 7. |
| `main.rs:143` `node up` after_help | `Refuses (exit 3) when another monerod already holds the data dir` | `Refuses (exit 3) when another program is already using the chain folder` | Rules 6 and 7. |
| `main.rs:202` `node remote` after_help | `Writes node.mode, node.remote and node.remote_trusted, re-renders both units` | `Writes node.mode, node.remote and node.remote_trusted, then sets up both background services again` | Rule 7 (`re-renders`, `units`); settings keys stay by rule 5's carve-out. |
| `main.rs:457` `wallet use-node` after_help | `This is the only way a remote node enters the picture: monero-wallet-rpc …` | `This is the only way a public node enters the picture: the wallet service …` | Vocabulary. |
| `main.rs:379` `wallet send` after_help | `Builds with do_not_relay and prints the REAL fee first.` | `Prepares the payment without sending it and prints the real fee first.` | Rules 6, 7. |
| `main.rs:406` `wallet sweep` after_help | `Sweeps the whole account (bounded by --below) and may build SEVERAL transactions` | `Sweeps the whole account (bounded by --below) and may need several transactions` | Vocabulary (`build`); the shout goes. |
| `main.rs:225` `node list` after_help | `Serves cache_dir/public-nodes.json and fetches only when --refresh is …` | `Uses the saved list and fetches a new one only when --refresh is …` | Rule 6 (a file path as the subject of a sentence). |
| `main.rs:295` `wallet create` after_help | `On a TTY the password is asked for twice and compared; an empty one is …` | `In a terminal the password is asked for twice and compared. An empty one is …` | Rule 6 (`TTY`), rule 1. |
| `main.rs:47-57` `AFTER_HELP` | exit-code list, amount grammar, the `--dry-run` list | unchanged | Every token is a flag, a code or a number. |
| `main.rs:108-456` the remaining twelve `after_help` blocks | reviewed | unchanged | They name flags, files and commands, and carry no banned prose. |

---

## 4. What does not change

- **`--json`, in full.** Every `id`, `reason`, `code_name`, warning id, doctor check id, key name
  and payload key is the agent contract (§4, §5, §5c, §7). Agents branch on those, and a rename
  there is a breaking change that buys a person nothing.
- **`capabilities` and `robot-docs`.** Machine surfaces by contract (§4). The exit-code
  descriptions in `error.rs` live there and stay.
- **Commands in the CLI's own hints.** `Try: fmonero wallet open <name>` is the point of §9's
  one-runnable-hint rule. Only the label changed, from `hint:` to `Try:`.
- **Every flag, exit code and amount grammar in `--help`.**
- **The `—` for an unknown value**, everywhere, in both surfaces.
- **Addresses are never elided** in a send confirm, in the picker's address column or in the
  address book (§7a.4, §2a).

## 5. How this stays true

`scripts/check.sh` gains one step (v5.6): a grep over the **user-facing string literals** of
`crates/fmonero/src/tui/*.rs`, `crates/fmonero/src/output.rs`, `crates/fmonero-core/src/status.rs`
and `plugin/oliver.monero/*.{qml,js}` for the §2o rule-7 word list — daemon, unit, RPC, loopback,
restricted, probe, layout, ssl, systemd, relay, broadcast, nettype, txpool, subaddress, priority,
in flight, high water — plus `§` and any `warning.id`-shaped token rendered outside `--json`.

**v5.6.1 widens both halves** (§8, and DECISIONS V17 demonstrated the need rather than asserting
it). The file list gains `crates/fmonero-core/src/rpc/jsonrpc.rs`,
`crates/fmonero-core/src/settings.rs` and `crates/fmonero/src/doctor/checks.rs` — fifteen files.
The word list gains `argv`, `breaker`, `snapshot` and `build`, each with word boundaries so that
`rebuild` and `fn build_transfer` still pass, and `monerod` / `monero-wallet-rpc`, which were
**allow-listed as commands** and thereby exempted the two worst prose strings in the program: the
allow-list now keeps only the shape a command has, `^"monerod --…` / `^"monero-wallet-rpc --…`,
which is what the doctor's two `--version` fix lines are and what prose never is. The self-test
plants one literal per hole and asserts all three fire.

Two things make that grep honest rather than decorative:

- **an allow-list for command lines**: a literal that is a runnable command (it starts with
  `fmonero `, `omarchy `, `monero-wallet-cli ` or `In a terminal: `) is exempt, because §2o.1's four
  exceptions and the CLI's own hints are commands on purpose;
- **a self-test**, in the same shape §6.7's greps already use: a planted literal
  `"the wallet daemon is not running"` in a fixture must make the step fail, and the allowed
  `"In a terminal: omarchy pkg add monero"` must not. A grep that cannot be shown to fire is a grep
  the first agent to hit it relaxes.

The string tests that pin these words (`crates/fmonero/src/tui/tests.rs`, `tests/e2e.sh`,
`tests/plugin-smoke.sh`, the doctor's `expect.json` fixtures) are **updated, never deleted**, and
`tests/tui_tmux.sh --capture` is re-run so the captures in `decisions/` show the new text.

## 6. The count

**v5.6.2 adds three surfaces to the inventory and closes its own gap** (the 2026-09-19 verification's
item 18 said this section lists fourteen surfaces and `node info`'s human output is not one of them).
The three are `node list`'s human table, `node info`'s human output and `node peers`'s — all three
rendered from `output.rs`, all three outside §3.9's row list, and each one a second vocabulary for
something the TUI already says (`monero.fail says` against `their block`; a generic key dump;
`last seen 1789560101`). They are in §9 below, with what they say now.



| | |
|---|---|
| Strings inventoried | **450** rows across 13 surfaces |
| Rewritten | **392** |
| Confirmed already plain, left alone | **58** |
| Deleted outright | **4** (two architecture sentences, the trust/ssl row, the `web` marker) |
| New strings the redesign needs | **7** (the public-node facts pane in three states, the picker's own-node chip, its KEYS row, the Wallet screen's background-sync line, the widget's wallet-sync headline) |
| Surfaces touched | TUI chrome · Node · Wallet · Send · Log · modals · toasts · action labels · widget · CLI human output · core warnings · core errors · doctor · `--help` |

Nine rows bundle a set of literals and are counted once above: the node badge (5 words, 5
rewritten), the wallet badge (7 words, 6 rewritten), the picker's column headers (7 cells), the
peers table's header (5), the history header in the TUI (6) and in `output.rs` (6), the Send
screen's fee chips (5), the Node screen's four fee levels (4), the log-level confirm's five
variants (5), and the batch of fourteen already-plain toasts (1 rewritten). Counted by literal
rather than by row, the rewrite is **432 strings**.

---

## 7. What the implementation settled (2026-09-18)

The rewrite table above is a specification; this section is what happened when it met the code, so
the next reader does not have to diff the two. **The reviewer returned no revisions**, so nothing in
§1–§6 was amended on review; every entry below comes from the implementation.

**Four strings the table missed, and the grep found.** §5's word grep landed before the last commit
and fired on four literals no row named: `new subaddress {i}` (→ `new address {i}`),
`daemon log level {n}` (→ `your node now logs at level {n}`), `a wallet name may not contain \`/\`
— the daemon refuses it` (→ `A wallet name cannot contain "/".`) and the modal title
` Label subaddress {i} ` (→ ` Name address {i} `). That is the whole argument for the grep: a table
of 450 rows written by hand misses four, and a grep with a self-test does not.

**Three deviations from the table, each in the direction of the rule.**

| Where | The table says | The code says | Why |
|---|---|---|---|
| the seed modal's close line (§3.5) | `Close this wallet first — press x.` | the modal says `With the wallet closed, in a terminal:` and no separate line | The table's own phrasing carries an em-dash, which DESIGN.md §2o rule 12 forbids; and the sentence core already prints says the same thing without needing a second one. §2o is the contract, so it wins over an illustration in this document. |
| the widget's locked-funds row (§3.8) | `{x} XMR locked for about {m} min` | `locked for about {m} min` | The amount is `balance - unlocked`, and that subtraction is arithmetic on money, which §2a.5 forbids in QML outright. DESIGN.md §6.5 records it and names the fix (a pre-rendered `locked_xmr` in §5). |
| `monerod_conf_mode`'s doctor title (§3.12) | unchanged, "already plain" | `~/.config/fmonero/monerod.conf is private (it carries a password)` | The old title named `rpc-login`, a flag, in prose. Rule 6 applies to the doctor's titles like everywhere else. |
| the addresses pane title (§3.3) | ` Your addresses — Enter copies · a new · l rename ` | ` Addresses · Enter copies · a new · l rename ` | At the Wide tier that pane is 48 columns and the specified title clipped to `… · a new · l renam` — a title that lies about its own keys. The em-dash goes with it, which rule 12 wanted anyway. |

**The exception this document used to keep is gone, and that is worth stating.** It read: several
modal titles keep a ` — ` (` Open a wallet — Enter opens · Esc cancels `, ` Payment — n edits the
note · Esc closes `, ` Keys — any key closes `), because there the dash separates a pane's **name**
from its **controls**. The code no longer does that in any of the three: it renders ` Open a wallet `,
` Payment · n edits the note · Esc closes ` and ` Keys · any key closes `, using the `·` rule 12
already allows for structure. Read off a real pty by the round-4 verification, at 120x32 and 150x40.
Rule 12's word grep is mechanised (`scripts/check.sh`) over string **literals**; it does not read the
`///` doc comments that become `--help`, and three of those still carry a prose em-dash — reported by
the round-4 verification, not fixed here.

**Two words the grep did not police, stated so nobody mistakes silence for coverage — and v5.6.1
polices both.** §2o rule 7 bans `build` (of a transaction) and `layout`; v5.6's step greps for
`layout` but **not** for `build`, because the word is unavoidable in the code around the strings and
a grep that fires on `fn build_transfer` is a grep the next agent relaxes. v5.6.1 adds it **with a
word boundary**, which is the distinction the first attempt lacked: `"the build"` fires and
`fn build_transfer`, `rebuild` and `building` do not (the grep reads string literals only, so the
function name was never at risk — the risk was `Press R to rebuild it.`, which is a real string in
`status.rs` and does not fire). Every `build` the table names was rewritten by hand and the tests
pin the replacements. The grep's other blind spot is its own: a `//` **inside** a
string literal on a continuation line reads as the start of a comment, so anything after it on that
line is not examined. Both are in the step's own comment, where the person who hits them will read
them.

**What the grep covers is wider than §5 asked for.** The word list gained `systemctl`, `mutator`,
`mempool` and `subaddr` — each of them a word §2o.2 retires and none of them in rule 7's own
list — and two exemptions rather than one: a runnable command (rule 11's whole point) and **a
single bare token**, because `"wallet_idle"` is a contract key and prose has spaces in it.

---

## 8. The plain-language fix-up (2026-09-19)

The 2026-09-19 verification of this round read both surfaces as a non-technical user on a real pty
and found **eighteen** things, three of them serious. Six of them are strings this document did not
inventory at all, and that is the useful part: §6 listed fourteen surfaces, and four of the six live
outside them.

**Three surfaces added to §5's grep, because §2o's first paragraph always governed them.**

| Surface | What it said | What it says now |
|---|---|---|
| `rpc/jsonrpc.rs` — the RPC error family (V8) | `monero-wallet-rpc is not answering: io: Connection refused (os error 111)`, and four more arms interpolating the binary name and the RPC method | `The wallet service is not answering.` / `The node is not answering.` and one plain sentence per arm, with the wire's own words moved to a new `detail` slot that only `--json` and `-v` render, and the in-app key in a new `action` slot the TUI shows |
| `settings.rs` — the 26 key descriptions `settings list` prints (V9) | `local \| remote — which daemon monero-wallet-rpc is pointed at (§2m)`, and six more rows carrying a `§` number | what the setting does for the user, then which values are valid: `Where your wallet gets its blocks: local, a node on this machine, or remote, a public node someone else runs.` |
| `doctor/checks.rs` — 54 details and fix lines (V10) | `{} and {} match units::render_* byte for byte`, `high water {}, {behind} behind the wallet's {wh}`, and fix lines naming `systemctl --user restart …` | one sentence each, block numbers grouped through the same formatter the TUI and the widget use, and **no fix line names systemctl**: each names `fmonero doctor --fix`, a command, or the keys `W` and `o` on the Wallet screen |

**The §2m.2 address refusals moved with them** (V11), because the TUI's `a` modal now shows them
under the field as the user types: `the port is required — monero-wallet-rpc fills a missing port
with 80 or 443, never 18081` became `The port is required, like node.example.com:18089.`, and the
one caller that told that case apart **by its prose** (`nodes::parse`, counting monero.fail's
portless rows) now branches on `detail: "no_port"`.

**Two rows of §3.1 and §3.3 were wrong about one fact each, and the fix is the same both times:
one predicate, two renderings.** `wallet service: running` sat four rows above a status bar reading
`wallet not running` (V13) and the Node hero's `Your node is not running` sat in the same frame as
`node not answering` (V14). Both pairs were computed correctly from two different fields. A service
that is up and silent now reads `not answering` on both surfaces, and a node that is off takes its
two words from one function.

**And three things this document cannot express, recorded because the next reader will look for
them here.** A cut line now ends at a word boundary with `…` (V15) — the status bar's toast and the
`?` overlay's rows were cut mid-word, and `A wallet is open. Press x to close i` is a complete
sentence about a key that does not exist. Decision 7's own two lines were drawn outside every frame
on the Wallet screen (V3) and are now inside its ` Balance ` pane. The `Use your own node` confirm
(V4) stated nothing at all, and `Dialog` is why: it draws its message as one centred row, so the
sentences after the first spilled past the border — it is an overlay now, the shape the picker's own
confirm has.

---

## 9. Plain-language round 3 (2026-09-19): the surfaces this document had not inventoried

The 2026-09-19 verification read the program as a non-technical user and found twenty-two things;
the round that closed them re-read every capture and found nineteen more. Most were rows in §3 that
the implementation had drifted from, and they are corrected in place above. **These are the ones this
document had never listed at all** — every one of them one command away from the owner, and every one
of them governed by §2o's first paragraph from the day it was written.

| Surface | What it said | What it says now |
|---|---|---|
| the doctor's **human table** (W1) | `warn:wallet_unreachable  wallet_rpc_reachable  The wallet service is not …` — the check's **id** where §7 says its title goes, and `:{reason}` after the status word. Fifteen of the forty ids carry a §2o rule-7 word | the title, a verdict in words (`OK` · `needs attention` · `not working` · `not checked: no wallet is open`), and the detail under it. Group headings are sentences (`The two background services`, not `[units]`) |
| `node list`'s **table** (W20) | group headings `monero.fail says` / `checked here` over two columns both headed `block`; `synced` rendering the JSON `true`; `ping 20` with no unit; ungrouped block numbers; `web` and `own` markers with nothing to explain them | the picker's own columns (`their block`, `block here`), `yes`/`no`, `20 ms`, grouped numbers, one marker and a footer that explains it |
| `node info`'s **human output** (W20) | the generic key dump: `layout remote`, `busy_syncing —`, `fee fees_pico=— per_byte_pico=—`, thirty rows of field names — and, in local mode, every field of every peer on one row, because `sync.peers` is an array | the rows `status` prints, then what only this command asks for: the version, the payments waiting, how many peers it is fetching from |
| `node peers`'s **table** (round 3's self-read) | `block 3541122`, `8412s`, `last seen 1789560101` — a raw unix second in the column whose whole content is *when*; `3 peer(s)` | a header, grouped blocks, `2h 20m`, `62h 53m ago`, `1 peer` / `3 peers` |
| `settings list`'s **default column** | `[default ]` for an empty default, and a `*` (changed) against `node.data_dir`, `node.log_file` and `wallet.dir` on a machine where nobody had changed anything — three of the defaults in `KEYS` are patterns | `(none)`, and `Settings::default_here` resolves the patterns through the same function `load` uses, so the marker and the printed default are both about this machine |
| `status`'s **node and wallet rows** (W3) | `node  not answering  …  block —  public node` and six `—` under it, about a node nobody had contacted; `wallet  not running  http://127.0.0.1:18083  —  balance — XMR` | the cached check (`public node · synced`, block, response time, `checked 2s ago`), and a wallet row that says only what is known |
| `wallet history`'s **table** | `2026-09-16T14:01`, `block` for a mining reward, `pool` for one waiting, `true` under `locked` | `2026-09-16 14:01`, `mined`, `waiting`, `locked`/blank — from `TransferKind::words()`, which the TUI's table reads too |

**And two rules this document said were "enforced by reading" now have greps** (§5's step): rule 12's
em-dash, with a self-test over three planted dashes and the three shapes of the unknown-value glyph
that must pass; and `txid`, which §2o.2 retires and the word list did not carry — the Wallet screen's
filter hint read `filter by txid, address or note` through the whole v5.6 round. A `{field}` name is
read as a substitution rather than as prose, so `"… · id {txid} · …"` is a sentence that says `id`.

---

## 10. Plain-language round 4 (2026-09-19): where the words and the facts had come apart

A fresh reader took 144 captures of the v5.6.3 build on a real pty at 120x32 and 150x40 and found
ten serious items and thirty minor ones. His own summary of the register is that the writing is good
now, and that what stops the build is *"the last few places where the words and the facts have come
apart"* — a sentence that is true of every row below. This section lists them by the surface they
were found on. Every one is corrected in §3 above where §3 had a row for it.

### 10.1 The three that blocked it

| What it said | What it says now |
|---|---|
| **S1.** With a `.keys` file present and the folder unreadable, the Wallet screen read `[o] Open a wallet      (none on this machine yet. Press c to create one)` and the open modal `There are no wallets yet, in this folder:` — a false statement about his money, followed by the one action that writes a new wallet file where the old one lives. The CLI on the identical state was honest. Cause: `wallet_dir::list(&ctx).unwrap_or_default()`, four times | `Tui::reload_wallets` keeps the reason. The chip reads `(fmonero cannot read your wallet folder. Press o to see why)` and the modal carries core's own sentence, the folder, and `Press o again once you can read that folder.` |
| **S2.** The Send confirm rendered `plan.total_amount` — the wallet service's reply — while the form two columns away showed what he typed, and nothing anywhere compared the two | `send::build` compares them and refuses with exit 3 `amount_mismatch`: *"You asked to send 0.2500 XMR and the wallet service prepared 0.1000 XMR. Nothing has been sent."* `fmonero-mock` echoes a request's destinations, so a capture cannot show a figure nobody typed |
| **S4.** With `node.mode = remote` and the public node silent, `status` listed two `[info]` lines and `doctor` said `0 need attention, 0 not working` — while the row one line above both said `no answer` | `public_node_down` at **warn**, with `On the Node screen, pick another node and press Enter.`; and `node_reachable` answers from the cached check instead of skipping, so `doctor --only node` exits 11 |

### 10.2 The rest of the serious list

| What it said | What it says now |
|---|---|
| **S3.** Pane title ` Waiting to be mined · fees in XMR per byte ` over `their fees  0.000427 XMR` — a pool total, four orders of magnitude from a per-byte rate, under a title claiming the unit for it | ` Waiting to be mined `, then `they carry  0.000427 XMR in fees altogether`, then `XMR a byte  cheapest 0.00000002 · normal 0.00000008` |
| **S5.** `Payments  6 · t: all` above `Nothing yet. Press R to check, / to filter, t to change what shows.` — the header saying six, the body saying none, and the first thing it names the one action that cannot help | three states: an empty wallet keeps that line; a text filter reads `No payment matches "zzzz". Press / to change it, or Esc to clear it.`; a kind filter reads `You have 6 payments, and none of them is sent. Press t to show them all.` |
| **S6.** `fmonero: cannot read` / `<path>:` / `Permission denied (os error 13)`, and `fmonero: Input/output error (os error 5)` on a closed pty — the operating system's words in the MESSAGE slot that `with_detail`'s own comment reserves for `detail` | `Error::io` / `io_words`: one sentence per `ErrorKind`, the raw text in `detail`, and a `scripts/check.sh` step that refuses a `{e}` substitution in any user-facing literal |
| **S7.** The log-level confirm said `Press n on the Node screen to read the log.` — `n` has no arm there at all | `Press 4 to read the log.` |
| **S8.** The bar could print `fmonero status failed (exit 4)` and the program's first stderr line verbatim | one sentence and one route: `fmonero could not say how things are. Press d for the doctor.`, and a failed button reads the Error's own `action` |
| **S9.** Eight seconds into a create: `[W] Stop the wallet service  (nothing is open, so nothing is interrupted)` over `running, but not answering yet` over `Creating mywallet 8s / 300s` | `(not while something is running)`, and the pane and the badge both take the running action's own label |
| **S10.** `A payment was prepared and may already have gone out: cccc1111…ccdde2.` — an elided hex id ending the product's most serious warning, on a 380-pixel bar | the amount: `A payment of 0.2500 XMR was prepared and may already have gone out.` Every character of the id is in `--json`. The same deletion in the pending banner, the sent toast, the Send screen's row and the payment detail modal |

### 10.3 The thirty minor ones

| # | What it said | What it says now |
|---|---|---|
| M1 | `node  not answering  127.0.0.1:18081  your own node` three lines above `[warn] Your node is not running.` | `node_off_words_from`, so the CLI's own row shares the predicate its warnings use |
| M2 | three duration shapes: `66h 23m` in the TUI, `2d 18h` on the bar, `running 3d 06h` in the CLI; and `1h 00m` beside `1d 0h` | one `fmonero_core::duration` with a days unit, two digits on every field after the first; `age` is it plus the preposition, `human_uptime` is it plus a clamp, `Model.age` is its JavaScript half |
| M3 | a column headed `ping`, and a hero collapsing six check failures into `no answer` while the column beside it said `refused` | `checked`, and `remote_check_words` returns the specific word from `probe_reason_words` |
| M4 | `fast 0.00000032 · fastest 0.000004` — the larger value in fewer digits; `0.0000` beside `0.00003072` in one fee column | `Piconero::to_xmr_min(FEE_DECIMALS)`: eight decimals down a fee column, still exact, never truncated |
| M5 | the note and rename modals put `Enter saves · Esc cancels` **above** the only field, and the field had no label; the note modal never said which payment | the field above the keys line, labelled `note ` / `name `, and the title names the payment: ` Note for the sent of 0.2500 XMR on 09-16 10:00 ` |
| M6 | the `NOTE` and `NAME` columns appeared only at 150x40, and `wallet history` had no note column at all | `history_cols` measures the note against the pane and gives it what is left before the fee; the addresses pane's name column starts at 48 columns; `wallet history` has a note column |
| M7 | `status --check-node` printed `not checked yet` and `Try: fmonero status --check-node` — the command just typed | the flagged run says a check is running in the background and names `fmonero status`; the unflagged one explains what the flag does |
| M8 | in public-node mode the Log screen promised `No log yet. Your node writes it once it runs.` | `This is your own node's log, and you are using a public node. Press 1, then l, to use your own node.` |
| M9 | in local mode the picker still drew `[l] Use your own node`, which answers `You are already using your own node.` | the chip is not drawn in the mode it cannot act in |
| M10 | with the node stopped, `d` opened ` Stop your node ` / `fmonero stops your node on this machine.`; with the wallet service down, `l` promised `the wallet service will restart`; and one sentence sat at the bullet indent without a bullet | both confirms describe the state they are in, and the sentence gets its own blank line |
| M11 | `Select a node first, with ↑↓ or a click, then Enter.` under the ` Use this node ` confirm for the node just selected | `toast_hidden`: a toast does not survive into a modal, the send confirm or the help overlay |
| M12 | ` Use this node ` said nothing about the row behind it reading `no answer` | `· when fmonero checked it, there was no answer: refused`, from the picker's own row |
| M13 | `f` described three ways in one screen, and `OWN` was the only capitalised word in the product | `f` gets a fresh list, everywhere; choosing a node is a selection and Enter; no shouted word |
| M14 | `can be read by others (0755). It should be 0700.` | what it means, and the fix line below it |
| M15 | `the public node 127.0.0.1:18099` and `127.0.0.1` one line apart | the address as the user set it, both times |
| M16 | `their fees`, `0 being rejected`, `limits`, `Peers  0 · b: bans`, `peers ⇄ bans`, `mainnet omarchy` | `they carry … in fees altogether`; the turned-away count only when it is not zero; `its limits`; a `[b]` chip under the pane; `Node: swap the peers table for the blocked addresses`; `network mainnet` and no theme name |
| M17 | `To      DwVjeRVd…` — a 95-character address showing its last 57 with nothing saying so | the row's second line reads `95 characters typed. The box is showing the end of them.` |
| M18 | `"" is not an amount. You typed nothing.` with the cursor two rows above | `mainnet address · now type how much, in XMR.` |
| M19 | `known after the check`, `Enter checks the fee`, `Checked. Nothing has been sent yet.` — a noun the money screen never defined | `known once Enter prepares it`, `Enter works out the fee. Nothing is sent yet.`, `The fee is worked out. Nothing has been sent yet.` |
| M20 | backticks printed literally in three `settings list` descriptions, and `wallet.priority` never used the words `fee level` | no backticks, and `The fee level to use unless you say otherwise` |
| M21 | `no wallets in <dir>` and two `Try:` lines; the toast `new address 3` | `There are no wallets in <dir>.` with one `Try:`, and `Address #3 is ready. Press Q to show it as a QR code.` |
| M22 | `/ Synced` changed nothing visible and said nothing; Esc kept the text, so the next `/` appended | `3 lines have "Synced" in them. n and N step between them.`, and Esc clears the field |
| M23 | `Tab is off here. Press Esc first to discard this.`; the label `Repeat the password` among three nouns | `Tab cannot change screens while this is open. Esc closes it and keeps nothing.`; `Password again` |
| M24 | the add-a-node field label `address` in lower case, and `Nodes you add here are treated as public.` | `Address`, and `fmonero will not tell a node you add here which amounts your wallet holds.` |
| M25 | `2 fmonero could not read` — a count in front of the program's name | `2 could not be read` |
| M26 | a button that failed showed the message and withheld every route, because only `hint` was considered | the Error's `action` is the in-app sentence, and it is read |
| M27 | ` · checked 1m 35s ago` — a row starting with a bare separator; and `Public node · wallet closed` with no verdict, for the same restricted node | `answered · checked 1m 35s ago`, and `Public node · answered · wallet closed` |
| M28 | `not shown by this node` about a node that is not running | `none while your node is off` |
| M29 | `failed  …  locked`, and blank for a spendable payment with nothing saying what blank meant | a `SPENDABLE` column: `yes` · `not yet` · `never` · `—`, from `history::lock_words`, in the TUI's table, `wallet history` and the detail modal |
| M30 | ` Receive · Esc closes `, a code, and 95 characters — no account number, no purpose | `Your address #0 · Primary account` above the code, and `Show this, or send the line above, to be paid.` under it |

### 10.4 Three things this round deleted rather than reworded

The payment detail modal's first row was the whole 64-character id; `unlocks at 0` was on every row;
and `double spend false` was on every correct payment. All three are gone — the id to `--json`, the
other two rendered only when they carry a fact. §2o has no rule that says "delete it", and it needs
none: a row that says nothing is a row that pushes the rows that do say something off the bottom.

---

## 11. Plain-language round 4, iteration 3 (2026-09-19): the CLI, and the four lines about money

A fresh reader took 129 human CLI captures and 101 TUI captures of v5.6.4 on a real pty at 120x32
and 150x40, and returned ten serious items and forty-two minor ones. His verdict was that the TUI is
*"genuinely finished work"* and that *"the distance left is almost entirely in the CLI"*. He was
right about where, and about two lines in the TUI that were worse than anything in the CLI.

### 11.1 The four about money

| Where | Was | Is |
|---|---|---|
| `wallet send` / `wallet sweep` refused for want of `--yes` (`main.rs::refuse_with_plan`) | `fmonero: error[safety]: This would send …` / `reason: needs_yes` / `hint: fmonero wallet send` | the plan, then one plain sentence and one `Try:`, through `Out::report` — the only error printer there is |
| the same sentence's total | `0.50003072 would leave this wallet.` | `0.50003072 XMR would leave this wallet.`, and where the sum overflows, `fmonero could not add the amount and the fee together, so do not send this.` |
| the TUI's pending banner, the bar's pending row, `status` | `0 of 1 sent` — `relayed_count`, an acknowledgement fmonero never got | `may already have gone out`, from `PendingStatus::progress_words`, which is the same wording the warning beside it already used |
| the TUI's status bar with a payment unresolved | `● node problem` on a node that was synced, answering and holding nineteen peers | the **wallet** cell reads `payment may be out`; the node cell reports the node |
| `wallet send --yes` against a mismatched relay answer | two 64-character ids differing in their first two characters, and *"check which one landed"* | *"The node says it took a different payment from the one fmonero prepared. The money may have moved, so do not send it again until you have looked."* Both ids are in `detail`. |
| `status` and the bar's hero with funds locked | the total balance alone | the spendable figure with it, from `wallet.locked_xmr` and `unlocked_xmr_short` |
| the send confirm's three figures | `0.0100` · `0.00003072` · `0.01003072` | all three at eight decimals, so the sum checks by eye |

### 11.2 The twenty-two commands that had no human renderer

`output::human` matched seventeen labels and fell through to a printer that used each JSON key as a
row label. That is where `host=203.0.113.66 ip=1114702027 seconds=84213`, `ssl_support disabled`,
`trusted false`, `nettype mainnet`, `unit_started false`, `synchronized —`, a 64-character payment id
under the plural key `txids`, and the seed warning a second time under `seed_backup_hint` all came
from — six of rule 7's banned words and rule 5's own prohibition, on twenty-two commands.

Each has a renderer now, and `capabilities` is the only label the generic printer still serves. The
shape of all of them: say what happened, then what to do next; one fact a line; a unit on every
quantity; no id outside `--json`. Three worth quoting:

* `node bans` → `2 addresses blocked` / `203.0.113.66   23h 23m left` — the words the TUI's own
  Blocked-addresses pane uses, and the 32-bit integer form of the address stays in `--json`.
* `wallet use-node` → `This wallet is now using the node at 127.0.0.1:18081.` /
  `fmonero will not tell that node which amounts your wallet holds.` /
  `Opening a wallet sets this again, so it lasts until then.` — §2o.2's own wording for `trusted`,
  and `ssl_support` is not shown at all.
* `version` → `fmonero 0.1.0` and `agent::BUILD_SUMMARY`, three short paragraphs ending *"treat a
  send as untested, and send nothing you could not lose."* `BUILD_NOTE`'s nine reason ids, five
  section numbers and `ProgramSimulator` stay in `--json`, where the agent it was written for reads
  them.

### 11.3 `--help`, read for the first time

Every line of `--help` is a `///` doc comment, and §2o.3's step greps string literals. So
`The local monerod: start, stop, inspect, peers, bans, limits`,
`stop_daemon over RPC first, so LMDB closes cleanly, then systemctl stop`,
`Point monero-wallet-rpc at a REMOTE node and keep it there (§2m)` and
`--units  Write both systemd user units and monerod.conf, then daemon-reload` had never been looked
at by the rule that governs them. 139 doc comments across four files are now read by a step of their
own, with a self-test. A sample of the rewrite:

| Was | Is |
|---|---|
| `The local monerod: start, stop, inspect, peers, bans, limits` | `Your node: start it, stop it, look at it, its peers, blocked addresses, limits` |
| `Stop the node: stop_daemon over RPC first, so LMDB closes cleanly, then systemctl stop` | `Stop your node, giving it time to close its files cleanly first` |
| `Point monero-wallet-rpc at a REMOTE node and keep it there (§2m)` | `Use a public node someone else runs, and keep using it` |
| `Send \`trusted\` — only ever for a node YOU run (§2m.6)` | `Let it see which amounts your wallet holds. Only ever for a node YOU run` |
| `Ask each listed node for get_info from THIS machine` | `Check each listed node from THIS machine (each one learns your IP address)` |
| `Transfer history, from a local snapshot plus one incremental fetch` | `Your payments, from a saved copy plus whatever is new since it was written` |
| `Clear the pending-send record after checking whether the transaction landed` | `Clear the record of a payment fmonero was in the middle of sending` |
| `Write both systemd user units and monerod.conf, then daemon-reload` | `Set up the two background services and the node's own settings file` |
| `16 tx rejected` | `16 the node would not take it` |

### 11.4 The forty-two minor ones, by the surface they were found on

**The Send screen.** The Preview read `Fee  known once Enter prepares it` while the confirm modal
beside it showed the real figure — it reads the plan off `Stage::Built` now and gains a Total row.
Five wordings for pressing Enter became one, `works out the fee`, which replaces §7a.6's own
`check it` (M19's undefined noun, sixth use).

**The Wallet screen.** The balance column is whole or absent, because at 120 columns it printed
`1.2345678…` three rows under the same number in full. The focused pane carries a `▸` and is the only
one whose title claims `Enter`. `Payments 2 of 2 · in only` on a wallet of six was the kind filter
narrowing the query as well as the view. `[t] What shows` is `[t] Choose which payments show`, the
words the `?` overlay uses. Three chips that told the reader not to press them are not drawn in the
states they cannot act in. `x` with nothing open answers about the wallet, not about a service.

**The payment detail modal.** Capitalised labels, like every other modal. `failed · never got into a
block`, because `not in a block yet` read as a payment still coming. No fee against money received.
The recipient's **whole** address, which this modal is the only place to show. ` Note for 0.1000 XMR
· sending · 09-16 14:01 `, because "the sending of" is not a phrase anyone says — this supersedes
§3.5's ` Note for this payment `, which M5 had already replaced for a reason that still holds.

**The confirms.** The log-level modal says what the level is now as well as what `y` would set,
stops naming a key that closes it — `4` meant a *level* in two of its own bullets — and is refused
outright while the node is off, where it was guaranteed to fail. The Stop confirm says what
confirming does instead of offering a confirmation for nothing.

**The `?` overlay.** Its bottom border was drawn on the status bar at 120x32, leaving
`●╰────…────╯p`: `overlay` reserves that row. Three keymap pairs merged to pay for the row the
overlay never had — how to move a selection in a list or a table — and the `sweep` exception is the
last row of the panel, reading `In a terminal: …`, which is the shape §2o.1 prescribes for all four.

**The wrapping.** `wrap_bullet` dropped two characters at every line break whenever a bullet carried
a token longer than the modal, silently and with no marker; the open-a-wallet modal lost three.
`wrap_hanging` gives each line its own budget before anything is wrapped, and a test walks every
character at five widths.

**The CLI.** One next step per error, and in the CLI it is the command: `Press o on the Wallet
screen` beside `Try: fmonero wallet open <name>` was the same instruction twice, naming a screen the
reader is not looking at (`action` stays in `--json`, for the TUI and the widget). No `Try:` under an
`[info]`. `node log` with no file says so once. `node info` stops claiming it is fetching once it has
caught up, and its fee continuation keeps its label. `answered` heads the column whose cells read
`100% of 18`, with a legend saying whose checks those were — this supersedes §3.9's and §4's
`reliability`. `require_yes` names the action in words, not the command twice.
`wallet clear-pending` says *payments*. `wallet address` gives each address a row of its own.

**The doctor.** A hoisted group note is a bullet that says which rows it explains. One wording for a
service that is not set up, where there were three. One hint in the footer instead of a `jq` pipeline
over the internal `status` value. `Only fmonero is listening on the node and wallet ports` is a
statement, not a question with no question mark, and its next step is `node info` rather than
`status --json`. `… can be read by others, and only you should be able to.` no longer joins two
contradictory clauses with "and", and it ends.

**The bar.** The locked row carries its amount, from `wallet.locked_xmr`. The hero's supporting fact
is what is spendable whenever some of the balance is not. `wallet_behind` is not repeated in the
problems section while the hero already says it. The peers row with a public node is `—`, the glyph
for a value with no source, rather than a sentence where a number goes.

**One word, everywhere.** `refused` was the one probe reason returned as itself while the other five
were turned into English, and to a Monero user it reads as the node turning him away. It is
`nothing listening`.

---

## 12. Plain-language round 5 (2026-09-19): the twelve serious and the twenty minor, closed

Round 4's independent verification read 112 captures off a real pty and returned twelve serious
findings and twenty minor ones. This section is what each one now says. The three defects the whole
effort is named after were all arithmetic, and all three are gone.

### 12.1 The three width defects

| Where | Was | Is |
|---|---|---|
| the picker's address column (`screens.rs`, S1) | `monero-rpc.cheems.de.box.skhron.com.ua:180` at 120 columns — two characters of the port gone, no ellipsis, no marker, and `:180` is a plausible port | columns drop from the right until the widest cell fits whole; the `width / 2` cap that permitted the cut is deleted, and so is the configured row's separate floor, which the measurement already covers |
| `node list`'s header (`output.rs`, S2) | 122 characters on a 120-column terminal, so `synced` arrived as `sync` and `ed` | `columns()` takes the row's own indent as a positional parameter, and `nodes_header` is a function a test can measure |
| the open-a-wallet modal (`screens.rs`, S3) | a 248-character path rendered as 245 — a `-`, a `0` and a `-` gone from the middle, naming a folder that does not exist | the budget is `MODAL_W - 6`: the two columns each line spends on its own `" {part} "` padding |

### 12.2 Money and ids

| Where | Was | Is |
|---|---|---|
| the pending-send refusal (S4) | `1 of 1 payments were prepared…` over a `Try:` line whose `#` comment carried all 64 characters of an id, on a command that takes no id | `The payment from 2026-09-16 14:01 was prepared and fmonero never saw it go out. It may still be on its way.` · one hint literal, `send::PENDING_HINT`, shared by five surfaces and carrying no id · `action` = `Press P on the Wallet screen to clear that record.` |
| the resolve step's refusal (S4) | `This wallet has no record of <64 hex>, which fmonero sent.` | `One payment fmonero sent is not in this wallet's own record of payments. It may still be on its way.` The id is in `detail`, which reaches `--json` and no human line |
| the doctor's `send_may_be_in_flight` row (S4's class) | every unrelayed id joined into the detail a person reads | the shared sentence, the destination, and `Only you can decide whether it arrived` |
| the sweep plan's rows (S5) | `5c2e0b83…29d74b  0.8000 XMR  fee 0.00003072 XMR  not sent` | `1.   0.80000000 XMR  fee 0.00003072 XMR  not sent` — a row number tells two equal amounts apart, and the id is in `--json` |
| the send refusal's own figures (M1) | the plan table at eight decimals and the sentence under it at four, changing under the reader mid-decision | `Piconero::to_xmr_money` — one precision for everything on the path where money is about to move |
| the sweep refusal (M2) | `This would send everything you have, 1.234506451234 XMR` three lines under `Total 1.234567891234 XMR`, which is the balance | `This would empty this account. 1.234506451234 XMR to <addr>, 0.000061440000 XMR in fees, and 1.234567891234 XMR would leave this wallet in 2 payments.` |
| `1 of 1 payments **were** prepared` (M3) | plural for one | one payment `was` prepared, two `were`; and the doctor's `payment(s)` is `payments()` |

### 12.3 One fact, one wording

| Where | Was | Is |
|---|---|---|
| the money ledger's time (S9) | `09-16 14:01` in the TUI's list, detail view and note title; `2026-09-16 14:01` in `wallet history` | `fmonero_core::when`, read by all four. `WHEN` is 16 cells where the pane has them and 11 where it does not, and the addresses pane gave up 2% of the Wide tier to make the room |
| `wallet history`'s header (M5) | `date  what  amount XMR  fee XMR  confirms  spendable  note` | `when  what  amount XMR  confirms  spendable  note  fee XMR` — the TUI's own order and first word |
| the bar's address shortening (M4) | eight and **six**, against the TUI's eight and eight | eight and eight, held by `plugin-smoke` |
| the bar's wallet row (M6) | `not running` for a wallet answering on a service fmonero did not write | `reachable` is the predicate; `running (not fmonero's)`, and `not answering` for a unit that is up and silent |
| the two "pick another node" actions (M19) | `Pick another one on the Node screen.` twice, under two bullets, on the Node screen | `Press l to use your own node instead.` and `Pick another node in the list and press Enter.` |
| `status` in public-node mode (M20) | the node unchecked twice in three lines | the badge states it; the footer is the hint alone |

### 12.4 Room, and what a reader is left with

| Where | Was | Is |
|---|---|---|
| the privacy consent modal (S6) | three sentences and two blank rows: `Enter continues · Esc cancels` was off screen at 120x32 and 150x40 | the overlay wraps its own text and asks for the rows it draws |
| the widget's warning rows (S7) | the TUI's keymap — seven keys the panel does not bind, two that mean something else there | `panel_action`, from `status.rs`'s `PANEL_ACTIONS`, naming only keys the panel binds and controls it draws |
| the send confirm and `Tab` (S12) | nothing moved and nothing was said, under a help overlay promising `Tab` always works in a pop-up | a row reading `Esc first, then Tab.`, and the overlay's first row says a pop-up holding unfinished work asks for `Esc` first |
| `● Synced` over `last block 3d 01h ago` (M7) | two facts a reader cannot reconcile | `Synced means it has every block its peers have offered. A last block this old means none has been offered for a while.` |
| the waiting pane (M8) | `14 payments` · `they carry 0.000427 XMR in fees altogether` | `14 payments across the network` · `their fees  0.000427 XMR altogether` — and `status` and the bar say it the same way |
| the fee-level rows (M9) | `XMR a byte` on two consecutive rows | `XMR a byte` then `and` |
| the Log screen's title (M10) | the log path elided mid-token, in the only place the file is named | the title keeps it when it fits whole; the pane's first row carries it hard-chunked when it does not |
| the doctor's long paths (M11) | one over-long line the terminal wrapped at column 1, with no marker | `wrap_to` hard-chunks a single token wider than the line |
| the payments filter (M12) | `filter by id, address or note` | `filter by address or note` — ids are never displayed |
| the empty wallet panel (M13, M14) | one chip, no sentence saying there are no wallets, and two status rows with no next step | `There are no wallets on this machine yet.`, plus `Opening or creating a wallet starts it` and `press 1 for the Node screen` |

### 12.5 `--help` and clap

| Where | Was | Is |
|---|---|---|
| `wallet sweep` (M15) | `Sweep an account to one recipient` — the word defined by itself | `Send everything in one account to one address, in as many payments as it takes` |
| `wallet set-daemon` (M16) | the command's **name** carried rule 7's first banned word | `wallet use-node`, with `set-daemon` kept as a hidden alias |
| three `///` em-dashes (M17) | `node list`, `node remote <ADDR>` and `node log --follow` | periods, and `node list` no longer uses `probe` and `checked` for one act in one sentence |
| clap's own voice (M18) | `unrecognized subcommand 'note'; tip: a similar subcommand exists: 'node'` and `unknown settings key 'nope'` | `"note" is not an fmonero command. Did you mean "node"?` and `"nope" is not one of fmonero's settings.` |

### 12.6 The two that were not wording

Both are in §2 above: the Send screen's pending refusal now names `P` and the Wallet screen (S8), and
local mode with no units written says `fmonero has not set up your node yet. Press S to set it up.`
on the hero and on all three keys that used to claim another program owned the node (S10).

S11 needed no edit: round 4's own commit had already corrected `README.md`, `agent.rs`'s `BUILD_NOTE`
and DESIGN §1.1 to what the journal shows, and V18's structural half — `FMONERO_STATE_DIR` gating
`install` but not `start`/`stop` — was closed with a test (`units::tests::a_sandbox_without_the_fixture_may_not_start_or_stop_a_real_unit`).
What round 5 adds is the rule where the next agent writing a sandbox will read it: `CLAUDE.md`.
