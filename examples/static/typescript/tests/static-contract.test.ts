import { describe, expect, expectTypeOf, it } from "vitest";

import {
  FEATURES,
  FEATURE_TABLE,
  FORMAT_EPOCH,
  FORMAT_MAGIC,
  FORMAT_VERSION,
  IDENTITY,
  MEDIA_TYPE,
  RELEASE_CHANNELS,
  REQUIRED_SECTIONS,
  ReleaseChannel,
  type FeatureDefinition,
  type FormatIdentity,
} from "../../.rspyts/typescript/index.js";
import * as contract from "../../.rspyts/typescript/index.js";

describe("generated static TypeScript contract", () => {
  it("omits Python-only behavior", () => {
    expect("releaseChannelLabel" in contract).toBe(false);
  });

  it("emits exact Rust enum values and format constants", () => {
    expect(ReleaseChannel).toEqual({
      Stable: "stable",
      Beta: "beta",
      Nightly: "nightly",
    });
    expect(FORMAT_VERSION).toBe(4);
    expect(FORMAT_EPOCH).toBe(4_294_967_296);
    expect(MEDIA_TYPE).toBe("application/x-example-archive");
    expect(FORMAT_MAGIC).toEqual([82, 83, 80, 89]);
    expect(RELEASE_CHANNELS).toEqual([
      ReleaseChannel.Stable,
      ReleaseChannel.Beta,
      ReleaseChannel.Nightly,
    ]);
  });

  it("emits string slices, tuple tables, and nested structs", () => {
    expect(REQUIRED_SECTIONS).toEqual(["manifest", "entries", "checksum"]);
    expect(FEATURE_TABLE).toEqual([
      ["stable", "checksums"],
      ["beta", "compression_v2"],
      ["nightly", "delta_index"],
    ]);
    expect(IDENTITY).toEqual({ name: "example_archive", formatVersion: 4 });
    expect(FEATURES).toEqual([
      {
        channel: ReleaseChannel.Stable,
        key: "checksums",
        introducedIn: "1.0",
        enabledByDefault: true,
      },
      {
        channel: ReleaseChannel.Beta,
        key: "compression_v2",
        introducedIn: "1.4",
        enabledByDefault: false,
      },
      {
        channel: ReleaseChannel.Nightly,
        key: "delta_index",
        introducedIn: "1.5",
        enabledByDefault: false,
      },
    ]);
  });

  it("publishes exact generated TypeScript types", () => {
    expectTypeOf(FORMAT_VERSION).toEqualTypeOf<4>();
    expectTypeOf(FORMAT_EPOCH).toEqualTypeOf<4_294_967_296>();
    expectTypeOf(MEDIA_TYPE).toEqualTypeOf<"application/x-example-archive">();
    expectTypeOf(FORMAT_MAGIC).toEqualTypeOf<readonly [82, 83, 80, 89]>();
    expectTypeOf(IDENTITY).toEqualTypeOf<{
      readonly formatVersion: 4;
      readonly name: "example_archive";
    }>();
    expectTypeOf(RELEASE_CHANNELS).toEqualTypeOf<
      readonly [
        ReleaseChannel.Stable,
        ReleaseChannel.Beta,
        ReleaseChannel.Nightly,
      ]
    >();
    expectTypeOf(REQUIRED_SECTIONS).toEqualTypeOf<
      readonly ["manifest", "entries", "checksum"]
    >();
    expectTypeOf(FEATURE_TABLE).toEqualTypeOf<
      readonly [
        readonly ["stable", "checksums"],
        readonly ["beta", "compression_v2"],
        readonly ["nightly", "delta_index"],
      ]
    >();
    expectTypeOf(FEATURES).toMatchTypeOf<
      readonly Readonly<FeatureDefinition>[]
    >();
    expectTypeOf(IDENTITY).toMatchTypeOf<Readonly<FormatIdentity>>();
  });

  it("deeply freezes generated value graphs", () => {
    expect(Object.isFrozen(IDENTITY)).toBe(true);
    expect(Object.isFrozen(FORMAT_MAGIC)).toBe(true);
    expect(Object.isFrozen(RELEASE_CHANNELS)).toBe(true);
    expect(Object.isFrozen(FEATURE_TABLE)).toBe(true);
    expect(Object.isFrozen(FEATURE_TABLE[0])).toBe(true);
    expect(Object.isFrozen(FEATURES)).toBe(true);
    expect(Object.isFrozen(FEATURES[0])).toBe(true);
  });
});
