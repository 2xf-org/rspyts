use rspyts::Type;
use serde::{Deserialize, Serialize};

/// Release channels supported by the example file format.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseChannel {
    Stable,
    Beta,
    Nightly,
}

/// Identity embedded in every compatible file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FormatIdentity {
    pub name: &'static str,
    pub format_version: u32,
}

/// Four-byte identifier at the beginning of every compatible file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(transparent)]
pub struct FormatMagic(#[rspyts(bytes)] pub [u8; 4]);

/// One feature available in a release channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FeatureDefinition {
    pub channel: ReleaseChannel,
    pub key: &'static str,
    pub introduced_in: &'static str,
    pub enabled_by_default: bool,
}

#[rspyts::export]
pub const FORMAT_VERSION: u32 = 4;

#[rspyts::export(static)]
pub const FORMAT_EPOCH: u64 = 4_294_967_296;

#[rspyts::export(static)]
pub const MEDIA_TYPE: &str = "application/x-example-archive";

#[rspyts::export(static)]
pub const FORMAT_MAGIC: FormatMagic = FormatMagic(*b"RSPY");

#[rspyts::export(static)]
pub const IDENTITY: FormatIdentity = FormatIdentity {
    name: "example_archive",
    format_version: FORMAT_VERSION,
};

#[rspyts::export(static)]
pub const RELEASE_CHANNELS: &[ReleaseChannel] = &[
    ReleaseChannel::Stable,
    ReleaseChannel::Beta,
    ReleaseChannel::Nightly,
];

#[rspyts::export(static)]
pub const REQUIRED_SECTIONS: &[&str] = &["manifest", "entries", "checksum"];

#[rspyts::export(static)]
pub const FEATURE_TABLE: &[(&str, &str)] = &[
    ("stable", "checksums"),
    ("beta", "compression_v2"),
    ("nightly", "delta_index"),
];

#[rspyts::export(static)]
pub const FEATURES: &[FeatureDefinition] = &[
    FeatureDefinition {
        channel: ReleaseChannel::Stable,
        key: "checksums",
        introduced_in: "1.0",
        enabled_by_default: true,
    },
    FeatureDefinition {
        channel: ReleaseChannel::Beta,
        key: "compression_v2",
        introduced_in: "1.4",
        enabled_by_default: false,
    },
    FeatureDefinition {
        channel: ReleaseChannel::Nightly,
        key: "delta_index",
        introduced_in: "1.5",
        enabled_by_default: false,
    },
];

/// Render a release channel for Python-only application code.
#[rspyts::export(python)]
pub fn release_channel_label(channel: ReleaseChannel) -> String {
    format!("channel:{channel:?}").to_lowercase()
}

rspyts::module!(native);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_values_are_the_contract_source() {
        assert_eq!(RELEASE_CHANNELS.len(), 3);
        assert_eq!(FEATURE_TABLE[1], ("beta", "compression_v2"));
        assert_eq!(FEATURES[2].introduced_in, "1.5");
        assert_eq!(FORMAT_EPOCH, 4_294_967_296);
        assert_eq!(release_channel_label(ReleaseChannel::Beta), "channel:beta");
    }
}
