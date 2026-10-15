# Mirrors the CI job locally. Windows/macOS are compile-checked only.
ci: fmt clippy test check-other
    cargo xtask size

fmt:
    cargo fmt --all --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings
    cargo clippy --no-default-features -- -D warnings

test:
    cargo test --workspace

# Portability guard: needs `rustup target add x86_64-pc-windows-gnu aarch64-apple-darwin`.
check-other:
    cargo check --target x86_64-pc-windows-gnu
    cargo check --target aarch64-apple-darwin

size:
    cargo xtask size

# Regenerate the render goldens (tests/golden/); review the images before committing.
bless:
    cargo xtask bless

bench fixture="":
    cargo xtask bench {{fixture}}

# Regenerate assets/fonts from upstream release zips extracted into DIR.
fonts dir:
    scripts/subset-fonts.sh {{dir}}

# No network access without I: needs a display and user namespaces.
netns:
    cargo build --release
    scripts/netns-check.sh
