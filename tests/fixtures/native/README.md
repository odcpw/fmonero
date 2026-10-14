# Native submission receipt fixture

`submit-monero-0.18.5.3.stdout` reproduces the exact English receipt format emitted
by Monero 0.18.5.3 with stdout piped and `LC_ALL=C`. Its two hashes are synthetic
32-byte public identifiers, not receipts from funded transactions or evidence of
live submission. The wording, angle delimiters and line boundaries come from:

- `src/simplewallet/simplewallet.cpp`, `commit_or_save`, line 10759: after
  `commit_tx`, the success writer emits the submission sentence, a streamed
  `crypto::hash`, `ENDL`, then the `show_transfers` guidance.
- `src/crypto/hash.h`, lines 95–96: the hash stream operator calls
  `epee::to_hex::formatted` on its byte span.
- `contrib/epee/include/hex.h`, lines 73–74: formatted hex includes `<` and `>`.

Root supplied the matching source copies as `/tmp/fmonero-simplewallet.cpp`,
`/tmp/fmonero-oracle-hash.h` and `/tmp/fmonero-oracle-hex.h`. This is a source-derived
wire-format fixture. Root's isolated native harness provides separate runtime
evidence; this fixture makes no native, funded-payment or hardware claim.
