# fmonero test fixtures

Everything here is **derived** from the Monero source at tag **v0.18.5.1** — the version Arch ships —
and **nothing was captured from a run**. No `monerod` and no `monero-wallet-rpc` exist on this machine
and `sudo` needs a password, so no agent could start one (DESIGN.md §1.1). That single fact is why
this directory has one provenance class where fproton's has two, and why every row these fixtures back
is **AMBER** in DESIGN.md §4b: they prove fmonero's wire format and fmonero's parsing, and they prove
nothing whatsoever about a real daemon's behaviour.

Two consequences worth stating before the tables:

- **A fixture cannot be wrong in the way a capture can.** A capture is evidence; these are
  reconstructions, and their only warrant is the `file:line` beside each field. Every one of those
  citations was read in the shallow clone at §1.6, and the cross-fixture arithmetic (heights,
  `confirmations`, `locked`, the sweep's amount/fee sums, the 5 GiB rounding, the base58 leading-digit
  sets) was recomputed rather than copied from DESIGN.md.
- **Replace them with captures once Monero is installed, and say so when you do.** The provenance
  header is the marker: delete it in the same commit that pastes a real capture in.

## Provenance header

`DESIGN.md` §1.1 fixes the form. Every derived file begins with the line

```
# derived from monero v0.18.5.1 source, not captured from a run
```

followed by further `#` lines naming the `file:line` each field came from. **Consumers skip leading
`#` lines.** JSON cannot carry a `#` comment, so for a `.json` fixture that text is the value of a
top-level **`"_provenance"`** string whose **first line is that exact sentence**, and the loader
deletes the key — §1.1's own words.

Mechanically, with **no exceptions** — all 82 JSON fixtures, `doctor/*/expect.json` included, and all
18 derived text fixtures:

```bash
# JSON fixtures
jq -re '._provenance' <file> | head -1     # must equal the sentence above
# text fixtures (systemd units, config.toml, systemctl.txt, monerod.conf)
head -1 <file>                             # must equal the sentence above
```

Where a file has a second source — the kernel's `/proc` formats, or a DESIGN.md section rather than the
Monero tree — that goes on **line 2**, never in place of the sentence. A per-file variant of the
sentence is what an exclusion in the checker grows out of.

`fmonero-mock` drops **every** top-level key beginning with `_` before serving, so `_provenance` and
`_request_address` never reach a client. No Monero RPC response field starts with an underscore, so
the rule cannot swallow a real one.

Four kinds of file have **no** header, and each is unreadable with one: `input/proc/*/cmdline` and
`input/proc/*/comm` (byte-faithful `/proc` samples, NUL-separated), `input/proc/net/tcp` (a
kernel-format table), and `input/history.bin` (postcard bytes). Their provenance is in the sibling
`expect.json`.

## Node RPC — `node/`

Field names and types are the `*_commands_defs.h` structs, not the handler code. Where a struct
**member** has no `KV_SERIALIZE` entry it never reaches the wire and is absent here; `connection_info`
is the case that bites (see Deviations). Every response inherits either `rpc_response_base`
(`status`, `untrusted` — `core_rpc_server_commands_defs.h:101-112`) or `rpc_access_response_base`,
which adds `credits` and `top_hash` (`:124-136`); the table says which.

Scenario files are named `<method>-<scenario>.json` and are selected by `--scenario <scenario>`.

| File | Struct (`src/rpc/core_rpc_server_commands_defs.h`) | Base | Notes |
|---|---|---|---|
| `get_info.json` | `COMMAND_RPC_GET_INFO::response_t` :706-746 | access | The synced mainnet node of §5's worked example. `target_height` is **0 because synced** (`core_rpc_server.cpp:523`), which the sync percentage must special-case. |
| `get_info-syncing.json` | same | access | `--scenario syncing`. Every field is **real**: `get_info` has no `CHECK_CORE_READY()` gate, unlike `get_last_block_header`. |
| `get_info-restricted.json` | same | access | `--scenario restricted`. Every poisoned field with the line that poisons it (`core_rpc_server.cpp:528-569`). `free_space` is 2^64-1, `database_size` is rounded up to a 5 GiB multiple, `tx_pool_size` is a **lower bound** rather than poison (`:527`). |
| `get_last_block_header.json` | `..._GET_LAST_BLOCK_HEADER::response_t` :1230-1238, `block_header_response` :1166-1215 | access | `pow_hash` empty: the request sends `fill_pow_hash: false`. `height` is `get_info.height - 1`. |
| `get_last_block_header-syncing.json` | same | access | `--scenario syncing`. **`status: "BUSY"` with an all-zero header** — the `CHECK_CORE_READY()` answer (`core_rpc_server.cpp:2453`, macro at `:477`), which is the answer for the *whole* initial sync. Without it nothing in this plan can fail on a 56-year-old block. |
| `sync_info.json` | `..._SYNC_INFO::response_t` :2426-2444 | access | `peers[]` is an array of `{ info: connection_info }` (:2396-2403), **not** of `connection_info`. `overview` is a string that looks like an array. |
| `get_connections.json` | `..._GET_CONNECTIONS::response_t` :1737-1760, `connection_info` (`cryptonote_protocol_defs.h:47-116`) | bare | `ip`, `port`, `peer_id`, `connection_id` are **strings**. |
| `get_peer_list.json` | `..._GET_PEER_LIST::response_t` :1387-1397, `peer` :1338-1369 | bare | `peer.ip` is a **u32 number** and `peer.port` a **u16 number** — the opposite of `connection_info`. The `ip` value is **network byte order in a host `uint32_t`**: `get_ip_int32_from_string` is literally `inet_addr` (`string_tools.cpp:79-86`), so `203.0.113.17` is `0x117100cb` = 292618443, **not** 3405803793. The arithmetic is written out in the file. |
| `get_bans.json` | `..._GETBANS::response_t` :2023-2031, `ban` :2002-2013 | bare | `seconds` on the way out is the **remaining** ban (`core_rpc_server.cpp:2750-2766`). `ip` uses the same `inet_addr` convention. |
| `set_bans.json` | `..._SETBANS::response_t` :2063-2068 | bare | Echoes nothing; `status: "OK"` is the whole answer. The SETBANS `ban` struct (:2037-2050) differs from GETBANS's — it adds a `ban` bool. |
| `get_transaction_pool_stats.json` | `..._GET_TRANSACTION_POOL_STATS::response_t` :1725-1733, `txpool_stats` :1680-1713, `txpool_histo` :1669-1678 | access | The file name is the **wire** name for both transports (`core_rpc_server.h:129`); naming it after `NodeClient::get_txpool_stats` would be a silent 404. |
| `get_fee_estimate.json` | `..._GET_BASE_FEE_ESTIMATE::response_t` :2266-2278 | access | The json_rpc method is `get_fee_estimate` though the struct says BASE_FEE (`core_rpc_server.h:176`). **`fee` IS `fees[0]`** since HF 15 (`core_rpc_server.cpp:3011-3012`), i.e. the cheapest tier — never a default. `fees[]`'s four entries ARE priority levels 1..4. |
| `get_version.json` | `..._GET_VERSION::response_t` :2196-2212, `hf_entry` :2183-2194 | bare | `version` is the **RPC** version, `(3 << 16) \| 16` = 196624 (:91-94), not the software version. `hard_forks[]` is the mainnet table verbatim from `hardforks.cpp:34-76`, ending at **v16 @ 2689608**. |
| `hard_fork_info.json` | `..._HARD_FORK_INFO::response_t` :1974-1994 | access | `version` is the **current** fork and the six voting fields describe the **next** one (`core_rpc_server.cpp:2738-2741`). Today they coincide, which is why the file below exists. Values derived from `hardfork.cpp:408-423` and `hardfork.h:42-52`; `state: 2` is `Ready`. |
| `hard_fork_info-voting.json` | same | access | `--scenario voting`. **Addition** — see Additions. A hypothetical v17 being voted on while v16 is current, so `version != voting` and §5c's "keep the two apart" is testable. |
| `get_net_stats.json` | `..._GET_NET_STATS::response_t` :807-823 | bare | **Lifetime counters, not rates.** Two samples of a static fixture differ by nothing, so a rate computed against it is 0 bps — correct arithmetic on constant input, and why a real rate is RED. |
| `get_limit.json` | `..._GET_LIMIT::response_t` :1865-1875 | bare | Fields are **`uint64_t`**. Open even under `--restricted` (`core_rpc_server.h:135`), unlike `/set_limit`. |
| `set_limit.json` | `..._SET_LIMIT::response_t` :1894-1904 | bare | Fields are **`int64_t`** — `-1` means "reset to default" on the way in. Echoes one fixed pair; a test that needs the echo to match its own argument asserts on the recorded request. |
| `out_peers.json`, `in_peers.json` | :1923-1931, :1949-1957 | bare | `set` is `KV_SERIALIZE_OPT` defaulting to **true** (:1917, :1943), so an omitted `set` **writes**. `peers_count()` must send `set: false`; the assertion is on the log. |
| `stop_daemon.json`, `set_log_level.json` | :1827-1832, :1488-1493 | bare | **Additions** — see Additions. |
| `error-restricted.json` | — | — | A whole JSON-RPC envelope, the only one in `node/`. The `-32601` of `http_server_handlers_map2.h:280-285`. The mock overwrites `id` with the request's. |

## Wallet RPC — `wallet/`

**No wallet response carries `status` or `untrusted`**: the wallet RPC has no `rpc_response_base`, so
§1b.3's `"BUSY"` rule is a node rule only. Lines are
`src/wallet/wallet_rpc_server_commands_defs.h`.

| File | Struct | Notes |
|---|---|---|
| `get_balance.json` | :102-119, `per_subaddress_info` :77-100 | **`per_subaddress` holds only FUNDED subaddresses** (`wallet2.cpp:7037-7060`) — one entry here against three addresses in `get_address.json`, deliberately, because §5 sources `subaddress_count` from `get_address` and §7a joins the two. `balance` is §5's worked example, 1234567891234 pico. |
| `get_balance-locked.json` | same | `--scenario locked`. **Addition.** `unlocked_balance < balance`, `blocks_to_unlock: 6`, so `wallet_locked_funds` and §6.5's "locked for N blocks" row have an input. |
| `get_address.json` | :151-160, `address_info` :136-149 | Three addresses: primary (used), a labelled subaddress (used), an unused one. This call is the source of `primary_address`, `subaddress_count`, and **the wallet's own nettype** — the §2a.6 send-path authority. |
| `get_accounts.json` | :298-309, `subaddress_account_info` :279-296 | One account; `base_address` equals `get_address.json`'s primary, so the two join. Cold-path call. |
| `get_height.json` | :464-470 | One key. **The clock for `confirmations` and `locked`** — `m_wallet->get_blockchain_current_height()` (`wallet_rpc_server.cpp:938-944`), the same one wallet-rpc recomputes them against. Equal to `node/get_info.json`'s height, so `blocks_behind` is 0. |
| `get_height-behind.json` | same | `--scenario behind`. **Addition.** 3535000, i.e. 6122 behind, so all four consumers of `WALLET_BEHIND_BLOCKS` have an input. |
| `get_transfers.json` | :1634-1649, `transfer_entry` :1446-1487 | Seven rows covering **all six** `type` values. `block` (coinbase) sits **inside the `in` list** (`wallet_rpc_server.cpp:501`) — the row that breaks a five-variant enum. One `in` row has `unlock_time: 1900000000`, a **timestamp**, and one has `3539060`, an absolute **height**: both branches of §1b.4's formula. `subaddr_index` is the object `{major, minor}`. |
| `get_transfer_by_txid.json` | :1667-1676 | The §2a.4 resolve step's answer. Its txid is `transfer-do-not-relay.json`'s, `relay_tx.json`'s and `get_transfers.json`'s `pending[0]` — so a build→relay→resolve leaves nothing behind. |
| `get_transfer_by_txid-failed.json` | same | `--scenario failed`. **Addition.** A `failed`-type row, which §2a.4 counts as **found**. |
| `transfer-do-not-relay.json` | `single_transfer_response` :567-594 | The build response. **`tx_metadata` is what `relay_tx` wants; `tx_blob` is not** (`wallet_rpc_server.cpp:1914-1943`). `tx_key` and `tx_blob` are empty because the request sends `get_tx_key: false, get_tx_hex: false` — the tx secret key is a secret (§2b.2). `amounts_by_dest` is the wrapper `{amounts: [...]}`, not a bare array. Served for a bare `transfer` too (see `FIXTURE_ALIAS`). |
| `relay_tx.json` | :970-977 | `tx_hash` **equals** the built txid, so §2a.9's comparison passes. |
| `relay_tx-mismatch.json` | same | `--scenario mismatch`. **Addition.** The same txid with its first two characters swapped, so §2a.9's failure path is reachable. |
| `sweep_all.json` | `split_transfer_response` :634-661 | **Two** transactions, so the list path and §2a.4's partial-sweep rule have an N > 1. All eight lists are index-aligned, which nothing on the wire enforces. `tx_key_list`/`tx_blob_list` are **empty lists**, not lists of empty strings. Amounts plus fees sum exactly to `get_balance.json`'s balance. |
| `validate_address-{standard,subaddress,integrated,wrong-nettype,invalid}.json` | :2592-2607 | Dispatched on `_request_address`; `-invalid` is also the fallback. Every verdict is upstream's own assertion from `tests/functional_tests/validate_address.py:82-92`. `-wrong-nettype` is a **valid** stagenet address, which is only sayable because the request sends `any_net_type: true`. `-invalid` is a valid address with one character changed: it passes `address::parse` and fails only the checksum, which is why §2a.6 validates twice. |
| `get_address_book.json` | :2006-2013, `entry` :1993-2004 | Four rows chosen so all three `@label` outcomes are reachable: `@savings` resolves, `@cold` misses (exit 6), `@exchange` matches **two** rows case-insensitively (exit 7). |
| `add_address_book.json`, `delete_address_book.json` | :1942-1949, :2029-2033 | **Additions.** |
| `refresh.json`, `create_address.json`, `get_tx_notes.json` | :2066-2075, :224-237, :1272-1279 | **Additions.** |
| `open_wallet.json`, `close_wallet.json`, `create_wallet.json`, `set_daemon.json`, `auto_refresh.json`, `store.json`, `label_address.json`, `set_tx_notes.json` | :2224-2228, :2244-2248, :2200-2204, :2643-2647, :2093-2097, :990-994, :255-259, :1252-1256 | All **empty** responses; the served result is `{}`. Each file's header carries the request shape and the trap that matters — `set_daemon` cannot precede `open_wallet` (`:4692`), `create_wallet` leaves the wallet **open** (`:3639`), `open_wallet` returns a bad password as **-1** not -22 (`:3705-3712`), `store` is why a SIGKILL after a relay does not orphan the record. Only `open_wallet` is named in §8; the rest are **Additions**. |
| `get_version.json` | :2563-2572 | `(1 << 16) \| 31` = 65567 → `"1.31"`. A **different shape** from the node's `get_version`, which is why `--role` exists. |
| `error-not-open.json`, `error-not-enough-money.json`, `error-wrong-address.json` | error codes header :46, :50, :35 | **Reference shapes, never served.** The dispatcher keys on wire method names and none of these is one. They pin the exact code and message and are the source of the `--fail` arguments that produce them. Messages are upstream's verbatim: `"No wallet file"` (`wallet_rpc_server.cpp:480-485`), `"not enough money"` (`wallet_errors.h:518` — `what()`, *not* `to_string()`, so the amounts are **not** in it), `"Invalid address"` (`:703-705`). |

## `address-forms.json`

One object per row of `address::PREFIXES` (§5b) — **the nine forms** — under a `forms` key, so the
file can also carry `_provenance` (§8 fixes the six keys of each object, not the container; the count
is `jq '.forms | length'`). This is the file that makes the FCMP++/Carrot fork one row plus one
object (§1b.1).

Prefixes are `cryptonote_config.h:227-229` / `:270-272` / `:285-287`. Lengths follow from
`encoded_block_sizes[]` (`base58.cpp:48`): 69 bytes → 95 characters, 77 → 106.

**`leading` is an array because the leading character is a set, not a character.** It is the base58
digit of the whole first 8-byte block — the prefix varint plus the first *seven* bytes of the public
spend key — so a prefix's block range can straddle a digit boundary. Recomputed for this file by
dividing `[p<<56, (p<<56)|0x00FF_FFFF_FFFF_FFFF]` by `58^10`:

| network | standard | integrated | subaddress |
|---|---|---|---|
| mainnet | `4` | `4` | `8` |
| testnet | **`9` or `A`** | `A` | `B` |
| stagenet | `5` | `5` | `7` |

Only testnet standard (prefix 53) spans two digits; the switch is at first-seven-bytes
`0xceb74b07d8c400`, so a hard leading-character check would reject roughly one testnet address in
five. Note that within a network the leading character does **not** separate standard from
integrated — only `len` does.

**Every `example` is a real, checksum-valid address from Monero's own test suite**, so each is a valid
input to a real `validate_address` as well as to `address::parse`. Eight come from
`tests/functional_tests/validate_address.py:67-81`, where upstream asserts exactly the
net/integrated/subaddress verdict recorded here (`:85-90`); the mainnet subaddress is
`src/simplewallet/simplewallet.h:56`, upstream's `donate@getmonero.org` address, asserted at
`validate_address.py:97-107`. The testnet standard example begins `9`; no upstream vector begins `A`
for that form, so the `A` half of the set rests on the arithmetic above. **The right assertion is
`leading.contains(example[0])`, never `example[0] == leading[0]`.**

## Doctor cases — `doctor/<check-id>[__<variant>]/`

`{input/, expect.json}` for the **seven parsing detectors** of §7.1, 22 cases in all. The
`<check-id>__<variant>` suffix follows fproton's precedent (`conflicting_tunnel__exit_node`).

`expect.json` pins `{id, group, severity, class, status, fixable}` exactly, plus **`detail_contains`**
— substrings the detail must contain, never the whole string, because pinning prose makes a fixture
fail on a reworded message and that is the assertion nobody keeps. Two meta keys say how to set the
case up: **`_env`** (values are paths *relative to the case directory*; `FMONERO_STATE_DIR` is always
also required and is the harness's own temp dir) and **`_settings`** (dotted key → value, **written
into `config.toml` directly**, not through `fmonero settings set`, because `node_layout__remote` needs
a non-loopback `node.url` that `settings set` refuses outright).

Where a `status` is Phase 0's reading of §7.1 rather than a quotation from it, the file says so.
§8 hands this directory to the **Phase 2 doctor agent**, which may edit it — the notes exist so it can
disagree with a reason instead of guessing.

| Cases | Drives | What the input is, and the trap it sets |
|---|---|---|
| `node_layout`, `__system_unit`, `__remote`, `__none` | `FMONERO_SYSTEMCTL_FIXTURE` | `<unit> <is-active> <is-enabled> <exists> <main-pid>` lines. The format has no user/system column, so the **unit name** discriminates. `__remote` deliberately leaves `fmonero-monerod.service` present **and active**, so a detector that reads the unit file and forgets the URL answers `user_unit` and this is the only case that catches it. `system_unit`'s second disjunct (`/etc/monerod.conf`) is RED: no agent can write `/etc`. |
| `node_data_dir_single_writer`, `__free`, `__via_config_file` | `FMONERO_PROC_ROOT` | Byte-faithful `/proc/<pid>/cmdline`: NUL-separated with a trailing NUL. **These files contain NUL bytes — a text editor will corrupt them; regenerate instead.** `__free` plants `monerod-not-really --data-dir <the same dir>`, so a `starts_with("monerod")` match fails the case. `__via_config_file` names the data dir **only inside `--config-file`**, which is §2i's second clause and the one an implementer skips — and the clause whose omission starts a second writer on a live LMDB. The path is `/var/lib/monero` and **does not exist**, because `canonicalize` fails on a missing path and a detector must fall back to lexical normalisation. The **fcntl half cannot be fixtured**: a held POSIX record lock is live kernel state, and §2i's own forked-child test covers it. |
| `ports_owned`, `__foreign`, `__external` | `FMONERO_PROC_ROOT` + `FMONERO_SYSTEMCTL_FIXTURE` | A kernel-format `/proc/net/tcp`: `local_address` is hex IP in `inet_addr` order (`127.0.0.1` → `0100007F`) and **the port is HEX** (18081 = `46A1`). Reading it as decimal finds nothing and the check reports `pass` on every machine. `st = 0A` is LISTEN; the last column is the socket inode, which `/proc/<pid>/fd/<n>` symlinks to as `socket:[inode]`. **Those symlinks are deliberately dangling** — git stores the target, which is all the scan reads, so copy with `cp -a`, never `cp -rL`. |
| `units_present`, `__absent` | (see below) | **A passing fixture for this check cannot be written**, and saying so is part of the fixture: §7.1 compares the units **byte for byte** against `units::render_*`, whose bytes embed `$HOME`, `wallet.port` and the network segment. So `units_present` is the **drifted** case (`Restart=always`, a forbidden `Wants=`, a dropped `StateDirectoryMode=0700`) and `__absent` is the empty one; the passing case belongs to the Phase 2 render→write→re-read round trip. Neither drift is a flag a grep would find, which is the point of having a byte comparison at all. |
| `unit_no_password_in_argv`, `__password_file`, `__rpc_login`, `__log_level` | (see below) | Clean units plus three one-line defects. `__log_level` is **the half of §7.1's condition a detector forgets** — the row requires the password-flag grep **and** `--log-level 0` on both units. The node unit carries **no** `--log-level` at all and that is correct (it comes from the config file), so ABSENT must satisfy the condition. And the node unit's own `rpc-login=` **inside its `--config-file` is correct and must not be reported**: the check is on `ExecStart`'s argv, because `/proc/<pid>/cmdline` is world-readable. |
| `settings_valid`, `__ok`, `__unparsable` | `FMONERO_CONFIG_DIR` | Four defects of four different kinds (bad enum value, out-of-range `wallet.priority` that `wallet2` would silently clamp to the *most expensive* tier, `timeouts.node_sec = 0`, an unknown key one edit from `wallet.account`), plus one **non-defect that must not be reported**: `node.url` on port 18099, which §2c permits because its rule is about the host, not the port. `__ok` is the all-defaults document and is what catches a detector that reports `warn` unconditionally. `__unparsable` is three real TOML syntax errors — the case that decides whether the one command that repairs `config.toml` can run at all on a machine whose `config.toml` is broken. |
| `history_cache_fresh`, `__stale`, `__bad_magic` | `FMONERO_CACHE_DIR` | A **postcard-encoded §5b `CacheHeader` and no body** — 30 bytes, with the byte-by-byte decoding in `expect.json`. Header-only because the body is a `Vec<Transfer>` whose layout is Phase 1a's and postcard is not self-describing; so **this is not a loadable cache**, and `history::load` must degrade rather than error. `high_water` is 3541112, not 3541122, because §5b caps the cache at `wallet_height - 10`. `wallet_fp` is `sha256(primary_address)[..16]` — the address is public (§2b), so no secret is in the file. |

Two of those seven have **no frozen way in**, and that is a gap in DESIGN.md rather than a decision
Phase 0 may take — see "Design gaps reported" below.

## `fmonero-mock`

`crates/fmonero-mock/src/main.rs`. A thin `main()` over `serve(routes)`.

```
fmonero-mock --role node|wallet --port N --fixtures DIR --log FILE
             [--restricted] [--auth USER:PASS] [--delay-ms N] [--rotate-nonce-after N]
             [--scenario NAME] [--fail METHOD=CODE:MESSAGE]...
```

**Fixture lookup**, in order: `<method>-<scenario>.json` when `--scenario` names one that exists,
then the method's own file through `FIXTURE_ALIAS` (one entry: `transfer` →
`transfer-do-not-relay.json`, because §8 names the file with a scenario suffix while the wire method
is `transfer`, and §2a.2 means fmonero never calls it any other way). `validate_address` instead
matches the request's `address` against each `validate_address-*.json`'s `_request_address`, sorted so
the order is deterministic, falling back to `-invalid`. **No fixture, no answer**: an unknown json_rpc
method gets `-32601` (which is what `monerod` answers too) and an unknown legacy path gets 404. There
is deliberately no "answer `{}` for anything unknown" fallback, because that hides a missing fixture
behind a passing test.

**Why each flag exists** (§8), and what breaks without it:

| Flag | Without it |
|---|---|
| `--role` | `get_version` and `get_height` are answered by both daemons with different shapes; a test needing both runs two instances. |
| `--scenario` | The syncing/restricted/behind/locked/failed/mismatch/voting paths have no input at all. |
| `--delay-ms` | §2a.11's two-process test becomes a coin flip: against an instant mock the first `send --yes` finishes build→relay→resolve before the second reaches `flock`, both legitimately succeed, and the only test of the one interlock that can double-spend gets marked flaky and then deleted. |
| `--fail` | The §1b.4 error-code → exit-code table — a money surface — cannot be tested. `--restricted` covers only `-32601`. On a **legacy** endpoint the CODE is ignored and the MESSAGE becomes `status`, deliberately: a legacy endpoint has no JSON-RPC error channel and `status` **is** the daemon's error channel (§1b.3). |
| `--restricted` | The restricted-node degradation of §5 has no input. Node-only: with `--role wallet` the mock **refuses to start**, because the wallet unit never sets `--restricted-rpc` and a silently accepted flag reads as coverage. |
| `--auth` | Roughly half of §4b: every wallet row needs it, together with a matching `$FMONERO_STATE_DIR/run/monero-wallet-rpc.<port>.login` (§2c). |
| `--rotate-nonce-after` | The `nc` reset of §2c — the one non-obvious property of `digest.rs`, whose absence silently doubles the §2l budget. |
| `--log` | Required. §2a's central assertions are all on the recorded request: no `unlock_time` or `payment_id` in any `transfer` body (§2a.8), `subaddr_indices_all: true` in every `sweep_all` body (§2a.10), `set: false` on a read-only `out_peers`, `filter_by_height: true` beside `min_height`. |

**Keep-alive and threads.** HTTP/1.1 with `Content-Length` on every response and `Connection: close`
on none; successive requests are served on one socket until the client closes it. The accept loop
spawns **one std thread per connection**, because fmonero holds an idle pool of one and keeps its
socket for the life of the process (§2c) — a single-threaded loop parks every second client in the
listen backlog behind a connection that is never closed, and the tests that would hang are exactly
§2a.4's and §2a.11's, the ones that prove `pending.json` survives concurrent writers. Two mock
instances do not rescue it: both fmonero processes read the same `settings.wallet.url`.

**Stateless** apart from the per-connection Digest session and the log. `open_wallet` is recorded,
answers `{}`, and gates nothing — a `-13` comes from `--fail`, never from mock state, because state in
a mock is a second implementation of the thing under test.

### Digest

epee's `digest_algorithms` is a one-element tuple holding `md5_` (`http_auth.cpp:155`), advertised as
`MD5` (`:108-110`) and implemented with `EVP_md5()` (`:144`). There is no SHA-256 variant, so MD5 is
inline in the mock — §8 caps it at `serde_json` plus std. No security property rests on it; it
authenticates a test daemon. The four facts of §1b.7, all reproduced exactly:

1. **A 401 carries TWO `WWW-authenticate` headers**, `algorithm=MD5` and `algorithm=MD5-sess`
   (`add_challenge` loops `for (unsigned i = 0; i < 2; ++i)`, `:665-691`). A real server accepts
   either, and `-sess` is a **different HA1** — `md5(key:nonce:cnonce)` (`:548-551`) — so a client
   that reads "the" header and lands on the second one fails every request. The mock accepts **only**
   the MD5 form, so an MD5-sess answer is refused here too.
2. **`qop=auth` is unconditional** (`fvalue = "Digest qop=\"auth\""`, `:671`); the RFC 2069 no-`qop`
   branch is server-side dead code and fmonero does not implement it (§2h).
3. **`nc` belongs to the nonce, not to the client.** The server zeroes its counter whenever it issues
   a nonce (`:749`), increments before each verify (`:734`), then requires equality (`:335-345`) — so
   the first request on a new nonce must carry `nc=00000001`, and a monotonic client counter mismatches
   forever at a `stale` 401 plus a retry per request.
4. **No `opaque`** (`:676-683`).

Two details are byte-exact on purpose, and a client that is sloppy about either fails here as it would
against a real daemon: the header name is spelled **`WWW-authenticate`** (`:96`), and `add_field`
pushes a **bare comma with no space** (`:206-210`). The realm is `monero-rpc` (`:97`).

`--rotate-nonce-after N` rotates **lazily**: after N authorized requests the next request presents a
nonce the server no longer accepts and takes the ordinary stale path — a 401 with `stale=true` and a
fresh nonce — which is what a real epee server does when its nonce turns over.

### Request log

One JSON object per line. §8 fixes `{seq, ts_ms, path, method, body}`; `conn`, `status` and `nc` are a
**documented superset** (see Deviations). `body` is the parsed JSON when it parses — so a test can
assert `.body.params.subaddr_indices_all` — and the raw text as a JSON string otherwise. One
`Mutex<File>` guards it, the only state shared between connection threads, so lines stay whole and
`seq` stays ordered.

## Deviations from DESIGN.md

Recorded here and in `decisions/phase0-mock.md`, and repeated in Phase 0's return text.

1. **`connection_info.ssl` is omitted.** §1b.3 lists `ssl` among `connection_info`'s fields. It is a
   struct member (`cryptonote_protocol_defs.h:52`) with **no `KV_SERIALIZE` entry** in the map at
   `:89-115`, so it never reaches the wire. The fixture follows the wire. A `Connection` struct with a
   required `ssl` field would fail to deserialize every row of a real `get_connections`.
2. **`get_accounts`' request has a third field, `regexp`.** §1b.4's row lists `tag` and
   `strict_balances`; v0.18.5.1 also has `regexp` (`:269`, `KV_SERIALIZE_OPT` false at `:274`).
   fmonero sends none of the three, so nothing changes — but the row is incomplete.
3. **The request log carries three keys beyond §8's five.** `conn` (the accept-order socket id),
   `status` (the HTTP status answered) and `nc` (the `nc` the Authorization header carried, or null).
   Each is needed by a gate §8 itself specifies: without `conn`, "two requests on one connection" and
   "two clients with distinct seq" cannot tell one socket from two; without `status`, the 401→200
   handshake is not assertable from the log; without `nc`, §2c's increment and reset assertions are
   inferred from a pair of 200s rather than read. Every §8 key keeps its stated meaning.
4. **`address-forms.json` wraps its nine objects in a `forms` key.** §8 fixes the six keys of each
   object and the count; a bare top-level array could not also carry `_provenance`, which §1.1
   requires of a JSON fixture. The count is `jq '.forms | length'`.
5. **The mock is 597 rustfmt-formatted lines, not "under 500" (§8).** Honest breakdown: **425 lines of
   non-test, non-comment code**, 89 comment lines carrying the `file:line` citations, 52 lines of
   self-test. `rustfmt` at the workspace's default settings (no `rustfmt.toml`, `max_width = 100`,
   default `use_small_heuristics`) is what fixes the number — the same source is 100 lines shorter
   before formatting, and `cargo fmt --check` is a Phase 1b/3 gate, so the formatted count is the real
   one. The single largest item forced by §8 itself is the inline MD5: capping dependencies at
   `serde_json` plus std costs ~55 lines for the K table and the compression function. Long-form
   rationale was moved into this README specifically to claw lines back. Remaining paths to <500 all
   delete something real: the three self-tests (which prove the MD5 and the RFC 2617 formula the whole
   Digest half rests on), or the citations. Phase 0 chose to keep both and report the overage.
6. **`--fail` on a legacy endpoint ignores the CODE.** §8 says `--fail METHOD=CODE:MESSAGE` "makes
   that method answer that JSON-RPC error". A legacy `POST /<name>` has no JSON-RPC error channel, so
   the MESSAGE becomes the response's `status` field — which *is* the daemon's error channel there
   (§1b.3, e.g. `set_limit`'s `"Invalid parameter"`) — and the CODE is dropped.

## Additions beyond §8's deliverable list

Each is a file §8's list does not name, added because a test DESIGN.md names elsewhere has no input
without it. None replaces a listed file.

| File | The assertion that needs it |
|---|---|
| `node/hard_fork_info-voting.json` | §1b.3/§5c require `node info` to keep the current fork version and the voted-on one apart. On mainnet today they coincide, so against `hard_fork_info.json` a consumer that conflated them passes everything. |
| `node/stop_daemon.json` | §2i's named test asserts `node down` **reaches the mock** under `user_unit`; `node_session::down` issues `stop_daemon` first so LMDB closes cleanly. |
| `node/set_log_level.json` | §7a binds `L` to `set_log_level`, and §2i's gate test asserts **zero** requests under `system_unit` — which proves nothing unless a request would otherwise be answered. |
| `wallet/relay_tx-mismatch.json` | §2a.9's test needs a **successful** `relay_tx` whose `tx_hash` disagrees with the build. `--fail` produces an error instead, a different code path. |
| `wallet/get_transfer_by_txid-failed.json` | §2a.4's test needs a **successful** response whose `type` is `failed`. Again not reachable with `--fail`. |
| `wallet/get_height-behind.json` | Four consumers of `WALLET_BEHIND_BLOCKS` (§5's warning, §2d's syncing classification, `wallet_daemon_link`, `wallet_daemon_auth`) all need a wallet that is behind. |
| `wallet/get_balance-locked.json` | §5's `wallet_locked_funds` and §6.5's "locked for N blocks" row. |
| `wallet/{close_wallet,create_wallet,set_daemon,auto_refresh,store,label_address,set_tx_notes,delete_address_book,add_address_book,refresh,create_address,get_tx_notes}.json` | §4b lists `wallet open\|close\|balance\|address\|new-address\|history\|refresh\|book\|notes\|set-daemon` and `wallet create` as AMBER rows verified against the mock, and §5c fixes their payloads. `set_daemon` and `auto_refresh` are steps 3 and 4 of `wallet_session::open`, and `store` and `set_tx_notes` are the last two steps of `send::relay` — without them every `wallet open` and every `wallet send --yes` 404s partway through, the send **after the money moved**. |

## Design gaps reported

**Six §7.1 checks need to read the two unit FILES, and §2i freezes no way to point them at a
fixture.** §2i's test seam is `FMONERO_PROC_ROOT` and `FMONERO_SYSTEMCTL_FIXTURE`, and
`Systemctl::Fixture` answers only is-active / is-enabled / exists / MainPID. But `units_present`,
`unit_loopback`, `unit_no_password_in_argv`, `unit_no_node_wants`, `wallet_dir_matches_unit` and
`wallet_port_consistent` all read `~/.config/systemd/user/fmonero-*.service` — and `units::install`
refuses to run at all when `FMONERO_STATE_DIR` is set (§4.2), so no test may write a real one.

`probes.rs` belongs to the Phase 2 doctor agent and §2k forbids Phase 0 from touching it, so the bytes
are in place and the two resolutions are named in the affected `expect.json` files: add
`FMONERO_SYSTEMD_USER_DIR` to `Paths`, honoured only when `FMONERO_STATE_DIR` is set (the same guard
§2i puts on its own two overrides — one field and one line), or have `probes.rs` resolve the unit
directory relative to `FMONERO_STATE_DIR`. The `_env` keys assume the first. **Until one is chosen
those six cases cannot run and must be recorded as skipped, never as passing** (§2j).

## Refreshing

Fixtures are reconstructions, so "refreshing" means re-deriving, not re-running:

- **JSON and text fixtures** — re-read the cited `file:line` in the v0.18.5.1 clone (§1.6) and edit.
  Never read the 2 800-line RPC headers whole; grep for the struct.
- **`input/proc/**` and `input/history.bin`** — **generated, not hand-written.** They contain NUL
  bytes, kernel-format tables and postcard varints; a text editor will corrupt them. The generator is
  ~120 lines of Python whose logic is spelled out in each `expect.json` (argv joined by NUL with a
  trailing NUL; `local_address` as `inet_addr`-order hex with a hex port and `st = 0A`;
  `socket:[inode]` symlinks; magic `FMHX` + varint version + 16 fingerprint bytes + varint high-water
  + zigzag-varint `built_at`).
- **`address-forms.json`** — the prefix table is `cryptonote_config.h`; the `leading` sets are
  recomputed by dividing each prefix's 8-byte block range by `58^10` over the alphabet at
  `base58.cpp:46`; the examples come from `tests/functional_tests/validate_address.py` and
  `src/simplewallet/simplewallet.h:56` and must not be invented — a hand-made address has no valid
  checksum and a real `validate_address` rejects it.

**Once Monero is installed, replace derived files with captures and delete the header in the same
commit.** Until then, no README, `--help` line or `robot-docs` sentence may describe anything these
fixtures back as verified against a real daemon (§2j).

## Gate

DESIGN.md §8's Phase 0 gate, run on 2026-09-17. The transcript, the two lines that could not be
closed by this phase and why, and the throwaway-crate workaround for `cargo build -p fmonero-mock` are
in **`decisions/phase0-mock.md`**, which Phase 1b folds into `DECISIONS.md`. Summary: **19 assertions,
19 passed, 0 failed.**
