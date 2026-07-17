import {
  FEATURES,
  FEATURE_TABLE,
  FORMAT_MAGIC,
  IDENTITY,
  RELEASE_CHANNELS,
  REQUIRED_SECTIONS,
  ReleaseChannel,
  type FeatureDefinition,
  type FormatIdentity,
} from "../../.rspyts/typescript/index.js";

// @ts-expect-error Python-only functions are not part of a static TypeScript package.
import { releaseChannelLabel } from "../../.rspyts/typescript/index.js";
void releaseChannelLabel;

declare const identity: FormatIdentity;
declare const feature: FeatureDefinition;

const identities: Readonly<FormatIdentity> = IDENTITY;
const features: readonly Readonly<FeatureDefinition>[] = FEATURES;
const channels: readonly ReleaseChannel[] = RELEASE_CHANNELS;
void identities;
void features;
void channels;

// @ts-expect-error Generated struct fields are readonly.
identity.name = "other";
// @ts-expect-error Generated struct fields are readonly.
feature.enabledByDefault = false;
// @ts-expect-error Exported struct constants are deeply readonly.
IDENTITY.formatVersion = 5;
// @ts-expect-error Exported fixed byte arrays are readonly tuples.
FORMAT_MAGIC[0] = 0;
// @ts-expect-error Exported list constants are readonly tuples.
FEATURES.push(feature);
// @ts-expect-error Objects nested in exported constants are readonly.
FEATURES[0].key = "other";
// @ts-expect-error Exported string slices are readonly.
REQUIRED_SECTIONS[0] = "other";
// @ts-expect-error Tuples nested in exported constants are readonly.
FEATURE_TABLE[0][0] = "other";
