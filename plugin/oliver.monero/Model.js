.pragma library

// Pure formatters for the Monero widget. No I/O, no state: everything here takes the last
// `fmonero … --json` document and returns a string or a boolean.
//
// **No arithmetic on money happens in this file, or anywhere else in this plugin.** Status JSON
// carries every monetary value twice — an integer `*_pico` and a pre-rendered `*_xmr` string
// (DESIGN.md §5) — and the widget only ever displays the string: the QML half of §2a.5, which
// tests/plugin-smoke.sh enforces by refusing float parsing and scientific division here. Byte
// rates and block numbers are not money; `int()` is the only numeric coercion and it is integer.

function parseJson(raw) {
  try {
    var d = JSON.parse(String(raw))
    return d && typeof d === "object" ? d : null
  } catch (e) {
    return null
  }
}

// A Status document (DESIGN.md §5), or a marker the panel renders as "not answering".
// `ok === true` plus a `node` object is the gate for every rendered value (§6.3).
function parseStatus(raw) {
  var d = parseJson(raw)
  if (!d || d.ok !== true || !d.node || typeof d.node !== "object") return { ok: false }
  return d
}

function int(value, fallback) {
  var n = parseInt(String(value), 10)
  return isFinite(n) ? n : fallback
}

// Space-grouped, so 3 541 122 reads as a block number rather than a phone number. An ordinary
// space and not U+2009: the bar's font renders a thin space at zero width, so the grouping was
// invisible on screen when this was first drawn.
function grouped(value) {
  var s = String(int(value, 0))
  var out = ""
  for (var i = 0; i < s.length; i++) {
    if (i > 0 && (s.length - i) % 3 === 0) out += " "
    out += s.charAt(i)
  }
  return out
}

// A size, and the same divisor and the same labels as `fmonero_core::bytes` (X6). This divided by
// **1024** and labelled the result `KiB`, so one waiting pool read `21.4 kB` in the CLI and
// `20.9 KiB` here. `tests/plugin-smoke.sh` holds the two halves to the same output, as it does for
// `grouped`. Integer arithmetic, with the one decimal place carried by hand, so the two cannot
// drift through a rounding mode either.
function bytes(n) {
  var v = Math.max(0, int(n, 0))
  var u = ["B", "kB", "MB", "GB", "TB"]
  var i = 0
  var scale = 1
  while (i + 1 < u.length && Math.floor(v / scale) >= 1000) { scale *= 1000; i++ }
  if (i === 0) return v + " B"
  var whole = Math.floor(v / scale)
  var tenths = Math.floor((v % scale) * 10 / scale)
  return whole + "." + tenths + " " + u[i]
}

function rate(bps) {
  if (bps === null || bps === undefined) return "—"
  return bytes(bps) + "/s"
}

// How far along something is, and the same rounding as `fmonero_core::percent` (X11). The hero
// floored and core's warning rounded, so one frame read `Your node syncing 33%` over `Your node is
// 34% synced.` Rounded down is the half that cannot lie: only a node that is actually there
// reads `100%`.
// Through `int()`, which truncates towards zero — and that IS the floor for a percentage. §2a.5's
// QML half (tests/plugin-smoke.sh) forbids the two float parsers outright, and a formatter that
// makes that grep fire is a formatter somebody later exempts, at which point the grep no longer
// protects the amounts it exists for.
function percent(p) {
  var v = int(p, 0)
  if (v < 0) v = 0
  if (v > 100) v = 100
  return v + "%"
}

// **The JavaScript half of `fmonero_core::duration`** (M2). Three formatters disagreed about one
// quantity: core's `age` had no days unit, so 239 000 seconds read `66h 23m` in the TUI and
// `2d 18h` here; `output.rs::human_uptime` had days and no `ago`; and this one padded only some of
// its fields, so `age(3600)` was `1h 00m` and `age(86400)` was `1d 0h`. Every field after the
// first is two digits, in all four branches, and `tests/plugin-smoke.sh` holds this function to
// the Rust one's own output.
function pad2(v) { return (v < 10 ? "0" : "") + v }
function age(sec) {
  if (sec === null || sec === undefined) return "—"
  var s = Math.max(0, int(sec, 0))
  var d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600)
  var m = Math.floor((s % 3600) / 60), r = s % 60
  if (d > 0) return d + "d " + pad2(h) + "h"
  if (h > 0) return h + "h " + pad2(m) + "m"
  if (m > 0) return m + "m " + pad2(r) + "s"
  return r + "s"
}

// "4AdUnd…9fL2Kq" — enough of both ends to recognise an address without showing one that could be
// mistaken for a full one. The panel copies the real value, never this.
function shortAddr(addr) {
  var a = String(addr || "")
  if (a.length <= 20) return a
  return a.substring(0, 8) + "…" + a.substring(a.length - 6)
}

// §6.5's node row, in one line (v5.6). `node` is Status.node.
//
// **A node address is never shortened** (X10). `shortAddr` turned `public.example.net:18089` into
// `public.e…:18089`, which names no node at all; the picker's own rule is address-first and
// never-elided (§7a.4), and the owner's own 20-character address escaped only by being short
// enough to fall under the threshold. A long one wraps in the panel, which is a row two lines
// tall rather than a row that lies.
function nodeLabel(node) {
  if (!node) return "—"
  if (node.layout === "remote") return "public node " + String(node.url || "")
  // **Not `your own node` when there is not one yet** (round 4 iteration 3): the hero said
  // `No node set up yet` while this row, in the same panel, said `your own node`. And something may
  // still be answering on that port with nothing set up, which is a different fact from "there is no
  // node" — the same two branches `output.rs`'s own first row draws.
  if (node.layout === "none") {
    return node.reachable === true ? "a node fmonero did not set up" : "no node set up yet"
  }
  return "your own node"
}

// §6.5: `node.remote_probe` as a block number **with its age in the same string** — the
// measurement came from an earlier invocation (§2n.4), so a four-hour-old `synced` says nothing
// about now. Renamed from `probeLabel` in v5.6; §2o.2 deletes that word.
function checkedLabel(node) {
  var p = node && node.remote_probe ? node.remote_probe : null
  if (!p) return "not checked yet"
  var when = "checked " + age(int(p.age_sec, 0)) + " ago"
  // **`reason_words` from the document** (round 4 iteration 3, §5): this had a `no answer` of its
  // own while the TUI's badge, its hero and its ` This node ` pane all rendered the specific
  // phrase — `nothing listening`, `too slow`, `no secure connection` — from one table in Rust.
  // One fact, one wording, and the widget cannot call that table.
  if (p.reachable !== true) return String(p.reason_words || "no answer") + " · " + when
  var out = ""
  if (p.height !== null && p.height !== undefined) out += "block " + grouped(p.height)
  if (p.synchronized === true) out += (out === "" ? "" : " · ") + "synced"
  else if (p.synchronized === false) out += (out === "" ? "" : " · ") + "syncing"
  // **M27: never a row that begins with a bare separator.** A restricted public node answers with
  // no height and no sync flag — which is the normal case — and this returned
  // ` · checked 1m 35s ago`, a row starting with a dot and nothing before it. The TUI says
  // `answered` for that state; so does this now.
  if (out === "") out = "answered"
  return out + " · " + when
}

// The wallet service's state as a person reads it (§2o.2). `up` and `idle` were the program's
// words: with a public node and nothing open the service is SUPPOSED to be down (§4.2), which
// is why §5 makes that an `info` rather than a warning.
function walletStateLabel(wallet, warnings) {
  if (wallet && wallet.unit_active === true) return "running"
  var idle = Array.isArray(warnings) && warnings.some(function (w) {
    return w && w.id === "wallet_idle"
  })
  return idle ? "starts when you open a wallet" : "not running"
}

// The node's own block number: the live `get_info` height with your own node, the **cached
// check's** with a public one (§5). `-1` when unknown, so nothing renders a 0 as "caught up".
function nodeHeight(status) {
  var n = status && status.node ? status.node : null
  if (!n) return -1
  if (n.height !== null && n.height !== undefined) return int(n.height, -1)
  var p = n.remote_probe
  if (p && p.reachable === true && p.height !== null && p.height !== undefined) return int(p.height, -1)
  return -1
}

function walletHeight(status) {
  var w = status && status.wallet ? status.wallet : null
  if (!w || w.height === null || w.height === undefined) return -1
  return int(w.height, -1)
}
function blocksLeft(status) {
  var n = nodeHeight(status), w = walletHeight(status)
  return n > 0 && w > 0 ? Math.max(0, n - w) : -1
}

function walletBehind(status) { return blocksLeft(status) > 0 }
function walletSyncPct(status) {
  var n = nodeHeight(status), w = walletHeight(status)
  return n > 0 && w > 0 ? Math.floor(w * 100 / n) : 0
}

// `walletSyncLabel` is **deleted** (round 4 iteration 3). It composed
// `Syncing 99% · 6 122 blocks left` for the hero's second line, under a header already reading
// `Wallet syncing 99%` — the same percent twice on a two-line hero, which is the defect §6.5 removes
// from every other row. The hero's second line is `grouped(blocksLeft) + " blocks left"`, built
// where it is used, and `headerLine` owns the percent.

// §6.4's header line: **at most five words**, one first-match table, the only string the hero
// renders. Ordered by what the user is waiting for — money moving, a wallet catching up, a
// balance, then the node — the reverse of v5.5, where the node came first because it came first
// in the JSON. One deviation from §6.4's written order, and it is a dead-row fix:
// `layout === "none"` is tested BEFORE `!nodeUp`, because a machine with no node configured also
// reports `reachable: false` and the row below could never match.
function headerLine(status, showAmounts, binaryMissing) {
  if (binaryMissing === true) return "fmonero not installed"
  if (!status || status.ok !== true) return "fmonero is not answering"
  var n = status.node, w = status.wallet
  if (status.pending) {
    return showAmounts === true
      ? "Sending " + String(status.pending.total_amount_xmr) + " XMR" : "Sending"
  }
  if (walletBehind(status)) return "Wallet syncing " + percent(walletSyncPct(status))
  if (w && w.open === true) {
    // **The SHORT field, never the exact one** (v5.6, §6.4, DECISIONS V16): a real balance is
    // `1.234567891234`, which makes this header 32 characters against a 380-pixel panel, and
    // §2a.5 forbids the QML from shortening it itself — that is arithmetic on a monetary value.
    // `balance_xmr_short` is rendered in Rust (four decimals, truncated, never rounded up) and
    // the panel's own Balance row still carries the exact figure.
    var short = w.balance_xmr_short || w.balance_xmr
    return showAmounts === true && short
      ? "Wallet open · " + String(short) + " XMR" : "Wallet open"
  }
  if (n && n.layout === "remote") {
    // **The verdict belongs on the hero too** (X13). With a public node that had been checked and
    // had not answered, this read `Public node · wallet closed` — the one fact the reader needed
    // was in one pane further down and nowhere else.
    var p = n.remote_probe
    if (!p) return "Public node · not checked yet"
    // **`reason_words`, the same phrase the Checked row and the TUI's badge use** (round 4
    // iteration 3): this said `no answer` above a row reading `nothing listening · checked 4s ago`.
    if (p.reachable !== true) return "Public node · " + String(p.reason_words || "no answer")
    if (p.synchronized === true) return "Public node · synced · wallet closed"
    if (p.synchronized === false) return "Public node · syncing"
    // **M27: a verdict, even when the node did not give one.** A restricted public node answers
    // with no sync flag, and this fell through to `Public node · wallet closed` — a header with no
    // verdict at all, where the TUI in the same state says `answered`.
    return "Public node · answered · wallet closed"
  }
  if (!n || n.layout === "none") return "No node set up yet"
  if (n.reachable !== true) return "Your node is not running"
  if (n.synchronized !== true) {
    return "Your node syncing " + percent(n.sync_percent)
  }
  return "Your node · synced · wallet closed"
}

// **One wording for peers, on all three surfaces** (X30). This read `↑ 16 out · ↓ 3 in`, the TUI's
// status bar `16 out · 3 in` and `status` `peers 16 out / 3 in` — three renderings of one fact, and
// the arrows added nothing the words `out` and `in` do not already say.
function peersLabel(node) {
  if (!node) return "—"
  var out = node.outgoing_connections_count, inc = node.incoming_connections_count
  if (out === null || out === undefined) {
    // **M28: a node that is off is not a node withholding an answer.** This returned `not shown by
    // this node` for the own-node-off document, where the truth is that there is nothing running to
    // ask — X23's shape, on the bar. And with a PUBLIC node it is neither: fmonero never asks one
    // anything (§2m.3), so the row says whose question it is, in the TUI's own words for the pane
    // that stands where this row would be.
    // **Not a sentence where a number goes** (round 4 iteration 3). With a public node fmonero
    // asks it nothing (§2m.3), so this row has no source at all — and `Peers: this is about your
    // own node` answers a question the label did not ask. `—` is the program's one glyph for a
    // value with no source (§5), which is what this is. The panel does not draw the row in that
    // mode anyway; this is the honest value if it ever does.
    if (node.layout === "remote") return "—"
    if (node.reachable !== true) return "none while your node is off"
    return "not shown by this node"
  }
  return int(out, 0) + " out · " + int(inc, 0) + " in"
}

// `status.pending` (§5) for the panel's pending row (§6.5). `showAmounts` is §6.2's showBalance:
// with it off the row carries neither the amount nor the destination, because pending is the row
// most likely to be on screen while someone is looking over a shoulder.
// **No `0 of 1 sent`** (round 4 iteration 3). That counter was `relayed_count` — fmonero's record
// of an acknowledgement it never got — printed directly above this panel's own warning saying the
// payment *may already have gone out*. As a count it asserts that none went out, and the obvious
// next move for whoever reads it is to send again. `pending.progress_words` is the one wording,
// rendered once in Rust and shared with the TUI's banner and the warning (§5, §6.5).
function pendingLabel(pending, showAmounts) {
  if (!pending) return ""
  var known = String(pending.progress_words || "may already have gone out")
  if (showAmounts !== true) return "A payment " + known
  return String(pending.total_amount_xmr) + " XMR to " + shortAddr(pending.dest) + " · " + known
}

// Highest severity among the Status warnings: "error", "warn", "info" or "".
function worstSeverity(warnings) {
  var worst = ""
  for (var i = 0; i < warnings.length; i++) {
    var s = String(warnings[i].severity || "")
    if (s === "error") return "error"
    if (s === "warn") worst = "warn"
    else if (s === "info" && worst === "") worst = "info"
  }
  return worst
}

// A command line typed into the widget's settings, split into argv. Every token must be inert: no
// spaces to re-split, no shell metacharacters, nothing that would matter if a launcher script
// passed it through `eval` (DESIGN.md §1.5).
function safeArgv(command) {
  var parts = String(command || "").trim().split(/\s+/)
  if (parts.length === 0 || parts[0] === "") return []
  for (var i = 0; i < parts.length; i++) {
    if (!/^[A-Za-z0-9._:@/=+-]+$/.test(parts[i])) return []
  }
  return parts
}
