import { describe, expect, expectTypeOf, it } from "vitest";

import {
  IDENTITY,
  METER_DEFINITIONS,
  METER_TABLE,
  PLATFORM_TOPIC,
  PROTOCOL_EPOCH,
  PROTOCOL_VERSION,
  PlatformEvent,
  REQUIRED_HEADERS,
  SUPPORTED_EVENTS,
  type MeterDefinition,
  type ProtocolIdentity,
} from "../../.rspyts/typescript/index.js";

describe("generated static TypeScript contract", () => {
  it("emits exact Rust enum values and protocol constants", () => {
    expect(PlatformEvent).toEqual({
      PortalHeartbeat: "portal_heartbeat",
      StudyOpened: "study_opened",
      SessionClosed: "session_closed",
    });
    expect(PROTOCOL_VERSION).toBe(4);
    expect(PROTOCOL_EPOCH).toBe(4_294_967_296n);
    expect(PLATFORM_TOPIC).toBe("example.platform.v4");
    expect(SUPPORTED_EVENTS).toEqual([
      PlatformEvent.PortalHeartbeat,
      PlatformEvent.StudyOpened,
      PlatformEvent.SessionClosed,
    ]);
  });

  it("emits string slices, tuple tables, and nested structs", () => {
    expect(REQUIRED_HEADERS).toEqual([
      "x-event-id",
      "x-clinic-id",
      "x-recorded-at",
    ]);
    expect(METER_TABLE).toEqual([
      ["portal_heartbeat", "portal.active_seconds"],
      ["study_opened", "study.opens"],
      ["session_closed", "session.seconds"],
    ]);
    expect(IDENTITY).toEqual({ service: "sample", protocolVersion: 4 });
    expect(METER_DEFINITIONS).toEqual([
      {
        event: PlatformEvent.PortalHeartbeat,
        meter: "portal.active_seconds",
        unit: "second",
        billable: false,
      },
      {
        event: PlatformEvent.StudyOpened,
        meter: "study.opens",
        unit: "event",
        billable: true,
      },
      {
        event: PlatformEvent.SessionClosed,
        meter: "session.seconds",
        unit: "second",
        billable: true,
      },
    ]);
  });

  it("publishes exact generated TypeScript types", () => {
    expectTypeOf(PROTOCOL_VERSION).toEqualTypeOf<4>();
    expectTypeOf(PROTOCOL_EPOCH).toEqualTypeOf<4_294_967_296n>();
    expectTypeOf(PLATFORM_TOPIC).toEqualTypeOf<"example.platform.v4">();
    expectTypeOf(IDENTITY).toEqualTypeOf<{
      readonly protocolVersion: 4;
      readonly service: "sample";
    }>();
    expectTypeOf(SUPPORTED_EVENTS).toEqualTypeOf<
      readonly [
        PlatformEvent.PortalHeartbeat,
        PlatformEvent.StudyOpened,
        PlatformEvent.SessionClosed,
      ]
    >();
    expectTypeOf(REQUIRED_HEADERS).toEqualTypeOf<
      readonly ["x-event-id", "x-clinic-id", "x-recorded-at"]
    >();
    expectTypeOf(METER_TABLE).toEqualTypeOf<
      readonly [
        readonly ["portal_heartbeat", "portal.active_seconds"],
        readonly ["study_opened", "study.opens"],
        readonly ["session_closed", "session.seconds"],
      ]
    >();
    expectTypeOf(METER_DEFINITIONS).toMatchTypeOf<
      readonly Readonly<MeterDefinition>[]
    >();
    expectTypeOf(IDENTITY).toMatchTypeOf<Readonly<ProtocolIdentity>>();
  });

  it("deeply freezes generated value graphs", () => {
    expect(Object.isFrozen(IDENTITY)).toBe(true);
    expect(Object.isFrozen(SUPPORTED_EVENTS)).toBe(true);
    expect(Object.isFrozen(METER_TABLE)).toBe(true);
    expect(Object.isFrozen(METER_TABLE[0])).toBe(true);
    expect(Object.isFrozen(METER_DEFINITIONS)).toBe(true);
    expect(Object.isFrozen(METER_DEFINITIONS[0])).toBe(true);
  });
});
