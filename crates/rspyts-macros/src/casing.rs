//! Identifier casing shared by every expansion.
//!
//! Wire names recorded in the manifest must match what the emitted serde
//! attributes produce at runtime. Both halves of that equation are
//! centralized here: [`RenameRule`] yields the literal handed to
//! `#[serde(rename_all = …)]` *and* computes the corresponding wire name,
//! so they cannot drift apart.

use heck::{ToLowerCamelCase, ToSnakeCase};

/// A field-renaming rule mirrored between the emitted
/// `#[serde(rename_all = …)]` attribute and the manifest wire names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameRule {
    /// `camelCase` — the default everywhere.
    Camel,
    /// `snake_case` — opt-in per struct via `#[bridge(rename_all = "snake_case")]`.
    Snake,
}

impl RenameRule {
    /// Human-readable list of accepted `rename_all` values, for diagnostics.
    pub const SUPPORTED: &'static str = r#""camelCase", "snake_case""#;

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "camelCase" => Some(Self::Camel),
            "snake_case" => Some(Self::Snake),
            _ => None,
        }
    }

    /// The exact string handed to `#[serde(rename_all = …)]`.
    pub fn serde_value(self) -> &'static str {
        match self {
            Self::Camel => "camelCase",
            Self::Snake => "snake_case",
        }
    }

    /// Compute the wire name of `ident` under this rule.
    pub fn apply(self, ident: &str) -> String {
        match self {
            Self::Camel => ident.to_lower_camel_case(),
            Self::Snake => ident.to_snake_case(),
        }
    }
}

/// camelCase — used for variant wire names, error codes, parameter keys,
/// and `data` object keys.
pub fn to_camel(ident: &str) -> String {
    ident.to_lower_camel_case()
}

/// The wire name of `ident` under `rule`. The same rule is handed to the
/// emitted `#[serde(rename_all = …)]`, so manifest and wire cannot
/// disagree.
pub fn wire_name(ident: &str, rule: RenameRule) -> String {
    rule.apply(ident)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel_matches_serde_for_snake_case_fields() {
        assert_eq!(to_camel("min_duration_s"), "minDurationS");
        assert_eq!(to_camel("sample_rate"), "sampleRate");
        assert_eq!(to_camel("samples"), "samples");
        assert_eq!(to_camel("worst_event"), "worstEvent");
    }

    #[test]
    fn camel_matches_serde_for_pascal_case_variants() {
        assert_eq!(to_camel("Crossed"), "crossed");
        assert_eq!(to_camel("InvalidSampleRate"), "invalidSampleRate");
        assert_eq!(to_camel("WindowTooLarge"), "windowTooLarge");
    }

    #[test]
    fn snake_rule_is_identity_for_snake_case_fields() {
        assert_eq!(RenameRule::Snake.apply("min_duration_s"), "min_duration_s");
    }

    #[test]
    fn parse_accepts_only_supported_rules() {
        assert_eq!(RenameRule::parse("camelCase"), Some(RenameRule::Camel));
        assert_eq!(RenameRule::parse("snake_case"), Some(RenameRule::Snake));
        assert_eq!(RenameRule::parse("PascalCase"), None);
        assert_eq!(RenameRule::parse(""), None);
    }

    #[test]
    fn wire_name_applies_the_rule() {
        assert_eq!(wire_name("login_event", RenameRule::Camel), "loginEvent");
        assert_eq!(wire_name("login_event", RenameRule::Snake), "login_event");
    }

    #[test]
    fn serde_value_round_trips_through_parse() {
        for rule in [RenameRule::Camel, RenameRule::Snake] {
            assert_eq!(RenameRule::parse(rule.serde_value()), Some(rule));
        }
    }
}
