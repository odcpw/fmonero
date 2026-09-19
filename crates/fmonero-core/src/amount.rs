//! Monero amounts as integers (DESIGN.md §2a.5, §5b).
//!
//! `1 XMR = 1 000 000 000 000` piconero, exactly (`src/cryptonote_config.h:66-67`), and
//! every amount in both RPCs is a `uint64_t` of piconero (§1b.3, §1b.4). No `f64` touches
//! a monetary value anywhere: parsing is integer arithmetic on the decimal digits, and
//! `to_xmr_string` is exact, so `Piconero::parse(x.to_xmr_string()) == x` for every `u64`.

use serde::{Deserialize, Serialize};

use crate::error::Error;

/// An amount in piconero.
///
/// `Default` is `Piconero(0)`, which the `#[serde(default)]` on every wire struct that
/// carries an amount relies on.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Debug, Serialize, Deserialize)]
pub struct Piconero(pub u64);

/// Decimal places in one XMR (`COIN = 1e12`).
const DECIMALS: usize = 12;
/// The minimum number of decimals `to_xmr_string` prints, so a rendered amount always
/// reads as money rather than as a bare integer.
const MIN_DECIMALS: usize = 4;

impl Piconero {
    /// `1 XMR` in piconero — `cryptonote_config.h:67`.
    pub const PER_XMR: u64 = 1_000_000_000_000;

    /// `"0.5"` | `"1.234567891234"` | `"250000000000p"`.
    ///
    /// Rejects more than 12 decimals, thousands separators, exponents, a bare `"."`, a
    /// leading `"+"` or `"-"`, surrounding whitespace, and anything that overflows `u64`
    /// — a value that does not fit in `u64` is not a value Monero could represent either.
    /// `"all"` is rejected by name: `wallet sweep` takes no amount argument, so there is
    /// no argument position for it to mean anything in (§4).
    pub fn parse(s: &str) -> Result<Piconero, Error> {
        if s.eq_ignore_ascii_case("all") {
            return Err(Error::usage(
                "\"all\" is not an amount. To send everything: fmonero wallet sweep <address> \
                 --dry-run",
            )
            .with_reason("bad_amount")
            .with_hint("fmonero wallet sweep <address> --dry-run   # to move everything"));
        }
        let bad = |why: &str| {
            Error::usage(format!("\"{s}\" is not an amount. {why}"))
                .with_reason("bad_amount")
                .with_hint("fmonero wallet send <address> 0.01 --dry-run   # XMR, or 10000000000p")
        };
        if s.is_empty() {
            return Err(bad("You typed nothing."));
        }
        // Raw piconero: an integer followed by `p`.
        if let Some(digits) = s.strip_suffix('p') {
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad("A p amount is a whole number of piconero."));
            }
            let value: u64 = digits.parse().map_err(|_| bad("It is too large."))?;
            return Ok(Piconero(value));
        }
        // Decimal XMR.
        let (int_part, frac_part) = match s.split_once('.') {
            Some((i, f)) => (i, f),
            None => (s, ""),
        };
        if int_part.is_empty() && frac_part.is_empty() {
            return Err(bad("A lone dot is not a number."));
        }
        if !int_part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad(
                "Use digits and at most one dot, like 0.5 for XMR or 250000000000p for piconero.",
            ));
        }
        if !frac_part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad(
                "Use digits and at most one dot, like 0.5 for XMR or 250000000000p for piconero.",
            ));
        }
        if frac_part.len() > DECIMALS {
            return Err(bad("Monero has 12 decimal places."));
        }
        let whole: u64 = if int_part.is_empty() {
            0
        } else {
            int_part.parse().map_err(|_| bad("It is too large."))?
        };
        let mut frac: u64 = 0;
        for i in 0..DECIMALS {
            let digit = frac_part
                .as_bytes()
                .get(i)
                .map(|b| u64::from(b - b'0'))
                .unwrap_or(0);
            frac = frac * 10 + digit;
        }
        let value = whole
            .checked_mul(Self::PER_XMR)
            .and_then(|v| v.checked_add(frac))
            .ok_or_else(|| bad("It is too large."))?;
        Ok(Piconero(value))
    }

    /// 12 decimals, trailing zeros trimmed to a minimum of four. **Exact, never rounded
    /// or truncated**, so the `*_xmr` string always agrees with the `*_pico` integer §5
    /// prints beside it: `1_234_567_891_234` → `"1.234567891234"`,
    /// `500_000_000_000` → `"0.5000"`, `0` → `"0.0000"`.
    pub fn to_xmr_string(self) -> String {
        self.to_xmr_min(MIN_DECIMALS)
    }

    /// The same string with a **wider floor**, for a column of small figures (M4).
    ///
    /// `to_xmr_string` trims to four decimals, which is right in a sentence and wrong down a
    /// column: a fee column printed `0.0000` beside `0.00003072`, and the node's four per-byte fee
    /// levels came out `cheapest 0.00000002 · normal 0.00000008 · fast 0.00000032 · fastest
    /// 0.000004` — the largest of the four rendered in the fewest digits, on the row a person uses
    /// to choose between them. A fee column asks for eight.
    ///
    /// **Still exact and still never truncated**: `min_decimals` is a floor on the trimming, not a
    /// precision. A figure that needs twelve digits prints twelve, and the column is then ragged
    /// rather than wrong — which is the right way round for money.
    pub fn to_xmr_min(self, min_decimals: usize) -> String {
        let whole = self.0 / Self::PER_XMR;
        let frac = self.0 % Self::PER_XMR;
        let mut digits = format!("{frac:0width$}", width = DECIMALS);
        while digits.len() > min_decimals.min(DECIMALS) && digits.ends_with('0') {
            digits.pop();
        }
        format!("{whole}.{digits}")
    }

    /// The floor a column of **fees** uses, so eight decimals is one number in one place (M4).
    pub const FEE_DECIMALS: usize = 8;

    /// **A glance figure: at most four decimals, and never more than you have** (v5.6, §6.4).
    ///
    /// `to_xmr_string` is exact, which makes a real balance thirteen characters of fraction —
    /// `1.234567891234` — and the bar widget's five-word header 32 characters long
    /// (DECISIONS V16). This is the short form for that one place, and it is **pre-rendered here
    /// rather than shortened in QML**, because §2a.5 forbids arithmetic on a monetary value in
    /// QML and cutting a decimal string is arithmetic.
    ///
    /// It **truncates** rather than rounds: `1.234567891234` is `1.2345`, not `1.2346`. Rounding
    /// up would put a number on the owner's bar that is larger than his balance, which is a
    /// small lie of exactly the kind this design refuses to tell about money — and the panel
    /// below the header carries the exact figure for anyone who wants it. A non-zero amount that
    /// truncates to nothing reads `< 0.0001` rather than `0.0000`, for the same reason.
    pub fn to_xmr_short(self) -> String {
        const SHORT_DECIMALS: u32 = 4;
        let per_step = 10u64.pow(DECIMALS as u32 - SHORT_DECIMALS);
        let steps = self.0 / per_step;
        if steps == 0 && self.0 > 0 {
            return "< 0.0001".to_string();
        }
        let scale = 10u64.pow(SHORT_DECIMALS);
        format!("{}.{:04}", steps / scale, steps % scale)
    }

    /// Addition that refuses to wrap — the only way money is summed (§5b `PendingStatus`).
    pub fn checked_add(self, other: Piconero) -> Result<Piconero, Error> {
        self.0
            .checked_add(other.0)
            .map(Piconero)
            .ok_or_else(|| Error::failure("amount overflow: the sum does not fit in 64 bits"))
    }
}

impl std::fmt::Display for Piconero {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_xmr_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worked_examples_of_the_design() {
        assert_eq!(
            Piconero(1_234_567_891_234).to_xmr_string(),
            "1.234567891234"
        );
        assert_eq!(Piconero(500_000_000_000).to_xmr_string(), "0.5000");
        assert_eq!(Piconero(0).to_xmr_string(), "0.0000");
        assert_eq!(Piconero::PER_XMR, 1_000_000_000_000);
    }

    #[test]
    fn decimal_and_piconero_forms_parse() {
        assert_eq!(Piconero::parse("0").unwrap(), Piconero(0));
        assert_eq!(Piconero::parse("1").unwrap(), Piconero(Piconero::PER_XMR));
        assert_eq!(Piconero::parse("0.5").unwrap(), Piconero(500_000_000_000));
        assert_eq!(
            Piconero::parse("1.234567891234").unwrap(),
            Piconero(1_234_567_891_234)
        );
        assert_eq!(
            Piconero::parse("250000000000p").unwrap(),
            Piconero(250_000_000_000)
        );
        assert_eq!(Piconero::parse(".5").unwrap(), Piconero(500_000_000_000));
        assert_eq!(Piconero::parse("1.").unwrap(), Piconero(Piconero::PER_XMR));
        assert_eq!(
            Piconero::parse("18446744.073709551615").unwrap(),
            Piconero(u64::MAX)
        );
    }

    #[test]
    fn everything_the_design_rejects_is_rejected_with_exit_two() {
        for bad in [
            "",
            ".",
            "+1",
            "-1",
            "1_000",
            "1,000",
            "1e3",
            "1.2345678912345", // 13 decimals
            "18446745",        // overflows u64 piconero
            "18446744.073709551616",
            "1.5p",
            "p",
            "abc",
            " 1",
            "1 ",
            "0x10",
        ] {
            let e = Piconero::parse(bad).unwrap_err();
            assert_eq!(e.exit_code(), 2, "'{bad}' must be a usage error");
            assert_eq!(e.reason(), Some("bad_amount"), "'{bad}'");
        }
    }

    /// V16: the header's short form. Four decimals, truncated, and a non-zero amount that would
    /// come out as `0.0000` says so instead.
    #[test]
    fn the_short_form_is_four_decimals_and_never_rounds_up() {
        for (pico, want) in [
            (1_234_567_891_234u64, "1.2345"),
            (1_234_600_000_000, "1.2346"),
            (500_000_000_000, "0.5000"),
            (0, "0.0000"),
            (1, "< 0.0001"),
            (99_999_999, "< 0.0001"),
            (100_000_000, "0.0001"),
            (18_400_000_000_000_000, "18400.0000"),
        ] {
            assert_eq!(Piconero(pico).to_xmr_short(), want, "{pico}");
        }
        // Never longer than `1.2345`-plus-the-whole-part, which is the whole point: the header is
        // five words wide (§6.4) and the exact figure is one row below it in the panel.
        let exact = Piconero(1_234_567_891_234).to_xmr_string();
        assert_eq!(exact, "1.234567891234");
        assert!(Piconero(1_234_567_891_234).to_xmr_short().len() < exact.len());
    }

    #[test]
    fn all_is_refused_by_name_and_points_at_sweep() {
        for spelling in ["all", "ALL", "All"] {
            let e = Piconero::parse(spelling).unwrap_err();
            assert_eq!(e.exit_code(), 2);
            assert!(e.hint().contains("wallet sweep"), "{}", e.hint());
        }
    }

    /// The round trip §5b makes a gate assertion: a truncating formatter would make the
    /// `*_xmr` string disagree with the `*_pico` integer beside it.
    #[test]
    fn to_xmr_string_round_trips_for_every_sampled_u64() {
        let mut samples: Vec<u64> = vec![
            0,
            1,
            9,
            10,
            999,
            1_000,
            999_999_999_999,
            1_000_000_000_000,
            1_000_000_000_001,
            1_234_567_891_234,
            500_000_000_000,
            100_000_000_000_000,
            u64::MAX,
            u64::MAX - 1,
        ];
        // A deterministic spread across the whole range (xorshift, not for cryptography).
        let mut x: u64 = 0x2545_F491_4F6C_DD1D;
        for _ in 0..2000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            samples.push(x);
            samples.push(x % 1_000_000_000_000);
            samples.push(x % 1_000_000);
        }
        for v in samples {
            let p = Piconero(v);
            let text = p.to_xmr_string();
            assert_eq!(
                Piconero::parse(&text).unwrap(),
                p,
                "round trip failed for {v} ({text})"
            );
            assert!(text.contains('.'), "{text}");
            let decimals = text.split_once('.').unwrap().1.len();
            assert!(
                (MIN_DECIMALS..=DECIMALS).contains(&decimals),
                "{text} has {decimals} decimals"
            );
        }
    }

    /// M4: a column floor widens the trimming and changes nothing else — every value still
    /// round trips, and a figure that needs more digits than the floor keeps them.
    #[test]
    fn a_column_floor_pads_the_small_figures_and_never_cuts_a_large_one() {
        assert_eq!(Piconero(0).to_xmr_min(8), "0.00000000");
        assert_eq!(Piconero(30_720_000).to_xmr_min(8), "0.00003072");
        assert_eq!(Piconero(20_000).to_xmr_min(8), "0.00000002");
        assert_eq!(Piconero(4_000_000).to_xmr_min(8), "0.00000400");
        // The four per-byte levels of the node's own estimate line up, which is the row this
        // exists for.
        for v in [20_000u64, 80_000, 320_000, 4_000_000] {
            assert_eq!(Piconero(v).to_xmr_min(8).len(), 10, "{v}");
        }
        // Exactness wins over the column: twelve significant decimals stay twelve.
        assert_eq!(Piconero(1_234_567_891_234).to_xmr_min(8), "1.234567891234");
        // And the four-decimal floor is what `to_xmr_string` already was.
        assert_eq!(Piconero(500_000_000_000).to_xmr_min(4), "0.5000");
        assert_eq!(
            Piconero(500_000_000_000).to_xmr_min(4),
            Piconero(500_000_000_000).to_xmr_string()
        );
    }

    #[test]
    fn display_is_the_xmr_string_and_checked_add_refuses_to_wrap() {
        assert_eq!(format!("{}", Piconero(1_500_000_000_000)), "1.5000");
        assert_eq!(Piconero(1).checked_add(Piconero(2)).unwrap(), Piconero(3));
        let e = Piconero(u64::MAX).checked_add(Piconero(1)).unwrap_err();
        assert_eq!(e.exit_code(), 1);
    }

    #[test]
    fn an_amount_serialises_as_an_integer() {
        let v = serde_json::to_value(Piconero(42)).unwrap();
        assert_eq!(v, serde_json::json!(42));
        let back: Piconero = serde_json::from_value(serde_json::json!(7)).unwrap();
        assert_eq!(back, Piconero(7));
    }
}
