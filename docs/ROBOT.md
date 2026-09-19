fmonero — robot docs

A Monero node + wallet front end. One JSON object per invocation on stdout; stderr carries
progress and diagnostics. A non-TTY stdout implies --json for every command that has a JSON
payload; `robot-docs` and `--help` are always plain text and `tui` exits 2 without a terminal.

START HERE
  fmonero capabilities        # exit codes, reasons, settings keys, warning ids, env, commands
  fmonero status --json       # the whole schema-1 Status document; starts nothing
  fmonero doctor --json       # the health idiom is: fmonero doctor --json >/dev/null; echo $?

BRANCH ON code_name + reason, NEVER ON PROSE
  Every failure is {ok:false, schema, code, code_name, error, hint, reason, detail, action}.
  `detail` carries the technical text `error` no longer does — the transport's own words, the
  RPC method, a serde message — and `action` is the in-app sentence the TUI renders instead of
  `hint`. Both are null for most errors and never absent.
  `status.warnings[]` carries the same pair plus one: `hint` for a terminal, `action` for the TUI
  and `panel_action` for the bar panel, whose keymap is a different one. Render the field that
  belongs to your surface; ids and reasons are what you branch on. `reason` is a
  stable discriminator WITHIN a code, because exit 3 alone covers eight unrelated conditions
  and the reflex for one of them (append --yes and retry) is wrong for the rest:
    3 needs_yes          -> re-run with --yes
    3 pending_send       -> a previous send may be in flight; DO NOT retry. Check
                            `fmonero wallet history --limit 5`, then
                            `fmonero wallet clear-pending --yes`
    3 send_in_progress   -> another fmonero process is between a build and a relay; wait
    3 fee_above_max      -> the built fee exceeded --max-fee; nothing was relayed
    3 foreign_node       -> this monerod is not fmonero's; it is never mutated
    3 data_dir_in_use    -> another monerod already holds the data dir
    4 not_installed     -> monerod / monero-wallet-rpc is not on PATH: `omarchy pkg add monero`.
                            `node up`, `node restart`, `wallet open` and `wallet create` answer
                            this in milliseconds and start nothing
    4 unit_missing       -> no fmonero unit is installed: `fmonero setup --units --yes`
    4 unreachable        -> the daemon is not listening
    4 auth_refused       -> it is listening and refused our credential
    4 remote_node        -> node.mode is remote and that command needs the LOCAL monerod:
                            `fmonero node local --yes`. Nine node commands answer this
    4 remote_address_rejected -> monero-wallet-rpc refused the remote address itself
    2 bad_remote_address -> the address is not [http[s]://]host:port with an explicit port
    2 remote_unset       -> node.mode = remote needs node.remote, and vice versa
   10 node_not_synchronized -> OUR node is behind; wait, then retry
   10 wallet_catching_up -> the wallet did not finish a refresh against the remote node inside
                            timeouts.refresh_sec; wait, then retry
    5 stalled            -> a call was killed at its deadline less than a minute ago: wait,
                            or pass --force for this one invocation
    5 rpc_error          -> the daemon answered and refused; retrying changes nothing
    5 restricted         -> the node runs --restricted-rpc and does not serve that method
    9 not_open           -> open a wallet
    9 bad_password       -> ask the user again; the same password will not work
   16 too_large          -> this send needs more than one transaction; use `wallet sweep`
    1 txid_mismatch      -> the relayed id differs from the built one. Check what landed.
  `fmonero capabilities` prints the full (code, reason) table, generated from the same
  constant the errors are built from.

MONEY, AND WHAT MAKES IT SAFE
  `wallet send` and `wallet sweep` build the transaction with do_not_relay FIRST, so the fee
  they print is the fee of a transaction that exists. Without --yes they print it and exit 3
  `needs_yes` — and in --json that refusal carries the COMPLETE send key set, so the fee is
  machine-readable without parsing stderr. --dry-run beats --yes and never relays.
    fmonero wallet send <addr|@label> <amount> --dry-run --json   # build, print, relay nothing
    fmonero wallet send <addr|@label> <amount> --yes --json       # build and relay
  --max-fee AMOUNT refuses the relay when the built total fee exceeds it. Amounts are a
  decimal XMR string (at most 12 decimals) or raw piconero with a `p` suffix. `all` is not an
  amount: `wallet sweep <addr>` moves everything, bounded by --below.
  Every monetary value appears twice in output: `*_pico` (integer) and `*_xmr` (string). Do
  not do arithmetic on the string.
  A crash between build and relay leaves a record that BLOCKS the next send. A normal send
  clears it itself; if a command exits 3 `pending_send`, a human decides.

REMOTE NODE (§2m)
  fmonero can point monero-wallet-rpc at somebody else's node instead of running one here.
  **fmonero itself never speaks to it**: its own HTTP client is loopback-plaintext-only and
  contains no TLS stack. Three settings keys, and nothing else is consulted anywhere:
    node.mode            local | remote (default local)
    node.remote          [http[s]://]host:port — the port is REQUIRED, no path, no credential
    node.remote_trusted  false by default. NEVER true for a public node
  Two more keys, both TUI-only readers:
    node.remote_probe_sec  how often the TUI probes THE CONFIGURED NODE while its Node screen
                           is showing. 60 by default; 0 = off; 1..9 refused. Nothing else
                           reads it: status, the widget and the doctor spawn nothing, ever.
    wallet.autostart       start the wallet daemon when `fmonero tui` launches. false.
  Two commands, both --yes, both --dry-run capable, both idempotent:
    fmonero node remote <host:port> --yes     # writes the keys, re-renders the units,
                                              # restarts monero-wallet-rpc if it was running
                                              # AND the rendered unit changed
    fmonero node local --yes                  # the way back
  And, since v5.5, the command that finds the address the first one needs:
    fmonero node list [--refresh] [--probe] [--limit N] [--all] [--json]
  In remote mode: `node up|down|restart|peers|bans|ban|unban|limit|log` exit 4 `remote_node`
  before any RPC or systemctl spawn; `node info` still exits 0 and reports the address, the
  trusted flag and the mode with its four measured sub-objects null; `status` reports
  node.layout "remote", node.reachable NULL (not false) and every measured node field null.
  A send's step-7 gate becomes ONE bounded `refresh` that must succeed.
  WHAT THE NODE SEES AND CAN DO, because none of it is hypothetical:
    * your IP address and the timing of every request, including the burst around a send;
    * which blocks the wallet asks for, which bounds the wallet's age;
    * with trusted, the amounts the wallet holds — they travel INSIDE the request;
    * the decoys: it supplies the output distribution and the ring members every transaction
      is built from, so a hostile node can degrade the ring signature. trusted:false does not
      reduce this.
  It can lie about the chain tip (a send built on a stale tip is rejected on relay: exit 16,
  funds unaffected), about whether a transaction was relayed (the pending record covers it),
  and about the FEE — the one lie that costs real money, because the wallet takes the
  fee-per-byte verbatim and the transaction relays successfully. Bound every send with
  --max-fee. In remote mode `wallet create` also takes the new wallet's restore height from
  that node, before fmonero's own set_daemon runs: create while it is answering.
  TLS here is --daemon-ssl autodetect: OPPORTUNISTIC AND UNAUTHENTICATED. It tries TLS, falls
  back to plaintext, and does not verify the certificate. Tor is not in v0.1.

PUBLIC NODE DISCOVERY (§2n, v5.5)
  `fmonero node list` serves cache_dir/public-nodes.json and fetches monero.fail's nodes.json
  and health.json only when --refresh is given or the cache is absent or older than 24 h;
  --probe then asks the top --limit candidates (20 by default) for get_info from THIS machine,
  with at most 8 probes in flight and a 5 s deadline each, cancellable. Payload keys: nodes[], source, fetched_at,
  cache_age_sec, skipped_no_port, skipped_invalid, probed, total, refreshed.
  WHAT IT COSTS, and it is the same four sentences README carries:
    * fetching the list tells MONERO.FAIL this machine's IP address and the time;
    * probing tells EACH PROBED NODE the same;
    * NOTHING is contacted automatically except the node already configured, and that only
      while the TUI's Node screen is showing (node.remote_probe_sec);
    * monero.fail's available/checks/last_height are MONERO.FAIL'S MEASUREMENTS FROM ITS OWN
      VANTAGE POINT — not a recommendation and not this machine's. A node rated healthy there
      may be unreachable from here, which is why --probe exists and why its numbers are printed
      in their own columns and never averaged into a score.
  In human mode `node list` prints that disclosure on stderr whenever the invocation actually
  fetches or probes. --json is silent: the envelope carries `refreshed` and `probed`.
  The fetch and the probes are `curl` CHILD PROCESSES through the one deadline helper — argv
  only, three shapes, carrying no credential, password, wallet name, address, amount or txid,
  because /proc/<pid>/cmdline is world-readable. fmonero's own client is unchanged: loopback
  http only, no TLS stack, no port bound. No curl on PATH is exit 4 curl_missing, raised before
  any network call. If monero.fail is down or changes shape, `node list` reports it and nothing
  else in fmonero is affected: the cache is served stale with its age.
  One honest discrepancy: curl verifies TLS certificates and --daemon-ssl autodetect does not,
  so an https node with a bad certificate probes reachable:false reason:tls while the wallet
  would have reached it by falling back to plaintext. Neither figure is adjusted to match.

TIMEOUTS AND HANGS
  --timeout SECONDS lowers every deadline and never raises one. `node up --wait` is bounded by
  --wait-timeout (default 1800; 0 means until interrupted and is refused without a TTY).
  `node log --follow` is refused in the CLI, where a pipe would hang.

WALLETS WITHOUT A TERMINAL
  FMONERO_WALLET_PASSWORD=… fmonero wallet open <name>    # or --password-file <0600 file>
  The password never reaches argv, and no seed, spend key, view key or transaction key ever
  appears in output, in state or in a log line.

WHAT THIS BUILD HAS BEEN SEEN TO DO
  `fmonero version --json` carries build_stage and build_note. Read it: the node and wallet
  paths are verified against a fixture-serving mock, not against a real daemon, and the note
  says exactly which rows those are.
