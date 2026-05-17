//! Typed view of Flux frontmatter.
//!
//! Flux is the document specification used by the Adeptus platform.
//! `markdoc::parse` preserves the YAML frontmatter as a generic
//! `Scalar::Object` on the document node; this module deserializes that
//! object into the typed [`FluxFrontmatter`] struct so renderers can read
//! fields directly without walking the loose map.
//!
//! Design notes:
//!
//! - **Lenient**: every field is `#[serde(default)]` and unknown fields
//!   are ignored. The Flux spec is still settling and real docs
//!   already exhibit per-doc inconsistencies (e.g. `accessLevel` is
//!   sometimes a string, sometimes an array). The deserializer is built
//!   to tolerate both rather than reject them.
//!
//! - **All document types in one struct**: type-specific fields like
//!   `hwVersionRobot` (manuals, articles) and `swAccess` (release notes)
//!   are collapsed into the same struct as `Option<…>`. Adding a new
//!   document type later means adding more `Option` fields, not a new
//!   sum type. `doc_type` discriminates if the renderer cares.
//!
//! - **`schema_version`**: a hint for forward-compat. If a future Flux
//!   bump introduces an incompatible field shape, authors can stamp
//!   their docs with the new version and renderers can branch on it.
//!
//! - **Two-hop deserialize via `serde_json::Value`**: the Markdoc
//!   `Scalar` already round-trips through serde, so we serialize it to
//!   `serde_json::Value` and then deserialize that into the typed
//!   struct. This keeps the mapping straightforward and inherits all
//!   serde features (defaults, error reporting).

use markdoc::Node;
use markdoc::types::Scalar;
use serde::{Deserialize, Deserializer};
use std::path::PathBuf;
use thiserror::Error;

/// Custom deserializer for `Option<u64>` fields. YAML integers come through
/// the Markdoc Scalar pipeline as `f64`, so serde_json's strict u64
/// deserializer rejects them ("invalid type: floating point `42.0`,
/// expected u64"). Accept whole-number floats as long as they're in range.
fn deser_opt_u64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    use serde::de::Error;
    let v = Option::<serde_json::Value>::deserialize(d)?;
    match v {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(n)) => {
            if let Some(u) = n.as_u64() {
                return Ok(Some(u));
            }
            if let Some(f) = n.as_f64() {
                if f.fract() == 0.0 && f >= 0.0 && f <= u64::MAX as f64 {
                    return Ok(Some(f as u64));
                }
                return Err(D::Error::custom(format!(
                    "expected a non-negative whole number, got {f}"
                )));
            }
            Err(D::Error::custom("number out of range"))
        }
        Some(other) => Err(D::Error::custom(format!("expected number, got {other:?}"))),
    }
}

#[derive(Debug, Error)]
pub enum FluxError {
    #[error("document node has no `frontmatter` attribute")]
    NoFrontmatter,
    #[error("frontmatter is not an object")]
    NotAnObject,
    #[error("failed to encode frontmatter scalar: {0}")]
    Encode(serde_json::Error),
    #[error("failed to decode frontmatter into FluxFrontmatter: {0}")]
    Decode(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct FluxFrontmatter {
    // ── Common to every document type ──────────────────────────────────
    pub id: Option<String>,
    /// Document type discriminator: `release_note`, `manual`, `notice`,
    /// `product_note`, `article`, `faq` (per the revised Flux spec).
    #[serde(rename = "type")]
    pub doc_type: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    /// Triggers automated PDF generation in the pipeline when present.
    #[serde(deserialize_with = "deser_opt_u64")]
    pub document_number: Option<u64>,
    /// `draft` / `review` / `published` / `archived` / `unpublished`
    pub status: Option<String>,
    pub version: Option<String>,
    /// Language-country code, e.g. `"en-us"` or `"en"`.
    pub language: Option<String>,
    pub first_release_date: Option<String>,
    pub update_date: Option<String>,
    pub access_level: AccessLevel,
    /// Document authors — combined into PDF `/Author` metadata.
    pub authors: Vec<String>,
    /// Application / pipeline that produced this document — surfaced
    /// as PDF `/Creator` metadata. When absent, the renderer falls
    /// back to its own default ("markdoc-pdf" for the bundled bin).
    pub creator: Option<String>,
    pub tags: Vec<String>,
    pub files: Vec<FileRef>,
    pub document_history: Vec<HistoryEntry>,

    // ── Manual / Article ───────────────────────────────────────────────
    pub hw_version_robot: Option<String>,
    #[serde(rename = "hwVersionTM")]
    pub hw_version_tm: Option<String>,
    pub sw_version: Option<String>,
    pub products: Vec<String>,
    pub config_file: Option<PathBuf>,
    pub sections: Vec<Section>,

    // ── Notice / Product Note ──────────────────────────────────────────
    /// For Notices: `product` | `safety`. For FAQs: free-form category path.
    pub category: Option<String>,
    pub affected_products: Vec<String>,
    pub affected_hw_ranges: Vec<HwRange>,
    pub expiry_date: Option<String>,
    /// Product Note: `new_product` | `hw_update` | `replacement` | `retired` | `eol`.
    pub note_type: Option<String>,
    pub replacement_products: Vec<String>,
    pub effective_date: Option<String>,

    // ── Article extras ─────────────────────────────────────────────────
    /// For spare-part guides.
    #[serde(deserialize_with = "deser_opt_u64")]
    pub order_number: Option<u64>,

    // ── FAQ extras ─────────────────────────────────────────────────────
    pub question: Option<String>,
    #[serde(deserialize_with = "deser_opt_u64")]
    pub popularity: Option<u64>,

    // ── Release Note extras ────────────────────────────────────────────
    pub sw_access: Option<String>,

    // ── Forward-compat marker ──────────────────────────────────────────
    /// Optional schema-version stamp. Renderers may branch on this if
    /// they have to support multiple Flux schema generations.
    #[serde(deserialize_with = "deser_opt_u64")]
    pub schema_version: Option<u64>,
}

/// `accessLevel` appears in real Flux documents both as a single string
/// (e.g. `"public"`) and as an array (e.g. `["partner", "engineering"]`).
/// This enum accepts either form transparently and exposes a uniform
/// `as_slice()` view for consumers.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum AccessLevel {
    Single(String),
    Multiple(Vec<String>),
    /// Frontmatter omitted the field.
    #[serde(skip)]
    #[default]
    Empty,
}

impl AccessLevel {
    /// Returns the access-level entries as a list, regardless of
    /// whether they were authored as a string or an array.
    pub fn as_slice(&self) -> Vec<&str> {
        match self {
            Self::Empty => Vec::new(),
            Self::Single(s) => vec![s.as_str()],
            Self::Multiple(v) => v.iter().map(String::as_str).collect(),
        }
    }

    pub fn into_vec(self) -> Vec<String> {
        match self {
            Self::Empty => Vec::new(),
            Self::Single(s) => vec![s],
            Self::Multiple(v) => v,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct FileRef {
    /// Local path (relative to the document) for files committed alongside.
    pub path: Option<String>,
    /// External URL for resources hosted elsewhere.
    pub url: Option<String>,
    /// Mime category — `image`, `link`, `pdf`, etc.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub description: Option<String>,
    /// Per-file access override; otherwise inherits from the document.
    pub access_level: Option<AccessLevel>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct HistoryEntry {
    /// Optional in the spec
    pub version: Option<String>,
    pub date: Option<String>,
    pub description: Option<String>,
}

/// A manual section authored as `[path, [subpath, ...]]` in YAML.
///
/// Real example from `hw_2.1_manual.md`:
/// ```yaml
/// sections:
/// - - "/manual/intro/intro_to_productx"
///   - []
/// - - "/manual/safety/intro_to_safety"
///   - - "manual/safety/safety_message_types"
///     - "manual/safety/general_safety_precautions"
/// ```
///
/// Deserialized as a 2-tuple, then converted to this struct.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(from = "(String, Vec<String>)")]
pub struct Section {
    pub path: String,
    pub subsections: Vec<String>,
}

impl From<(String, Vec<String>)> for Section {
    fn from((path, subsections): (String, Vec<String>)) -> Self {
        Self { path, subsections }
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct HwRange {
    pub from: Option<String>,
    pub to: Option<String>,
    pub product: Option<String>,
}

impl FluxFrontmatter {
    /// Pull the `frontmatter` attribute off a Document node and
    /// deserialize it. Returns `Err(NoFrontmatter)` if the node has no
    /// frontmatter (i.e. the source document had no YAML block).
    pub fn from_node(node: &Node) -> Result<Self, FluxError> {
        let scalar = node
            .attributes
            .get("frontmatter")
            .ok_or(FluxError::NoFrontmatter)?;
        Self::from_scalar(scalar)
    }

    /// Deserialize from a raw `Scalar` (must be `Scalar::Object`).
    pub fn from_scalar(scalar: &Scalar) -> Result<Self, FluxError> {
        if !matches!(scalar, Scalar::Object(_)) {
            return Err(FluxError::NotAnObject);
        }
        let value = serde_json::to_value(scalar).map_err(FluxError::Encode)?;
        Ok(serde_json::from_value(value)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use markdoc::parse;

    fn parse_fm(yaml: &str) -> FluxFrontmatter {
        let src = format!("---\n{yaml}---\n\nbody");
        let doc = parse(&src, None).unwrap();
        FluxFrontmatter::from_node(&doc).unwrap()
    }

    #[test]
    fn parses_full_manual_frontmatter() {
        let fm = parse_fm(
            r#"id: "item-2025-11-04-002"
type: "manual"
title: "ProductX Manual"
documentNumber: 75371702
status: "published"
version: "5.3"
language: "en"
firstReleaseDate: "2024-07-30"
updateDate: "2025-11-10"
accessLevel: "public"
tags:
- "produtx"
- "manual"
hwVersionRobot: "2.1"
hwVersionTM: null
swVersion: "3.x"
products:
- "ProdutX"
configFile: "products/productx/hw_2.1_config.json"
documentHistory:
- version: "5.3"
  date: "2025-11-10"
  description: "Added monthly maintenance task."
"#,
        );

        assert_eq!(fm.id.as_deref(), Some("item-2025-11-04-002"));
        assert_eq!(fm.doc_type.as_deref(), Some("manual"));
        assert_eq!(fm.title.as_deref(), Some("ProductX Manual"));
        assert_eq!(fm.document_number, Some(75371702));
        assert_eq!(fm.version.as_deref(), Some("5.3"));
        assert_eq!(fm.first_release_date.as_deref(), Some("2024-07-30"));
        assert_eq!(fm.access_level.as_slice(), vec!["public"]);
        assert_eq!(fm.tags, vec!["productx", "manual"]);
        assert_eq!(fm.hw_version_robot.as_deref(), Some("2.1"));
        assert!(fm.hw_version_tm.is_none()); // explicit null
        assert_eq!(fm.sw_version.as_deref(), Some("3.x"));
        assert_eq!(fm.products, vec!["ProductX"]);
        assert_eq!(
            fm.config_file.as_ref().unwrap().to_string_lossy(),
            "products/productx/hw_2.1_config.json"
        );
        assert_eq!(fm.document_history.len(), 1);
        let h = &fm.document_history[0];
        assert_eq!(h.version.as_deref(), Some("5.3"));
        assert_eq!(h.date.as_deref(), Some("2025-11-10"));
    }

    #[test]
    fn parses_release_note_frontmatter_with_access_level_array() {
        let fm = parse_fm(
            r#"id: "item-2025-11-04-001"
type: "release_note"
title: "Software 1.8.0"
status: "published"
version: "1"
language: "en"
firstReleaseDate: "2025-11-04"
updateDate: "2025-11-04"
tags: ["bugfix", "3.7.3"]
accessLevel:
- "partner"
- "engineering"
swVersion: "1.8.0"
swAccess: "standard"
"#,
        );
        assert_eq!(fm.doc_type.as_deref(), Some("release_note"));
        assert_eq!(fm.access_level.as_slice(), vec!["partner", "engineering"]);
        assert_eq!(fm.sw_version.as_deref(), Some("1.8.0"));
        assert_eq!(fm.sw_access.as_deref(), Some("standard"));
    }

    #[test]
    fn parses_manual_sections_tuple_tree() {
        let fm = parse_fm(
            r#"type: "manual"
title: "Test"
sections:
- - "/manual/intro/intro_to_productx"
  - []
- - "/manual/safety/intro_to_safety"
  - - "manual/safety/safety_message_types"
    - "manual/safety/general_safety_precautions"
"#,
        );
        assert_eq!(fm.sections.len(), 2);
        assert_eq!(fm.sections[0].path, "/manual/intro/intro_to_productx");
        assert!(fm.sections[0].subsections.is_empty());
        assert_eq!(fm.sections[1].path, "/manual/safety/intro_to_safety");
        assert_eq!(fm.sections[1].subsections.len(), 2);
        assert_eq!(
            fm.sections[1].subsections[0],
            "manual/safety/safety_message_types"
        );
    }

    #[test]
    fn parses_files_array_with_path_and_url_variants() {
        let fm = parse_fm(
            r#"type: "release_item"
title: "x"
files:
- path: "assets/screenshot.png"
  type: "image"
  description: "Screenshot of new graph"
- url: "https://design.example.com/specs/v2"
  type: "link"
  description: "Design spec"
"#,
        );
        assert_eq!(fm.files.len(), 2);
        assert_eq!(fm.files[0].path.as_deref(), Some("assets/screenshot.png"));
        assert_eq!(fm.files[0].kind.as_deref(), Some("image"));
        assert!(fm.files[0].url.is_none());
        assert_eq!(
            fm.files[1].url.as_deref(),
            Some("https://design.example.com/specs/v2")
        );
        assert!(fm.files[1].path.is_none());
    }

    #[test]
    fn missing_frontmatter_errors() {
        let doc = parse("# No frontmatter here", None).unwrap();
        let err = FluxFrontmatter::from_node(&doc).unwrap_err();
        assert!(matches!(err, FluxError::NoFrontmatter));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        // Forward-compat: a doc with fields the current schema doesn't
        // know about must still deserialize without error.
        let fm = parse_fm(
            r#"title: "Future"
unknownNewField: "from-the-future"
anotherUnknown: 42
"#,
        );
        assert_eq!(fm.title.as_deref(), Some("Future"));
    }

    #[test]
    fn empty_frontmatter_yields_default() {
        let fm = parse_fm("");
        assert_eq!(fm, FluxFrontmatter::default());
    }

    #[test]
    fn access_level_empty_when_field_absent() {
        let fm = parse_fm("title: x\n");
        assert!(fm.access_level.as_slice().is_empty());
        assert!(matches!(fm.access_level, AccessLevel::Empty));
    }

    #[test]
    fn schema_version_round_trips() {
        let fm = parse_fm("schemaVersion: 2\ntitle: x\n");
        assert_eq!(fm.schema_version, Some(2));
    }

    #[test]
    fn access_level_with_only_a_string() {
        let fm = parse_fm("accessLevel: public\ntitle: x\n");
        assert_eq!(fm.access_level.as_slice(), vec!["public"]);
        assert!(matches!(fm.access_level, AccessLevel::Single(_)));
    }

    #[test]
    fn parses_authors_and_creator() {
        let fm = parse_fm("title: x\nauthors:\n- Alice\n- Bob\ncreator: \"docs-pipeline v3\"\n");
        assert_eq!(fm.authors, vec!["Alice", "Bob"]);
        assert_eq!(fm.creator.as_deref(), Some("docs-pipeline v3"));
    }

    #[test]
    fn authors_default_empty_creator_default_none() {
        let fm = parse_fm("title: x\n");
        assert!(fm.authors.is_empty());
        assert!(fm.creator.is_none());
    }

    #[test]
    fn from_scalar_rejects_non_object() {
        let err =
            FluxFrontmatter::from_scalar(&Scalar::String("not an object".into())).unwrap_err();
        assert!(matches!(err, FluxError::NotAnObject));
    }
}
