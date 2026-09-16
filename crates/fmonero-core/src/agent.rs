//! The machine contract: `capabilities` and `robot-docs` (DESIGN.md §5b, §4b).
//!
//! Phase 1a writes the parts that are **generated from tables it owns** — the exit-code
//! dictionary, the `reason` discriminators, the settings keys and the warning ids — because
//! §4 says the documented set and the constructible set may not drift, and those tables are
//! frozen here. Phase 1b adds `env`, `commands` and the real [`ROBOT_GUIDE`] (§8).
//!
//! Nothing in this file may claim more than §4b marks GREEN or AMBER (§2j).

use crate::context::Context;
use crate::error::{EXIT_CODES, REASONS};
use crate::settings::Settings;
use crate::status::WARNING_IDS;

/// What this build actually is. `version` is the crate version; this names the phase, so a
/// `capabilities` dump can never be mistaken for a finished tool.
pub const BUILD_STAGE: &str = "phase-1a-api-freeze";

/// The §4b summary, in prose, and honest about the one thing that matters: no daemon path
/// exists in this build yet.
pub const BUILD_NOTE: &str = "\
API freeze only (DESIGN.md §8, Phase 1a). The command surface, the exit codes, the `reason` \
discriminators, the settings table, amount parsing and address parsing are implemented and \
tested. Every path that talks to monerod or monero-wallet-rpc still exits 1 \
'not implemented in this build'. No Monero binary is installed on the build machine, so \
nothing that needs a live daemon has ever been observed working (§1.1, §4b RED).";

/// The agent handbook, plain text (§4). Phase 1b replaces this with the real guide; what is
/// here says only what is true of this build.
pub const ROBOT_GUIDE: &str = "\
fmonero — robot docs (phase 1a: API freeze)

This build implements the command surface, the exit codes and the local validators only.
Anything that needs monerod or monero-wallet-rpc exits 1 'not implemented in this build'.

  fmonero capabilities        # the machine contract: exit codes, reasons, settings keys
  fmonero settings list       # the 20 configuration keys, with defaults
  fmonero status --json       # the schema-1 Status document (all null until a daemon exists)

Every command takes --json, and a non-TTY stdout implies it. Every failure is one JSON
object with ok, schema, code, code_name, error, hint and reason. Branch on code_name and
reason, never on prose.
";

/// The machine contract (§5b).
///
/// `exit_codes`, `reasons`, `settings` and `warnings` are **generated** from
/// `error::EXIT_CODES`, `error::REASONS`, `Settings::KEYS` and `status::WARNING_IDS`, so a
/// documented value that cannot be constructed would be a drift those tables exist to
/// prevent. `env` and `commands` are Phase 1b's (§8).
pub fn capabilities_json(ctx: &Context) -> serde_json::Value {
    let exit_codes: Vec<serde_json::Value> = EXIT_CODES
        .iter()
        .map(|(code, name, meaning)| {
            serde_json::json!({"code": code, "code_name": name, "meaning": meaning})
        })
        .collect();
    let mut reasons: Vec<serde_json::Value> = Vec::new();
    for (code, _, name) in EXIT_CODES {
        let within: Vec<&str> = REASONS
            .iter()
            .filter(|(c, _)| c == code)
            .map(|(_, r)| *r)
            .collect();
        if !within.is_empty() {
            reasons.push(serde_json::json!({
                "code": code, "code_name": name, "reasons": within
            }));
        }
    }
    let settings: Vec<serde_json::Value> = Settings::KEYS
        .iter()
        .map(|(key, default, doc)| {
            serde_json::json!({
                "key": key,
                "default": default,
                "doc": doc,
                "value": ctx.settings.get(key).unwrap_or_default(),
            })
        })
        .collect();
    serde_json::json!({
        "tool": crate::NAME,
        "version": crate::VERSION,
        "schema": crate::SCHEMA_VERSION,
        "contract_version": crate::CONTRACT_VERSION,
        "build_stage": BUILD_STAGE,
        "build_note": BUILD_NOTE,
        "exit_codes": exit_codes,
        "reasons": reasons,
        "settings": settings,
        "warnings": WARNING_IDS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_contract_is_generated_from_the_frozen_tables() {
        let dir = std::env::temp_dir().join(format!("fmonero-caps-{}", std::process::id()));
        let ctx = Context::for_test(&dir).expect("context");
        let v = capabilities_json(&ctx);
        assert_eq!(v["exit_codes"].as_array().unwrap().len(), EXIT_CODES.len());
        assert_eq!(v["settings"].as_array().unwrap().len(), 20);
        assert!(!v["reasons"].as_array().unwrap().is_empty());
        assert_eq!(v["warnings"].as_array().unwrap().len(), WARNING_IDS.len());
        assert_eq!(v["build_stage"], BUILD_STAGE);
        // §2j: this build claims nothing about a daemon it has never reached.
        let note = v["build_note"].as_str().unwrap();
        assert!(note.contains("not implemented"), "{note}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
