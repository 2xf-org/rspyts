import {
  IDENTITY,
  METER_DEFINITIONS,
  METER_TABLE,
  REQUIRED_HEADERS,
  SUPPORTED_EVENTS,
  PlatformEvent,
  type MeterDefinition,
  type ProtocolIdentity,
} from "../../.rspyts/typescript/index.js";

declare const identity: ProtocolIdentity;
declare const meter: MeterDefinition;

const identities: Readonly<ProtocolIdentity> = IDENTITY;
const meters: readonly Readonly<MeterDefinition>[] = METER_DEFINITIONS;
const events: readonly PlatformEvent[] = SUPPORTED_EVENTS;
void identities;
void meters;
void events;

// @ts-expect-error Generated struct fields are readonly.
identity.service = "other";
// @ts-expect-error Generated struct fields are readonly.
meter.billable = false;
// @ts-expect-error Exported struct constants are deeply readonly.
IDENTITY.protocolVersion = 5;
// @ts-expect-error Exported list constants are readonly tuples.
METER_DEFINITIONS.push(meter);
// @ts-expect-error Objects nested in exported constants are readonly.
METER_DEFINITIONS[0].meter = "other";
// @ts-expect-error Exported string slices are readonly.
REQUIRED_HEADERS[0] = "other";
// @ts-expect-error Tuples nested in exported constants are readonly.
METER_TABLE[0][0] = "other";
