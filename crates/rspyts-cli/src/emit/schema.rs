//! The JSON Schema emitter (codegen.md §6).
//!
//! Emits one draft 2020-12 bundle, `schema.json`: a `$defs` entry per
//! named data type (error enums have no data shape and are skipped),
//! wire-cased property names, `additionalProperties: false`, integer
//! bounds, and docs as `description`. `serde_json`'s `preserve_order`
//! feature keeps keys in authoring order.

use super::util::{doc_lines, int_bounds};
use rspyts_core::ir::{FieldDecl, Manifest, Ty, TypeDecl};
use serde_json::{Map, Value, json};

/// Emit `schema.json`.
pub fn emit(m: &Manifest, hash: &str) -> Vec<(&'static str, String)> {
    let mut root = Map::new();
    root.insert(
        "$schema".to_string(),
        json!("https://json-schema.org/draft/2020-12/schema"),
    );
    let mut meta = Map::new();
    meta.insert("version".to_string(), json!(m.crate_version));
    meta.insert("crate".to_string(), json!(m.crate_name));
    meta.insert("manifestHash".to_string(), json!(format!("sha256:{hash}")));
    root.insert("x-rspyts".to_string(), Value::Object(meta));

    let mut defs = Map::new();
    for decl in &m.types {
        match decl {
            TypeDecl::Struct {
                name, docs, fields, ..
            } => {
                let mut def = Map::new();
                add_description(&mut def, docs);
                object_schema(&mut def, None, fields);
                defs.insert(name.clone(), Value::Object(def));
            }
            TypeDecl::StringEnum {
                name,
                docs,
                variants,
                ..
            } => {
                let mut def = Map::new();
                add_description(&mut def, docs);
                def.insert("type".to_string(), json!("string"));
                def.insert(
                    "enum".to_string(),
                    Value::Array(variants.iter().map(|v| json!(v.wire_name)).collect()),
                );
                defs.insert(name.clone(), Value::Object(def));
            }
            TypeDecl::Enum {
                name,
                docs,
                tag,
                variants,
                ..
            } => {
                let mut def = Map::new();
                add_description(&mut def, docs);
                let one_of: Vec<Value> = variants
                    .iter()
                    .map(|v| {
                        let mut variant = Map::new();
                        object_schema(&mut variant, Some((tag, &v.wire_name)), &v.fields);
                        Value::Object(variant)
                    })
                    .collect();
                def.insert("oneOf".to_string(), Value::Array(one_of));
                defs.insert(name.clone(), Value::Object(def));
            }
            // Error enums project to exceptions, never to a data shape.
            TypeDecl::ErrorEnum { .. } => {}
        }
    }
    root.insert("$defs".to_string(), Value::Object(defs));

    let mut text = serde_json::to_string_pretty(&Value::Object(root))
        .expect("schema serialization cannot fail");
    text.push('\n');
    vec![("schema.json", text)]
}

/// Fill `def` with an object schema: optional tag property first, then
/// the fields, then `required` and `additionalProperties: false`.
fn object_schema(def: &mut Map<String, Value>, tag: Option<(&str, &str)>, fields: &[FieldDecl]) {
    def.insert("type".to_string(), json!("object"));
    let mut properties = Map::new();
    let mut required: Vec<Value> = Vec::new();
    if let Some((tag_key, tag_value)) = tag {
        let mut prop = Map::new();
        prop.insert("const".to_string(), json!(tag_value));
        properties.insert(tag_key.to_string(), Value::Object(prop));
        required.push(json!(tag_key));
    }
    for f in fields {
        let mut prop = Map::new();
        add_description(&mut prop, &f.docs);
        extend_with(&mut prop, ty_schema(&f.ty));
        properties.insert(f.wire_name.clone(), Value::Object(prop));
        // Optional fields may be omitted on input (they default to null).
        if !(f.optional || matches!(f.ty, Ty::Option { .. })) {
            required.push(json!(f.wire_name.clone()));
        }
    }
    def.insert("properties".to_string(), Value::Object(properties));
    if !required.is_empty() {
        def.insert("required".to_string(), Value::Array(required));
    }
    def.insert("additionalProperties".to_string(), json!(false));
}

fn add_description(map: &mut Map<String, Value>, docs: &str) {
    let lines = doc_lines(docs);
    if !lines.is_empty() {
        map.insert("description".to_string(), json!(lines.join("\n")));
    }
}

fn extend_with(map: &mut Map<String, Value>, value: Value) {
    let Value::Object(entries) = value else {
        unreachable!("ty_schema always returns an object")
    };
    for (k, v) in entries {
        // Field docs win over the type's own description (`Json`).
        map.entry(k).or_insert(v);
    }
}

/// The schema of one [`Ty`] in a data position.
fn ty_schema(ty: &Ty) -> Value {
    if let Some((lo, hi)) = int_bounds(ty) {
        let mut map = Map::new();
        map.insert("type".to_string(), json!("integer"));
        map.insert("minimum".to_string(), json!(lo));
        map.insert("maximum".to_string(), json!(hi));
        return Value::Object(map);
    }
    match ty {
        Ty::Bool => json!({"type": "boolean"}),
        Ty::F32 | Ty::F64 => json!({"type": "number"}),
        Ty::String => json!({"type": "string"}),
        Ty::Unit => json!({"type": "null"}),
        Ty::Option { inner } => json!({"anyOf": [ty_schema(inner), {"type": "null"}]}),
        Ty::List { inner } => {
            let mut map = Map::new();
            map.insert("type".to_string(), json!("array"));
            map.insert("items".to_string(), ty_schema(inner));
            Value::Object(map)
        }
        Ty::Map { value } => {
            let mut map = Map::new();
            map.insert("type".to_string(), json!("object"));
            map.insert("additionalProperties".to_string(), ty_schema(value));
            Value::Object(map)
        }
        Ty::Ref { name } => json!({"$ref": format!("#/$defs/{name}")}),
        // Schemaless passthrough: the empty schema accepts anything.
        Ty::Json => json!({"description": "schemaless"}),
        // Buf crosses as the tail placeholder object (ABI §6).
        Ty::Buf { dt } => json!({
            "type": "object",
            "properties": {
                "__rspyts_buf__": {
                    "type": "object",
                    "properties": {
                        "off": {"type": "integer", "minimum": 0},
                        "len": {"type": "integer", "minimum": 0},
                        "dt": {"const": dt.wire_name()}
                    },
                    "required": ["off", "len", "dt"],
                    "additionalProperties": false
                }
            },
            "required": ["__rspyts_buf__"],
            "additionalProperties": false
        }),
        Ty::Slice { .. } => unreachable!("slices are param-only; validation rejects them here"),
        // Bounded integers are handled above.
        Ty::U8 | Ty::U16 | Ty::U32 | Ty::I8 | Ty::I16 | Ty::I32 => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_manifest::{manifest, manifest_hash};
    use super::*;

    #[test]
    fn schema_json_matches_golden() {
        let m = manifest();
        let hash = manifest_hash(&m);
        let (name, actual) = emit(&m, &hash).remove(0);
        assert_eq!(name, "schema.json");
        let expected = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "x-rspyts": {
    "version": "0.1.0",
    "crate": "demo-crate",
    "manifestHash": "sha256:@HASH@"
  },
  "$defs": {
    "AnalysisParams": {
      "description": "Parameters controlling the analysis pass.",
      "type": "object",
      "properties": {
        "minDurationS": {
          "description": "Minimum duration, in seconds.",
          "type": "number"
        },
        "threshold": {
          "anyOf": [
            {
              "type": "number"
            },
            {
              "type": "null"
            }
          ]
        },
        "metadata": {
          "description": "schemaless"
        }
      },
      "required": [
        "minDurationS",
        "metadata"
      ],
      "additionalProperties": false
    },
    "CatalogInfo": {
      "description": "Catalog description reported by the device.",
      "type": "object",
      "properties": {
        "vendor": {
          "type": "string"
        },
        "channelCount": {
          "type": "integer",
          "minimum": 0,
          "maximum": 65535
        }
      },
      "required": [
        "vendor",
        "channelCount"
      ],
      "additionalProperties": false
    },
    "Severity": {
      "type": "string",
      "enum": [
        "low",
        "medium",
        "high"
      ]
    },
    "ThresholdEvent": {
      "description": "Signal threshold transitions.",
      "oneOf": [
        {
          "type": "object",
          "properties": {
            "kind": {
              "const": "crossed"
            },
            "atSample": {
              "type": "integer",
              "minimum": 0,
              "maximum": 4294967295
            },
            "value": {
              "type": "number"
            }
          },
          "required": [
            "kind",
            "atSample",
            "value"
          ],
          "additionalProperties": false
        },
        {
          "type": "object",
          "properties": {
            "kind": {
              "const": "cleared"
            },
            "atSample": {
              "type": "integer",
              "minimum": 0,
              "maximum": 4294967295
            }
          },
          "required": [
            "kind",
            "atSample"
          ],
          "additionalProperties": false
        }
      ]
    }
  }
}
"#
        .replace("@HASH@", &hash);
        if actual != expected {
            let diff = similar::TextDiff::from_lines(expected.as_str(), actual.as_str());
            panic!(
                "schema.json does not match its golden:\n{}",
                diff.unified_diff()
                    .context_radius(3)
                    .header("expected", "actual")
            );
        }
    }

    #[test]
    fn buf_schema_is_the_placeholder_shape() {
        let v = ty_schema(&Ty::Buf {
            dt: rspyts_core::ir::Dtype::F32,
        });
        assert_eq!(
            v["properties"]["__rspyts_buf__"]["properties"]["dt"]["const"],
            "f32"
        );
        assert_eq!(v["additionalProperties"], false);
    }

    #[test]
    fn integer_bounds_are_emitted() {
        let v = ty_schema(&Ty::I16);
        assert_eq!(v["minimum"], -32768);
        assert_eq!(v["maximum"], 32767);
    }

    #[test]
    fn json_is_the_empty_schema_with_a_marker_description() {
        let v = ty_schema(&Ty::Json);
        assert_eq!(v, json!({"description": "schemaless"}));
    }

    #[test]
    fn field_docs_win_over_the_json_marker_description() {
        let mut prop = Map::new();
        add_description(&mut prop, "What the caller sent.");
        extend_with(&mut prop, ty_schema(&Ty::Json));
        assert_eq!(prop["description"], "What the caller sent.");
    }

    #[test]
    fn foreign_origin_types_are_always_inlined() {
        // The schema bundle is self-contained: origin and import
        // mappings never remove a definition.
        let m = manifest();
        let hash = manifest_hash(&m);
        let (_, text) = emit(&m, &hash).remove(0);
        assert!(text.contains("\"CatalogInfo\""), "{text}");
    }
}
