.pragma library

// Pure formatters for the Monero widget. No I/O, no state: everything here takes the last
// `fmonero … --json` document and returns a string or a boolean.
//
// **No arithmetic on money happens in this file, or anywhere else in this plugin.** Status JSON
// carries every monetary value twice — an integer `*_pico` and a pre-rendered `*_xmr` string
// (DESIGN.md §5) — and the widget only ever displays the string. That is the QML half of
// DESIGN.md §2a.5 ("no f64 touches a monetary value anywhere, in Rust or in QML"), and
// tests/plugin-smoke.sh enforces it by refusing float parsing and scientific-notation division in
// every QML and JS file here. Byte rates and block heights are not money and are formatted
// normally; `int()` below is the only numeric coercion in the plugin and it is integer-only.

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

// Thin-space groups, so 3 541 122 reads as a height rather than a phone number.
function grouped(value) {
  var s = String(int(value, 0))
  var out = ""
  for (var i = 0; i < s.length; i++) {
    if (i > 0 && (s.length - i) % 3 === 0) out += " "
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

// The one phrase under the hero. `node` is Status.node, or null.
function syncLabel(status) {
  if (!status || status.ok !== true) return "fmonero is not answering"
  var n = status.node
  if (!n || n.reachable !== true) {
    return n && n.layout === "none" ? "No node configured" : "Node is not running"
  }
  if (n.synchronized === true) return "Synced"
  if (n.sync_percent === null || n.sync_percent === undefined) return "Syncing"
  return "Syncing " + Math.floor(n.sync_percent) + "%"
}

function heightLabel(node) {
  if (!node || node.height === null || node.height === undefined) return "Node"
  return "Node · " + grouped(node.height)
}

function peersLabel(node) {
  if (!node) return "—"
  var out = node.outgoing_connections_count, inc = node.incoming_connections_count
  if (out === null || out === undefined) return "not available on a restricted node"
  return "↑ " + int(out, 0) + " out · ↓ " + int(inc, 0) + " in"
}

// `status.pending` (DESIGN.md §5), rendered for the panel's pending row (§6.5).
// `showAmounts` is the §6.2 showBalance flag: with it off the row carries neither the amount nor
// the destination, because pending is the row most likely to be on screen while someone is
// looking over a shoulder.
function pendingLabel(pending, showAmounts) {
  if (!pending) return ""
  var head = "Send in flight"
  if (showAmounts === true) {
    head += ": " + String(pending.total_amount_xmr) + " XMR → " + shortAddr(pending.dest)
  }
  var relayed = int(pending.relayed_count, 0), total = int(pending.total_count, 0)
  if (total > 0 && relayed < total) head += " · " + relayed + " of " + total + " relayed"
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
