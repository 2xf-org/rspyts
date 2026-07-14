//! Serde-exact identifier casing shared by every expansion.
//!
//! Serde deliberately treats Rust fields and enum variants differently.
//! In particular, acronym and digit boundaries are not the same as the
//! word-splitting rules used by `heck`. Keeping the small services here
//! makes the manifest use exactly the names produced by Serde derive.

/// A `serde(rename_all = "…")` rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameRule {
    None,
    Lower,
    Upper,
    Pascal,
    Camel,
    Snake,
    ScreamingSnake,
    Kebab,
    ScreamingKebab,
}

impl RenameRule {
    /// The exact set accepted by Serde derive.
    pub const SUPPORTED: &'static str = r#""lowercase", "UPPERCASE", "PascalCase", "camelCase", "snake_case", "SCREAMING_SNAKE_CASE", "kebab-case", "SCREAMING-KEBAB-CASE""#;

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "lowercase" => Some(Self::Lower),
            "UPPERCASE" => Some(Self::Upper),
            "PascalCase" => Some(Self::Pascal),
            "camelCase" => Some(Self::Camel),
            "snake_case" => Some(Self::Snake),
            "SCREAMING_SNAKE_CASE" => Some(Self::ScreamingSnake),
            "kebab-case" => Some(Self::Kebab),
            "SCREAMING-KEBAB-CASE" => Some(Self::ScreamingKebab),
            _ => None,
        }
    }

    pub fn serde_value(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Lower => Some("lowercase"),
            Self::Upper => Some("UPPERCASE"),
            Self::Pascal => Some("PascalCase"),
            Self::Camel => Some("camelCase"),
            Self::Snake => Some("snake_case"),
            Self::ScreamingSnake => Some("SCREAMING_SNAKE_CASE"),
            Self::Kebab => Some("kebab-case"),
            Self::ScreamingKebab => Some("SCREAMING-KEBAB-CASE"),
        }
    }

    /// Apply Serde's algorithm for an enum variant identifier.
    pub fn apply_to_variant(self, variant: &str) -> String {
        match self {
            Self::None | Self::Pascal => variant.to_owned(),
            Self::Lower => variant.to_ascii_lowercase(),
            Self::Upper => variant.to_ascii_uppercase(),
            Self::Camel => lower_first_ascii(variant),
            Self::Snake => {
                let mut snake = String::new();
                for (index, ch) in variant.char_indices() {
                    if index > 0 && ch.is_uppercase() {
                        snake.push('_');
                    }
                    snake.push(ch.to_ascii_lowercase());
                }
                snake
            }
            Self::ScreamingSnake => Self::Snake.apply_to_variant(variant).to_ascii_uppercase(),
            Self::Kebab => Self::Snake.apply_to_variant(variant).replace('_', "-"),
            Self::ScreamingKebab => Self::ScreamingSnake
                .apply_to_variant(variant)
                .replace('_', "-"),
        }
    }

    /// Apply Serde's algorithm for a struct or variant field identifier.
    pub fn apply_to_field(self, field: &str) -> String {
        match self {
            Self::None | Self::Lower | Self::Snake => field.to_owned(),
            Self::Upper => field.to_ascii_uppercase(),
            Self::Pascal => {
                let mut pascal = String::new();
                let mut capitalize = true;
                for ch in field.chars() {
                    if ch == '_' {
                        capitalize = true;
                    } else if capitalize {
                        pascal.push(ch.to_ascii_uppercase());
                        capitalize = false;
                    } else {
                        pascal.push(ch);
                    }
                }
                pascal
            }
            Self::Camel => lower_first_ascii(&Self::Pascal.apply_to_field(field)),
            Self::ScreamingSnake => field.to_ascii_uppercase(),
            Self::Kebab => field.replace('_', "-"),
            Self::ScreamingKebab => field.to_ascii_uppercase().replace('_', "-"),
        }
    }
}

fn lower_first_ascii(value: &str) -> String {
    match value.char_indices().nth(1) {
        Some((end, _)) => value[..end].to_ascii_lowercase() + &value[end..],
        None => value.to_ascii_lowercase(),
    }
}

/// camelCase for Rust field-style identifiers such as parameters.
pub fn to_camel(ident: &str) -> String {
    RenameRule::Camel.apply_to_field(ident)
}

pub fn field_wire_name(ident: &str, rule: RenameRule) -> String {
    rule.apply_to_field(ident)
}

pub fn variant_wire_name(ident: &str, rule: RenameRule) -> String {
    rule.apply_to_variant(ident)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_rules_match_serde_edge_cases() {
        assert_eq!(to_camel("minimum_value"), "minimumValue");
        assert_eq!(to_camel("http2_server"), "http2Server");
        assert_eq!(
            RenameRule::Pascal.apply_to_field("xml_http2_id"),
            "XmlHttp2Id"
        );
        assert_eq!(
            RenameRule::Kebab.apply_to_field("xml_http2_id"),
            "xml-http2-id"
        );
        assert_eq!(
            RenameRule::ScreamingSnake.apply_to_field("http2_id"),
            "HTTP2_ID"
        );
    }

    #[test]
    fn variant_rules_preserve_serde_acronym_and_digit_behavior() {
        assert_eq!(
            RenameRule::Camel.apply_to_variant("HTTP2Server"),
            "hTTP2Server"
        );
        assert_eq!(
            RenameRule::Snake.apply_to_variant("HTTP2Server"),
            "h_t_t_p2_server"
        );
        assert_eq!(
            RenameRule::Kebab.apply_to_variant("IPv6Route"),
            "i-pv6-route"
        );
        assert_eq!(
            RenameRule::Lower.apply_to_variant("HTTP2Server"),
            "http2server"
        );
    }

    #[test]
    fn every_serde_rule_round_trips() {
        for rule in [
            RenameRule::Lower,
            RenameRule::Upper,
            RenameRule::Pascal,
            RenameRule::Camel,
            RenameRule::Snake,
            RenameRule::ScreamingSnake,
            RenameRule::Kebab,
            RenameRule::ScreamingKebab,
        ] {
            let value = rule.serde_value().unwrap();
            assert_eq!(RenameRule::parse(value), Some(rule));
        }
        assert_eq!(RenameRule::parse(""), None);
    }
}
