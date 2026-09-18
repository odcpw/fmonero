# fmonero: the research, plan and build session (2026-09-16/18)

Rendered from the raw transcript `4901fef8-2906-4bc0-b226-799715c2e661.jsonl` by `docs/sessions/render.py`. Only the conversation is kept; tool output, reasoning and background-task notifications are omitted. Times are local.

---

### Oliver · 2026-09-16 10:18

can we make a new project under odcpw and odcpw github to have a faster replacement for monero gui? it's a front end to monerod right? can we have something made with the frankensuite? 
can you use your skills to research all this and make a statement for what's possible?

### Claude · 2026-09-16 10:18

I'll research three things in parallel: what Monero GUI actually is and how it talks to monerod, what the FrankenSuite stack offers for a replacement, and what already exists on this machine under odcpw. Then I'll write up a statement of what's possible.

### Claude · 2026-09-16 10:22

Two research agents are running in the background: one on Monero GUI's architecture and the Rust Monero ecosystem, one on the FrankenSuite inventory and your fproton/fbittorrent house pattern. I've confirmed the local facts already: Arch ships monerod and monero-wallet-rpc in the `monero` package, monero-gui 0.18.5.2 is in extra, Feather is in the AUR, you have 238 GB free and 30 GB RAM for a pruned node, and Omarchy 4.0.3 has your two existing widget plugins. Nothing else is left that doesn't depend on the agents' reports, so I'll wait for them.

_tools: Skill×2, Bash×12, Agent×2_

### Claude · 2026-09-16 10:35

The Monero research is in. The key finding: monero-gui is 27k lines of QML over an in-process wallet2 library, and it spawns and babysits monerod itself, so its slowness is mostly Qt scene-graph rendering, list-model rebuilds, and UI-thread blocking rather than anything a replacement must inherit. Waiting on the FrankenSuite inventory agent before I write the statement.

### Claude · 2026-09-16 10:40

Both reports are in. Writing the statement now.

### Claude · 2026-09-16 10:43

Yes, this is feasible, and it is the same shape of project as fproton and fbittorrent. The full statement is in [fmonero-feasibility.md](fmonero-feasibility.md).

**What monero-gui actually is.** It is not just a front end to monerod. It is a Qt5/QML app with 27k lines of QML that links the wallet engine (wallet2) in-process and also spawns and babysits monerod as a child, polling readiness by shelling out to `monerod sync_info`. Its slowness comes from the QML scene graph, list-model rebuilds on large wallets, and wallet calls blocking the UI thread. Upstream closed a 2025 rewrite request as not actionable.

**What a FrankenSuite replacement can be.** A Rust CLI plus FrankenTUI app in your house pattern removes every GUI-specific bottleneck: millisecond startup, zero idle CPU, virtualized history, RPC off the UI thread. It cannot make wallet scanning faster, because that is wallet2's speed and Feather, Cake and everyone else inherit it too.

**Recommended architecture.**
- **Node:** JSON-RPC to a local unrestricted monerod. Remote nodes hide sync info, peers and bans, so the design is local-first.
- **Wallet:** drive `monero-wallet-rpc` as a systemd user unit, the way fbittorrent runs qbittorrent-nox. That keeps Monero's own engine, so the coming FCMP++ fork costs nothing. The pure-Rust wallet crate cannot spend under FCMP++ and would leave us owning persistence and reorgs.
- **FrankenSuite usage:** FrankenTUI 0.7 is the load-bearing piece and has every widget needed. Skip asupersync (nightly by default, no Digest auth, and ftui's loop is blocking anyway) and fsqlite (nightly-only on x86_64, which fproton already rejected on Sept 14). Use ureq, threads, and postcard snapshots as before.

**Gotchas found.** monero-wallet-rpc has no default port and requires `--rpc-bind-port`. Its login is MD5 Digest, which no Rust client in your repos speaks, so bind loopback and disable login. Arch's `monero` package ships a system unit running as user `monero` with `/etc/monerod.conf`, and the doctor has to detect that layout. FCMP++ is not activated on mainnet yet despite several articles claiming otherwise.

**Machine check.** Nothing Monero-related is installed. You have 238 GB free and 30 GB RAM, enough for a pruned node.

I did not create the repo, since that is outward-facing. Next step would be creating `~/Projects/odcpw/fmonero` and a private `odcpw/fmonero`, then writing DESIGN.md v1 from sections 5 and 6 of the statement in fproton's layout. I can also publish the statement as a shareable page if you want a link.

_tools: Bash×2, SendUserFile, Write_

### Oliver · 2026-09-16 10:52

so in plain english?

### Claude · 2026-09-16 10:52

Yes, we can build it, and it will be much snappier than the official app.

The official Monero GUI is slow because of how it draws its screens, not because of Monero itself. It also runs the node inside the same app and babysits it, which makes the whole thing feel sluggish when the node is busy. The wallet engine underneath is fine, and every other Monero wallet uses that same engine.

So the plan is: keep Monero's own node and wallet programs running in the background, and build our own fast terminal app on top of them, exactly like fproton sits on Proton's CLI and fbittorrent sits on qBittorrent. We show the node's sync progress, peers and log, and the wallet's balance, history, receive addresses and a send form with a confirmation step. Plus a bar widget and a doctor, same as your other two tools.

FrankenTUI is the part of the FrankenSuite we use. The other pieces (the async runtime and the database) need a nightly compiler and don't fit, so we skip them, which is what fproton already decided.

What will not get faster: the first-time wallet scan and the node's initial download. Those are Monero's speed, and the same for every wallet.

One thing to know up front: Monero has a big protocol upgrade coming within about a year. Because we use Monero's own wallet program rather than reimplementing it, we get that upgrade for free.

If you say go, I create the `fmonero` folder and GitHub repo and write the design document.

### Oliver · 2026-09-16 10:53

Go for it. use jeff's skills to plan, review, and implement. use workflow with opus agents.

### Claude · 2026-09-16 10:57

Creating the repo and the seed files now, cloning the Monero source for the planners to read, and moving the session into the project.

_tools: mcp__ccd_session__mark_chapter, Bash×2, Skill×2_

### Claude · 2026-09-16 19:34

The planning workflow is running in the background (up to 13 Opus agents: draft, three review lenses per round, integrator, until steady state). The repo, initial commit and push are done, and the session now lives in the project folder. I'll pick up as soon as the plan lands, sanity-check DESIGN.md myself, then launch the implementation workflow.

_tools: Bash×3, ToolSearch, mcp__ccd_directory__change_directory, Workflow_

### Claude · 2026-09-16 22:18

The planning workflow finished after three rounds, but the last round still flagged structural changes. Let me look at what rounds 2 and 3 found and at the current DESIGN.md before deciding whether to run another round.

### Claude · 2026-09-16 22:24

The implementation workflow script is ready and syntax-checked in the repo's temp folder. The only thing left before launching it is the fourth plan review round, which is still running; when its notification arrives I'll commit the revised DESIGN.md, correct the Monero source path in §1.6, and start implementation.

_tools: Bash×15, Workflow×2, Read, Write_

### Claude · 2026-09-16 22:52

Round 4 converged from 45 to 25 revisions and left §8's phase structure intact, so it's time to build. Committing the v5 plan with the corrected Monero source path, then launching implementation.

_tools: Bash_

### Claude · 2026-09-16 22:55

Implementation is underway in the background: the API-freeze agent is working on `main` and the fixtures-and-mock agent in its own worktree, with core, docs, TUI, doctor, plugin and integration queued behind them. The Phase 4 review script is written and checked. Nothing else can proceed until integration lands, so I'm waiting for that notification; I'll then inspect the build myself and launch the review.

_tools: Bash×4, Workflow, Write_

### Claude · 2026-09-17 04:06

The implementation workflow finished after about five hours. Let me read each phase's report and then verify the integrated build myself before starting the review.

### Claude · 2026-09-17 04:08

Integration is verified on my side: release build and 210 tests green, `fmonero status --json` answers correctly with both daemons absent, a send with no wallet daemon refuses with exit 4 and a runnable hint, the fast e2e suite passes 301 checks, and the bar widget is loaded in your live Omarchy shell. Main is pushed to GitHub. The Phase 4 workflow (four reviewers, a fixer, a verifier that installs twice) is running; I'll report when it finishes.

_tools: Bash×8, Workflow_

### Claude · 2026-09-17 06:41

The review workflow is done: four lenses produced 45 findings, then the fixer and verifier ran. Let me read their outcomes and the final state of the repo.

### Claude · 2026-09-17 06:44

Phase 4 results are pushed. The review found no blockers; 42 of 45 findings are fixed, and the real install ran twice on your machine (units written but disabled, plugin enabled, menu merged, binary in `~/.local/bin`). The verifier surfaced three things worth closing before I call it done: `node up` without Monero installed waited 30 s and left the unit flapping instead of refusing at once, two setup actions re-ran on every install, and the test suite assumed a never-set-up machine. A fix-up workflow (one Opus fixer, one independent Opus verifier) is running to close those plus the two deferred items; I'll report when it lands.

_tools: Bash×6, Workflow_

### Claude · 2026-09-17 08:04

The fix-up is in. Let me read the independent verifier's result and do my own final checks before reporting.

### Claude · 2026-09-17 08:06

One Opus agent is closing the last three hint defects with tests; everything else is green and pushed (main at the independent verifier's commit). I'll push again and write up the final report when that lands.

_tools: Bash×3, Agent_

### Claude · 2026-09-17 08:29

The last three defects are closed with tests (226 unit tests, 415 e2e assertions). Doing the final pass: reinstall the binary from HEAD, confirm the units are untouched, push, and update memory.

### Claude · 2026-09-17 08:29

fmonero v0.1 is built, reviewed, fixed, installed on your machine, and pushed to [github.com/odcpw/fmonero](https://github.com/odcpw/fmonero). The one thing it has never done is talk to a real Monero daemon, because none is installed here and I cannot install one without your password.

**What was built.** A Rust workspace of about 30k lines: the engine crate, the CLI and TUI binary, and a test-only mock daemon. Plan: [DESIGN.md](DESIGN.md) v5.1 after four Opus review rounds. Evidence log: [DECISIONS.md](DECISIONS.md) with every gate transcript. The house pattern is fproton's: `--json` on every command, typed exit codes with runnable hints, a doctor with undo, a FrankenTUI app with Node, Wallet, Send and Log screens, and the `oliver.monero` bar widget, which is already loaded in your Omarchy bar.

**Money safety as implemented.** A send builds the transaction with `do_not_relay`, shows the real fee, and relays exactly that built transaction only after `--yes`. The pending record is fsynced before the first relay and a file lock stops two sends at once. The wallet password never touches argv, logs or JSON. Both daemons are loopback only, with MD5 Digest auth to the wallet daemon so a web page cannot hit it.

**What is verified on this machine right now.**
- Build, clippy and fmt clean; 226 unit tests; 415 end-to-end assertions; plugin and TUI smoke tests. The same suite passes under an empty HOME and leaves nothing running.
- `scripts/install.sh --yes` ran for real and is idempotent: the two systemd user units are written but disabled, your monerod config is at 0600, the widget is enabled, the menu entries are merged.
- `fmonero node up` with Monero absent refuses in 2 ms with the hint `omarchy pkg add monero`, and leaves no unit flapping. That was a real bug the verifier caught and the fix-up closed.

**What is still unverified (RED by design).** Sync, peers, balance, opening a wallet file, and any real send or sweep. The README, `--help` and `robot-docs` say so explicitly.

**What you do next.** Install Monero, then run the stagenet rehearsal so the first real send is with test coins:

```bash
omarchy pkg add monero
```

After that, the full ordered command list (stagenet setup, wallet create, a `--dry-run` send, a real send run twice to prove the pending record clears, then back to mainnet) is in DECISIONS.md under "Fix-up verification". Tell me when Monero is installed and I'll run it with you.

**Known deviations, recorded not hidden.** The TUI is 4,557 lines against the plan's original 1,800 budget, so the budget was raised. The full e2e run takes two minutes, not the planned 90 seconds, because it deliberately waits out deadline and breaker paths. The uninstaller has only been syntax-checked.

_tools: Bash×2_

### Oliver · 2026-09-17 17:37

ok installed monero. How are wallet managed? I?ld like to import one from monero gui later

### Claude · 2026-09-17 17:37

Let me check what the code actually does with wallet files, and confirm the doctor sees the new install.

### Claude · 2026-09-17 17:38

Monero 0.18.5.1 is on the machine and the doctor now passes 26 checks. The only failure is `node_reachable`, because the node hasn't been started yet.

**How wallets are managed.** fmonero never touches wallet files itself. Monero's own `monero-wallet-rpc` does, running as your user unit, from one directory per network:

```
~/.local/share/fmonero/wallets/mainnet/
```

A wallet is the standard Monero pair of files, `NAME.keys` (the encrypted keys) and `NAME` (the scan cache). The commands are `fmonero wallet list` (lists the `.keys` names in that folder), `wallet create NAME --yes`, `wallet open NAME` (prompts for the password without echo), and `wallet close`. One wallet is open at a time. Seed restore was deliberately left out so no seed ever passes through this code; the empty-list output names the `monero-wallet-cli` command that does it.

**Importing a monero-gui wallet later.** The GUI keeps each wallet in its own subfolder, typically `~/Monero/wallets/NAME/NAME` plus `NAME.keys`. Two ways in:

- **Copy the files.** Close the wallet in the GUI first, then copy `NAME.keys` (and `NAME`, if you want to keep the scan cache) into the mainnet folder above. Open it with `fmonero wallet open NAME` and the same password. The keys file carries the wallet's refresh height, so even a keys-only copy scans from where the wallet was created, not from block zero.
- **Restore from seed** into that folder with `monero-wallet-cli --restore-deterministic-wallet`, giving a restore height to keep the scan short.

Never let two programs open the same wallet file at once. Copying is the right move; pointing both the GUI and fmonero at one file is not.

**Next steps.** Nothing needs to change for a mainnet import. Starting the node is a multi-day pruned sync of roughly 100 GB or more, and the safest first send is the stagenet rehearsal listed in DECISIONS.md. Say the word and I'll start the node with `fmonero node up --wait`, or run the stagenet rehearsal first.

_tools: Bash×2_

### Oliver · 2026-09-17 17:38

i would connect to a public node

### Claude · 2026-09-17 17:39

Good question, because the tool was built local-node-first. Let me check exactly what the code does today when the wallet daemon is pointed at a remote node, before telling you what works and what needs a change.

### Claude · 2026-09-17 17:43

The remote-node workflow is running (design amendment, a money-and-Monero review, implementation, then a live verification against a real public node from a sandbox with an empty throwaway wallet). Until it lands, the honest answer to your question is: today the tool would fight you on a public node, and by the time this finishes, `fmonero node remote <address> --yes` will be the supported path with the local node never needed. I'll report back with the node the verifier used and the exact commands.

_tools: Bash×4, Workflow_

### Oliver · 2026-09-17 19:39

can you pu tthis sesson inthe repo's local folder so I can read via remote controle?

### Claude · 2026-09-17 19:41

Done. The session is now readable from the repo, and the session itself is easier to find remotely.

- **Log in the repo:** [docs/sessions/2026-09-16-fmonero-build.md](docs/sessions/2026-09-16-fmonero-build.md), pushed to GitHub. It holds the conversation only: your six prompts and my replies, with a one-line note of which tools each reply used. Tool output, reasoning and background notifications are left out, and I swept it for secrets before committing.
- **Refreshable:** [docs/sessions/render.py](docs/sessions/render.py) regenerates it from the raw transcript, so I'll re-render it when this session ends to capture the remote-node work.
- **Session renamed** to "fmonero: research, plan, build" so it stands out in the Remote Control list. The session's own storage stays tied to the scratch workspace it started in; that can't be moved, which is why the log lives in the repo instead.

The remote-node workflow is still running in the background; I'll report when it finishes.

_tools: Bash×4, ToolSearch, mcp__ccd_session_mgmt__get_session, mcp__ccd_session_mgmt__set_session_title_

### Claude · 2026-09-17 20:52

The remote-node workflow finished. Let me read the implementation and live-verification results, then check the tree myself.

### Claude · 2026-09-17 20:54

Only Cake Wallet's node answers from here right now (synced, height 3.76 M, plain HTTP); the five other well-known public nodes did not respond within 8 s. That matches what the verifier saw, so it is the node I'll propose unless you have one you trust more. The fix agent is still working on the three defects; I'll reinstall and hand you the switch commands when it's done.

_tools: Bash×4, Agent_

### Claude · 2026-09-17 22:00

All three defects are closed and every gate is green, including the full e2e run. Finishing up: reinstalling the binary from HEAD, switching your installation to the Cake Wallet node (reversible with one command, and nothing connects until you open a wallet), refreshing the notes and the session log, and pushing.

### Claude · 2026-09-17 22:01

Your installation now runs in remote node mode against `xmr-node.cakewallet.com:18081`, and the doctor passes with no failures. Nothing has connected yet: the wallet daemon starts only when you open or create a wallet.

**What changed.** Remote node mode is now a first-class setting. The wallet daemon is pointed at the public node as untrusted, fmonero itself never talks to it, and the send flow gates on a wallet refresh instead of a local node. Node commands that need a local daemon refuse with a hint. Everything was verified live from a sandbox against that node with a throwaway empty wallet: create, open, refresh, balance, address, history, and a dry-run send that correctly stopped at "insufficient funds". Three defects that surfaced in that verification are fixed, including one where `doctor --fix` would have flipped you back to local mode. Full details are in [DECISIONS.md](DECISIONS.md) and the [session log](docs/sessions/2026-09-16-fmonero-build.md), all pushed.

**Why that node.** It was the only well-known public node that answered from your machine; five others timed out. It is plain HTTP and restricted, which is normal for public nodes. Switching is one command:

```bash
fmonero node remote <host:port> --yes
```

and going back to a local node is `fmonero node local --yes`.

**Your first wallet.** Create one or copy one from monero-gui, then open it:

```bash
fmonero wallet create main --yes
```

Write the seed down before funding it; the command tells you how to display it with `monero-wallet-cli` offline. For a GUI wallet, copy `NAME.keys` (and `NAME` if you want the cache) into `~/.local/share/fmonero/wallets/mainnet/` and run `fmonero wallet open NAME`. Then `fmonero tui` or the bar widget.

**Caveats, plainly.** A public node learns your IP and request timing, can lie about the chain, and can overstate fees, so never mark one trusted. A real send of any amount has still never been run through fmonero; the first one should be small, and I'd suggest doing it together so I can read the transaction it builds before you confirm.

_tools: Bash×3_

### Oliver · 2026-09-18 14:51

So in a sense, it's a bit stupid because tab puts me into a tabby mode and then I can't go tab between node wallet send and log and I don't have a mouse I can use so the buttons need to be mouseable as well I'd say uh, then um, uh, on node I'd like you to fetch a list of public nodes so I can choose one to connect to it in the TUI and um, and then it needs a button somehow to run the daemon or at least to make the the daemon run automatically um somewhere maybe in the drop down so you can start the node start the daemon because here i can't start the node because there's no daemon or i can't i don't understand it maybe you can uh, figure it out and fix it all

### Claude · 2026-09-18 14:52

Let me look at the current state of the repo first, since another session has been working on it since yesterday, then map each of your points to what the TUI does today.

### Claude · 2026-09-18 15:24

Understood on all four, and I've turned them into one design-review-implement-verify round (Opus agents, a few hours). What's being built:

- **Tab always switches screens.** Node, Wallet, Send, Log, from anywhere, even inside the Send form or a focused filter. Fields inside the Send form move with Up and Down instead. The tab strip becomes clickable.
- **Mouse support.** Tabs, table rows, form fields and every modal button get click targets, plus on-screen buttons wherever there is a key hint today. Keyboard stays a full peer.
- **Public node picker on the Node screen.** The list comes from monero.fail (195 clear-net nodes plus its health data, which I checked today). It shows health, last height and, when you press `p`, a live probe with latency. Enter switches the wallet daemon to that node after a confirm. Probing is opt-in because every probed node sees your IP; only the node you already use is probed automatically.
- **The daemon confusion.** In remote mode there is no node to start; the only daemon is the wallet daemon, and it starts when you open a wallet. The Wallet screen gets a proper empty state with Open, Create and Start-daemon buttons, a create-wallet flow with the seed-backup notice, a `wallet.autostart` setting, and the widget drop-down gets "Open wallet…" and "Choose node…". The "wallet down" warning becomes an idle notice.

The verifier will exercise the whole thing on a real terminal, fetch the node list live, create and open a throwaway wallet against the Cake Wallet node, reinstall, and restart the bar. I'll report when it lands.

_tools: Bash×5, mcp__ccd_directory__change_directory, ListAgents, Workflow_

### Claude · 2026-09-18 19:23

The UX round finished after about four hours. Let me read the implementation and verification results and check the tree.

### Claude · 2026-09-18 19:24

The fix agent is working on the six findings; the current state (v5.5 with Tab, mouse, the node picker and the wallet flow) is pushed. I'll reinstall and report as soon as it lands.

_tools: Bash×3, Agent_

