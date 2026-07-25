# 🧩 shared/proto README

## What This Is

This folder contains the cross-language Protocol Buffer definitions used by
Vortex.

The schemas in this directory define:

- shared wire message structures
- payload envelopes
- transport-safe typed messages
- long-term compatibility boundaries between Rust and Kotlin codebases

The main file is:

- `vortex.proto`

## Why This Folder Matters

`shared/proto` is the contract layer between platforms.

If the schema changes carelessly:

- Android and Linux can drift apart
- decode failures become hard to debug
- backward compatibility can be broken silently

Treat schema changes as protocol changes, not just code changes.

## How It Is Used

Rust side:

- generated through `prost-build`
- usually invoked from `build.rs`
- generated types should not be edited by hand

Kotlin side:

- generated through `protobuf-gradle-plugin`
- integrated into the Gradle build graph
- generated outputs should remain derived artifacts

## Generation Commands

Rust generation:

```bash
cargo build
```

Expected behavior:

- `build.rs` invokes `prost-build`
- Rust types are regenerated automatically when schema changes

Kotlin generation:

```bash
./gradlew generateProto
```

Expected behavior:

- Gradle plugin regenerates Java or Kotlin-compatible Proto classes
- Android modules consume generated sources from the build directory

## Field Numbering Rules

Field numbers are permanent protocol real estate.

Rules:

- `1-15`: reserve for frequently used fields because they encode in one byte
- `16+`: use for less frequent fields
- `NEVER` reuse numbers from removed fields

Bad practice:

- deleting a field and reusing its old number for something new

Good practice:

- mark removed fields as `reserved`
- add a new field number for changed semantics

## Versioning Strategy

Safe changes:

- adding optional fields
- adding new messages
- adding new enum values with safe defaults

Unsafe or breaking changes:

- changing field types
- reusing field numbers
- changing existing semantics without version gating

When removing a field:

```proto
message Example {
  reserved 4;
}
```

## Compatibility Rules

Backward compatibility expectations:

- older peers ignore unknown optional fields
- newer peers tolerate missing optional fields
- message wrappers should remain stable once experiments converge

Forward planning:

- reserve room for new payload types
- document every schema-affecting decision in protocol review

## Testing

Required test types:

- round-trip encode/decode tests
- Rust encode -> Kotlin decode tests
- Kotlin encode -> Rust decode tests
- malformed payload handling tests

Good test cases:

- empty optional fields
- populated nested messages
- unknown field preservation or ignore behavior where applicable

## Files

- `vortex.proto` — primary wire protocol schema for Vortex

## TODO / FIXME / NOTE

- TODO: add example generated output paths once Rust and Android builds exist
- FIXME: confirm whether Kotlin generation targets Java or Kotlin stubs in MVP
- NOTE: keep this directory small and contract-focused
