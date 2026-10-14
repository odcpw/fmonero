import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const code = fs.readFileSync(new URL('../plugin/oliver.monero/Model.js', import.meta.url), 'utf8')
  .replace(/^\.pragma library\s*/m, '');
const model = vm.createContext({});
vm.runInContext(code, model, { timeout: 1000 });
const hostile = '\x1b]52;c;clipboard\x07\x1b[31m<img src="https://example.invalid/tracker">\x00';
const safe = model.safeText(hostile);
assert(!safe.includes('\x1b') && !safe.includes('\x00') && !safe.includes('<'));
assert(safe.includes('‹img'));
assert(model.safeText('x'.repeat(10000)).length <= 4096);
assert.equal(model.safeText('René & friends'), 'René & friends');
const privateError = 'Private test-wallet /private/example payment detail';
assert(!model.actionLine(privateError, privateError, '', true, true).includes('test-wallet'));
assert(!model.actionLine('', privateError, '', true, false).includes('/private/'));
assert.equal(model.actionLine('Opening wallet…', '', '', true, false), 'Opening wallet…');
assert.equal(model.actionLine(privateError, privateError, '', false, true), privateError);
const publicSensitive = {
  total_amount_xmr: '1.234567891234', dest: 'sensitive-public-address',
  progress_words: 'may already have gone out',
};
const hidden = model.pendingLabel(publicSensitive, false);
assert(!hidden.includes(publicSensitive.dest) && !hidden.includes(publicSensitive.total_amount_xmr));
assert(hidden.includes('may already have gone out'));
const status = { ok: true, node: { layout: 'remote' }, wallet: { open: true,
  balance_xmr: '1.234567891234', name: 'private-wallet-name' } };
assert.equal(model.headerLine(status, false, false), 'Wallet open');
console.log('PASS: widget control, markup, bounds and private presentation checks');
