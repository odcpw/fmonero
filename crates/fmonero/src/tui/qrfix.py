s = open("screens.rs").read()
old = '''    let qr_rows = side.div_ceil(2);
    let text_rows = wrapped(address, 72).len() as u16;
    let Some(inner) = overlay(
        m,
        frame,
        side + 4,
        qr_rows + text_rows + 2,
        " Receive — Esc closes ",
    ) else {
        return;
    };
    let fits = inner.height > qr_rows && inner.width >= side;'''
new = '''    let qr_rows = side.div_ceil(2);
    // `overlay` spends a row and a column on its border and one more of each on its padding,
    // so the interior it hands back is (w - 4, h - 4): asking for exactly the code's own
    // height left the address nowhere to go and reported a 120x40 terminal as too small.
    let text_rows = wrapped(address, side as usize).len() as u16;
    let Some(inner) = overlay(
        m,
        frame,
        side + 4,
        qr_rows + text_rows + 4,
        " Receive — Esc closes ",
    ) else {
        return;
    };
    let fits = inner.height >= qr_rows && inner.width >= side;'''
assert old in s, "the qr_modal header changed again"
s = s.replace(old, new, 1)
open("screens.rs", "w").write(s)

s = open("tests.rs").read()
old = '''    let text = sim_text(&s, 120, 40);
    assert!(text.contains("Receive"), "{text}");
    // The address is wrapped, never elided (§7a).
    assert!(text.contains(&DEST[..40]) || text.contains(&DEST[..20]), "{text}");'''
new = '''    let text = sim_text(&s, 120, 40);
    assert!(text.contains("Receive"), "{text}");
    // The code itself, in half-block cells — not the "too small" fallback.
    assert!(
        !text.contains("too small for a QR"),
        "a 120x40 terminal has room for the code:\\n{text}"
    );
    assert!(
        text.contains('\\u{2580}') || text.contains('\\u{2584}') || text.contains('\\u{2588}'),
        "{text}"
    );
    // The address is wrapped, never elided (§7a).
    assert!(text.contains(&DEST[..20]), "{text}");'''
assert old in s, "the qr test changed"
s = s.replace(old, new, 1)
open("tests.rs", "w").write(s)
print("done")
