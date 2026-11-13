# Vendored office2pdf

Source: <https://crates.io/crates/office2pdf> v0.10.0, Apache-2.0.

Only the library source and embedded fonts are vendored. The upstream
`Cargo.toml` declares both `cdylib` and `rlib` crate types; the `cdylib` target
is not needed by DocForge and exceeds the MinGW export-ordinal limit on Windows
GNU builds. The only patch is:

```toml
[lib]
crate-type = ["rlib"]
```

The Rust API is unchanged. Update this directory when upgrading `office2pdf`
and re-apply only that crate-type change.
