"""Don't retry a failing wallet read once a second (deleted after use)."""
s = open("mod.rs").read()
old = """                        if open && self.wallet.addresses.is_none() {
                            return Cmd::batch(vec![self.load_wallet(), self.load_history()]);
                        }"""
new = """                        // Load once when a wallet appears, and **not** again every second
                        // after a failure: a 1 Hz retry of four RPCs against a wallet that is
                        // answering errors is the pile-up §2d exists to prevent. `r` retries,
                        // and the pane says so.
                        if open && self.wallet.addresses.is_none() && self.wallet.err.is_none() {
                            return Cmd::batch(vec![self.load_wallet(), self.load_history()]);
                        }"""
assert old in s
s = s.replace(old, new, 1)
# `r` on the Wallet screen clears the error so the load is attempted again.
old = """                    Screen::Wallet => {
                        cmds.push(self.load_wallet());
                        cmds.push(self.load_history());
                    }
                    Screen::Log => cmds.push(self.load_log()),"""
new = """                    Screen::Wallet => {
                        self.wallet.err = None;
                        self.wallet.rows_err = None;
                        cmds.push(self.load_wallet());
                        cmds.push(self.load_history());
                    }
                    Screen::Log => cmds.push(self.load_log()),"""
assert old in s
s = s.replace(old, new, 1)
open("mod.rs", "w").write(s)

s = open("screens.rs").read()
old = """            let text = m.wallet.err.clone().unwrap_or_else(|| "loading…".into());
            para(text, th.dimmed(), true).render(inner, frame);"""
new = """            let (text, style) = match &m.wallet.err {
                Some(e) => (format!("{e}\\nr retries"), th.err()),
                None => ("loading…".to_string(), th.dimmed()),
            };
            para(text, style, true).render(inner, frame);"""
assert old in s
s = s.replace(old, new, 1)
open("screens.rs", "w").write(s)
print("done")
