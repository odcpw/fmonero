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
                "'all' is not an amount; `wallet sweep` moves the whole account and takes no amount",
            )
            .with_reason("bad_amount")
            .with_hint("fmonero wallet sweep <address> --dry-run   # to move everything"));
        }
        let bad = |why: &str| {
            Error::usage(format!("'{s}' is not an amount: {why}"))
                .with_reason("bad_amount")
                .with_hint("fmonero wallet send <address> 0.01 --dry-run   # XMR, or 10000000000p")
        };
        if s.is_empty() {
            return Err(bad("it is empty"));
        }
        // Raw piconero: an integer followed by `p`.
        if let Some(digits) = s.strip_suffix('p') {
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad("a `p` amount is an integer number of piconero"));
            }
            let value: u64 = digits
                .parse()
                .map_err(|_| bad("it does not fit in 64 bits of piconero"))?;
            return Ok(Piconero(value));
        }
        // Decimal XMR.
        let (int_part, frac_part) = match s.split_once('.') {
            Some((i, f)) => (i, f),
            None => (s, ""),
        };
        if int_part.is_empty() && frac_part.is_empty() {
            return Err(bad("a bare '.' is not a number"));
        }
        if !int_part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad(
                "only digits and one '.' are allowed (no separators, no sign, no exponent)",
            ));
        }
        if !frac_part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad(
                "only digits and one '.' are allowed (no separators, no sign, no exponent)",
            ));
        }
        if frac_part.len() > DECIMALS {
            return Err(bad("Monero has 12 decimal places"));
        }
        let whole: u64 = if int_part.is_empty() {
            0
        } else {
            int_part
                .parse()
                .map_err(|_| bad("it does not fit in 64 bits of piconero"))?
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
            .ok_or_else(|| bad("it does not fit in 64 bits of piconero"))?;
        Ok(Piconero(value))
    }

    /// 12 decimals, trailing zeros trimmed to a minimum of four. **Exact, never rounded
    /// or truncated**, so the `*_xmr` string always agrees with the `*_pico` integer §5
    /// prints beside it: `1_234_567_891_234` → `"1.234567891234"`,
    /// `500_000_000_000` → `"0.5000"`, `0` → `"0.0000"`.
    pub fn to_xmr_string(self) -> String {
        let whole = self.0 / Self::PER_XMR;
        let frac = self.0 % Self::PER_XMR;
        let mut digits = format!("{frac:0width$}", width = DECIMALS);
        while digits.len() > MIN_DECIMALS && digits.ends_with('0') {
            digits.pop();
        }
        format!("{whole}.{digits}")
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
