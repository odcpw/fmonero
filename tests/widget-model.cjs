// Exercise the same pure functions Quickshell imports, including zero-height sync.
const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const path = require('node:path');
const model = vm.createContext({});
vm.runInContext(fs.readFileSync(path.join(__dirname, '../plugin/oliver.monero/Model.js'), 'utf8').replace(/^\.pragma library\s*/, ''), model);
const status = {ok: true, node: {layout: 'remote', remote_probe: {reachable: true, height: 100}}, wallet: {open: true, height: 0}};
assert.equal(model.blocksLeft(status), 100);
assert.equal(model.walletSyncPct(status), 0);
assert.equal(model.headerLine(status, false, false), 'Wallet syncing 0%');
status.wallet.height = 101;
assert.equal(model.walletSyncPct(status), 100);
status.wallet = {open: false, reachable: false, busy: true};
assert.equal(model.headerLine(status, false, false), 'Wallet busy · checking');
assert.equal(model.walletStateLabel(status.wallet, []), 'busy · waiting for the wallet');
console.log('widget-model: passed');
