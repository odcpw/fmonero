"""The confirm modal owns the keys wherever it is drawn (deleted after use).

`Msg::ActionDone(Built)` can arrive while the user has moved to another screen — the build is
not cancelled by leaving Send — and the modal is drawn on top of whatever is there. Without
this the modal would be visible while `y`/`send`/`Esc` went to the screen behind it: a confirm
modal that ignores its own keys is the worst possible state for the money surface.
"""
s = open("mod.rs").read()
old = """        // The Send screen owns every key while its form, its picker or its confirm modal is
        // focused — otherwise an address typed into the To field would switch screens.
        if self.screen == Screen::Send && self.send.owns_keys() {
            return self.on_send_key(k);
        }"""
new = """        // The Send screen owns every key while its form, its picker or its confirm modal is
        // focused — otherwise an address typed into the To field would switch screens. The
        // confirm modal owns them wherever it is drawn: a build finishes even if the user
        // moved to another screen, and a visible modal that ignored `send`/`Esc` would be the
        // worst state the money surface could be in.
        if self.send.confirming() || (self.screen == Screen::Send && self.send.owns_keys()) {
            return self.on_send_key(k);
        }"""
assert old in s
s = s.replace(old, new, 1)
open("mod.rs", "w").write(s)

s = open("tests.rs").read()
old = """#[test]
fn the_open_wallet_modal_masks_the_password_and_lists_the_wallet_dir() {"""
new = """/// A build survives leaving the Send screen, so the modal it produces must own the keys
/// wherever it lands — including over the Wallet screen.
#[test]
fn the_confirm_modal_owns_the_keys_even_on_another_screen() {
    let w = World::new("modal-keys");
    let mut sim = ProgramSimulator::new(model(&w.ctx, Screen::Send));
    sim.init();
    type_str(&mut sim, DEST);
    sim.inject_event(key(KeyCode::Tab));
    type_str(&mut sim, "0.1");
    sim.inject_event(key(KeyCode::Enter));
    // While the build runs the form holds no keys, so the user can walk away.
    sim.inject_event(key(KeyCode::Char('2')));
    assert_eq!(sim.model().screen, Screen::Wallet);
    run_action(&mut sim);
    assert!(matches!(sim.model().send.stage, Stage::Built(_)));
    let shown = sim_text(&sim, 110, 34);
    assert!(shown.contains("Type \\"send\\""), "{shown}");
    // `2` would switch screens on the Wallet screen; here it is typed into the modal.
    sim.inject_event(key(KeyCode::Char('2')));
    assert_eq!(sim.model().send.typed, "2", "the modal must eat the key");
    sim.inject_event(key(KeyCode::Escape));
    assert!(matches!(sim.model().send.stage, Stage::Form), "Esc discards");
    assert_eq!(w.count("relay_tx"), 0, "nothing may be broadcast");
}

#[test]
fn the_open_wallet_modal_masks_the_password_and_lists_the_wallet_dir() {"""
assert old in s
s = s.replace(old, new, 1)
open("tests.rs", "w").write(s)
print("done")
