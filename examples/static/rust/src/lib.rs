use rspyts::Type;
use serde::Serialize;

/// Protocol events shared by Portal and Hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PlatformEvent {
    PortalHeartbeat,
    StudyOpened,
    SessionClosed,
}
/// Identity shipped with every platform event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolIdentity {
    pub service: &'static str,
    pub protocol_version: u32,
}

/// One stable mapping from an event to its metering identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MeterDefinition {
    pub event: PlatformEvent,
    pub meter: &'static str,
    pub unit: &'static str,
    pub billable: bool,
}

#[rspyts::export(static)]
pub const PROTOCOL_VERSION: u32 = 4;

#[rspyts::export(static)]
pub const PROTOCOL_EPOCH: u64 = 4_294_967_296;

#[rspyts::export(static)]
pub const PLATFORM_TOPIC: &str = "neurovirtual.platform.v4";

#[rspyts::export(static)]
pub const IDENTITY: ProtocolIdentity = ProtocolIdentity {
    service: "cloud",
    protocol_version: PROTOCOL_VERSION,
};

#[rspyts::export(static)]
pub const SUPPORTED_EVENTS: &[PlatformEvent] = &[
    PlatformEvent::PortalHeartbeat,
    PlatformEvent::StudyOpened,
    PlatformEvent::SessionClosed,
];

#[rspyts::export(static)]
pub const REQUIRED_HEADERS: &[&str] = &["x-event-id", "x-clinic-id", "x-recorded-at"];

#[rspyts::export(static)]
pub const METER_TABLE: &[(&str, &str)] = &[
    ("portal_heartbeat", "portal.active_seconds"),
    ("study_opened", "study.opens"),
    ("session_closed", "session.seconds"),
];

#[rspyts::export(static)]
pub const METER_DEFINITIONS: &[MeterDefinition] = &[
    MeterDefinition {
        event: PlatformEvent::PortalHeartbeat,
        meter: "portal.active_seconds",
        unit: "second",
        billable: false,
    },
    MeterDefinition {
        event: PlatformEvent::StudyOpened,
        meter: "study.opens",
        unit: "event",
        billable: true,
    },
    MeterDefinition {
        event: PlatformEvent::SessionClosed,
        meter: "session.seconds",
        unit: "second",
        billable: true,
    },
];

rspyts::module!(native);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_values_are_the_contract_source() {
        assert_eq!(SUPPORTED_EVENTS.len(), 3);
        assert_eq!(METER_TABLE[1], ("study_opened", "study.opens"));
        assert_eq!(METER_DEFINITIONS[2].unit, "second");
        assert_eq!(PROTOCOL_EPOCH, 4_294_967_296);
    }
}
