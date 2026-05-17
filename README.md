# flux-types

Typed view of Adeptus **Flux** document frontmatter. Sister crate to
[`markdoc`](https://github.com/EntSP/markdoc) in the documentation pipeline:

```
.mdoc source
     │
     ▼
markdoc::parse  →  Node (frontmatter as loose Scalar::Object)
                       │
                       ▼
            flux-types::FluxFrontmatter  →  typed struct
                       │
                       ▼
              markdoc-pdf / Scriptor / Adeptus
```

[Flux](https://github.com/EntSP/flux) is the document format spec used across the platform.
`markdoc::parse` preserves the YAML frontmatter as a generic
`Scalar::Object` on the document node; this crate deserializes that
object into a typed [`FluxFrontmatter`] so consumers can read fields
directly without walking the loose map.

## What it does

- Parses every frontmatter field defined by the Flux spec — common
  fields (`id`, `type`, `title`, `documentNumber`, `status`,
  `accessLevel`, `tags`, `files`, `documentHistory`, …) plus the
  per-document-type extras for manuals, articles, notices, product
  notes, FAQs and release notes.
- Tolerates real-world inconsistencies (string-or-array
  `accessLevel`, YAML-float-encoded integers, missing fields, unknown
  future fields).
- Returns a single `FluxFrontmatter` struct with `Option<…>` /
  `Vec<…>` fields; the `doc_type` field discriminates if the consumer
  cares about which document type it is.

## Why a separate crate

`markdoc` is intentionally generic — it parses CommonMark plus the
`{% tag %}` extension and exposes frontmatter as untyped scalars. Flux
is the specific *application* schema layered on top, and is still
evolving. Keeping the typed view in its own crate means:

- `markdoc` stays application-agnostic and reusable.
- Schema additions (new document type, new field) touch only this
  crate, not the parser.
- Renderers (`markdoc-pdf`), services (Scriptor, Adeptus) and CLI
  tools share one canonical Rust definition of Flux frontmatter.

## Usage

```rust
use flux_types::FluxFrontmatter;
use markdoc::parse;

let src = std::fs::read_to_string("manual.mdoc")?;
let doc = parse(&src, None)?;

// Missing frontmatter is an error; missing fields are not.
let fm = FluxFrontmatter::from_node(&doc)?;

println!("title:   {:?}", fm.title);
println!("version: {:?}", fm.version);
println!("authors: {:?}", fm.authors);

// `accessLevel` is uniformly a slice regardless of whether the
// author wrote a string or an array.
for level in fm.access_level.as_slice() {
    println!("access: {level}");
}
```

`FluxFrontmatter::from_node(&doc)` returns `Err(FluxError::NoFrontmatter)`
when the source had no YAML block. If you'd rather treat that as
"empty frontmatter", use `.ok().unwrap_or_default()`.

## Design notes

These choices are deliberate; if you go to change them, read these
first.

### Lenient deserialization

Every field is `#[serde(default)]` and unknown fields are silently
ignored. The Flux spec is still settling and real documents already
exhibit per-doc drift (e.g. `accessLevel` is sometimes a string,
sometimes an array; YAML integers come through as floats). The
deserializer accepts both rather than rejecting either.

This means **a typo in a field name will not error** — the field
just stays at its default. That's the right tradeoff for a forward-
compatible schema, but worth knowing when debugging "why is this
field empty".

### One struct, not a sum type

Type-specific fields (`hwVersionRobot` for manuals, `swAccess` for
release notes, `noteType` for product notes, …) all live as
`Option<…>` on the same `FluxFrontmatter`. Adding a new document
type means adding more `Option` fields, not a new variant.

Trade-off: consumers that only render manuals still see `swAccess`
in their struct. The fields are cheap (`Option<String>`), the
ergonomic win — one type to pass around, no `match` on doc kind — is
worth it for a schema this fluid.

### `AccessLevel` enum

`accessLevel` appears in the wild both as a single string
(`"public"`) and an array (`["partner", "engineering"]`). The
[`AccessLevel`] enum accepts either form transparently via
`#[serde(untagged)]` and exposes `as_slice()` / `into_vec()` for
uniform consumption.

### `deser_opt_u64`

YAML integers round-trip through the Markdoc `Scalar` pipeline as
`f64`. `serde_json`'s default `u64` deserializer rejects them
("invalid type: floating point `42.0`, expected u64"). The custom
deserializer accepts whole-number floats in range. Used for
`documentNumber`, `orderNumber`, `popularity`, `schemaVersion`.

### `schema_version` forward-compat hint

If a future Flux revision changes a field shape incompatibly,
authors can stamp `schemaVersion: 2` and renderers can branch on
[`FluxFrontmatter::schema_version`]. Today it is unused; it exists
so we don't have to invent a versioning story under pressure later.

### Two-hop deserialization

`from_scalar` serializes the Markdoc `Scalar` to `serde_json::Value`
and then deserializes that into the typed struct. This is one extra
allocation but inherits all of serde's defaulting, untagged-enum, and
error-reporting behaviour for free, instead of hand-rolling a
visitor over `Scalar`.

## API surface

| Item | Purpose |
|------|---------|
| `FluxFrontmatter` | Main struct; one field per Flux frontmatter key |
| `FluxFrontmatter::from_node(&Node)` | Pull and decode from a parsed Markdoc document |
| `FluxFrontmatter::from_scalar(&Scalar)` | Decode from a raw `Scalar::Object` |
| `AccessLevel` | String-or-array polymorphic field |
| `FileRef` | Entry in the `files:` array (`path:` or `url:` variant) |
| `HistoryEntry` | Entry in `documentHistory:` |
| `Section` | Manual section, deserialized from `[path, [sub, …]]` YAML tuple |
| `HwRange` | Entry in `affectedHwRanges:` |
| `FluxError` | `NoFrontmatter` / `NotAnObject` / `Encode` / `Decode` |

## Dependencies

| Crate | Why |
|-------|-----|
| `markdoc` (path) | `Node` and `Scalar` types |
| `serde` + `serde_json` | Derivation and the two-hop decode |
| `thiserror` | Error enum |

No async, no I/O, no transitive Tokio. Safe to use in any context
that needs to read Flux frontmatter — workers, CLIs, web servers.

## Tests

```sh
cargo test
```

Tests cover every documented quirk: string-vs-array `accessLevel`, float-encoded
integers, unknown fields, missing frontmatter, the manual-sections
tuple-tree shape, and the round-trip of `authors` / `creator` /
`schemaVersion`.

## Consumers

- **markdoc-pdf** — reads `title`, `language`, `description`,
  `authors`, `creator`, `firstReleaseDate` for PDF `/Info` metadata.
- **Scriptor** (planned) — uses the same fields to drive the
  pipeline, plus `documentNumber` to gate automated rendering.
- **Adeptus** (planned) — typed view for storage and GraphQL
  exposure.

## License

MIT.
