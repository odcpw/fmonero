.pragma library

// Pure formatters for the Monero widget. No I/O, no state: everything here takes the last
// `fmonero … --json` document and returns a string or a boolean.
//
// **No arithmetic on money happens in this file, or anywhere else in this plugin.** Status JSON
// carries every monetary value twice — an integer `*_pico` and a pre-rendered `*_xmr` string
// (DESIGN.md §5) — and the widget only ever displays the string: the QML half of §2a.5, which
// tests/plugin-smoke.sh enforces by refusing float parsing and `/ 1e` in every file here. Byte
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

function bytes(n) {
  var v = Math.max(0, int(n, 0))
  var u = ["B", "KiB", "MiB", "GiB", "TiB"]
  var i = 0
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++ }
  return (i === 0 ? v.toFixed(0) : v.toFixed(1)) + " " + u[i]
}

function rate(bps) {
  if (bps === null || bps === undefined) return "—"
  return bytes(bps) + "/s"
}

function age(sec) {
  if (sec === null || sec === undefined) return "—"
  var s = Math.max(0, int(sec, 0))
  var d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600)
  var m = Math.floor((s % 3600) / 60), r = s % 60
  if (d > 0) return d + "d " + h + "h"
  if (h > 0) return h + "h " + (m < 10 ? "0" : "") + m + "m"
  if (m > 0) return m + "m " + (r < 10 ? "0" : "") + r + "s"
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
function nodeLabel(node) {
  if (!node) return "—"
  if (node.layout === "remote") return "public node " + shortAddr(String(node.url || ""))
  return "your own node"
}

// §6.5: `node.remote_probe` as a block number **with its age in the same string** — the
// measurement came from an earlier invocation (§2n.4), so a four-hour-old `synced` says nothing
// about now. Renamed from `probeLabel` in v5.6; §2o.2 deletes that word.
function checkedLabel(node) {
  var p = node && node.remote_probe ? node.remote_probe : null
  if (!p) return "not checked yet"
  var when = " · checked " + age(int(p.age_sec, 0)) + " ago"
  if (p.reachable !== true) return "no answer" + when
  var out = ""
  if (p.height !== null && p.height !== undefined) out += "block " + grouped(p.height)
  if (p.synchronized === true) out += (out === "" ? "" : " · ") + "synced"
  else if (p.synchronized === false) out += (out === "" ? "" : " · ") + "syncing"
  return out + when
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

// Decision 7's headline while a wallet is behind. The scanning happens inside the wallet
// service, so it continues after the TUI's window closes — as fbittorrent's transfers do.
function walletSyncLabel(status) {
  var left = blocksLeft(status)
  if (left <= 0) return ""
  return "Syncing " + walletSyncPct(status) + "% · " + grouped(left) + " blocks left"
}

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
  if (walletBehind(status)) return "Wallet syncing " + walletSyncPct(status) + "%"
  if (w && w.open === true) {
    return showAmounts === true && w.balance_xmr
      ? "Wallet open · " + String(w.balance_xmr) + " XMR" : "Wallet open"
  }
  if (n && n.layout === "remote") {
    var p = n.remote_probe
    return p && p.reachable === true && p.synchronized === true
      ? "Public node · synced · wallet closed" : "Public node · wallet closed"
  }
  if (!n || n.layout === "none") return "No node set up yet"
  if (n.reachable !== true) return "Your node is not running"
  if (n.synchronized !== true) {
    var pct = n.sync_percent === null || n.sync_percent === undefined
      ? 0 : Math.floor(n.sync_percent)
    return "Your node syncing " + pct + "%"
  }
  return "Your node · synced · wallet closed"
}

function peersLabel(node) {
  if (!node) return "—"
  var out = node.outgoing_connections_count, inc = node.incoming_connections_count
  if (out === null || out === undefined) return "not shown by this node"
  return "↑ " + int(out, 0) + " out · ↓ " + int(inc, 0) + " in"
}

// `status.pending` (§5) for the panel's pending row (§6.5). `showAmounts` is §6.2's showBalance:
// with it off the row carries neither the amount nor the destination, because pending is the row
// most likely to be on screen while someone is looking over a shoulder.
function pendingLabel(pending, showAmounts) {
  if (!pending) return ""
  var head = "Sending"
  if (showAmounts === true) {
    head += " " + String(pending.total_amount_xmr) + " XMR to " + shortAddr(pending.dest)
  }
  var relayed = int(pending.relayed_count, 0), total = int(pending.total_count, 0)
  if (total > 0 && relayed < total) head += " · " + relayed + " of " + total + " sent"
  return head
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
