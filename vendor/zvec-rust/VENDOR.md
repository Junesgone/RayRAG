# Vendored `zvec-ai/zvec-rust` (v0.7.1)

RayRAG links the zvec native library through `ZVEC_LIB_DIR`, so the Docker build
never needs the vendored C++ sources — but `Cargo.toml` tracks the crate by git
tag (v0.7.1 is not published on crates.io; 0.7.0 is the newest published
version). A git dependency makes `cargo` fetch the full repository **including
the `vendor/zvec` C++ submodule**, which is slow and fragile from both mainland
China and some overseas networks.

To keep `docker build` deterministic and network-tolerant, this directory holds
a verbatim copy of the two crates from tag `v0.7.1`
(`448b6b48`, release 2026-09-14):

- `zvec/` — the safe Rust bindings (`zvec-rust` 0.7.1)
- `zvec-sys/` — the raw FFI bindings (`zvec-rust-sys` 0.7.1)
- `Cargo.toml` — the upstream workspace root, so `version.workspace = true`
  resolves for both crates
- `README.md` — referenced by `zvec/Cargo.toml` (`readme = "../README.md"`)

The Dockerfile appends a `[patch]` section that redirects the git dependency to
this path, so the image build performs no GitHub fetch at all. Local (non-Docker)
builds keep using the git tag declared in `Cargo.toml`.

## Refreshing the vendored copy

```bash
tag=v0.7.1
tmp=$(mktemp -d)
curl -fsSL "https://ghfast.top/https://github.com/zvec-ai/zvec-rust/archive/refs/tags/${tag}.tar.gz" \
  | tar xz -C "$tmp" --strip-components=1
rm -rf vendor/zvec-rust/zvec vendor/zvec-rust/zvec-sys
cp -r "$tmp/zvec" "$tmp/zvec-sys" vendor/zvec-rust/
cp "$tmp/Cargo.toml" "$tmp/README.md" vendor/zvec-rust/
sed -i '/exclude = \["fuzz"\]/d' vendor/zvec-rust/Cargo.toml
```

Keep the vendored version, `Cargo.toml`'s git tag, the `ZVEC_RUST_VERSION`
build arg and the installed native library
(`~/.local/lib/zvec/<version>/libzvec_c_api.so`) in step with each other.
