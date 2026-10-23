# Mirrors the CI job locally. Windows/macOS are compile-checked only.
ci: fmt clippy test check-other
    cargo xtask size

fmt:
    cargo fmt --all --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings
    cargo clippy --no-default-features -- -D warnings
    cargo clippy --all-targets --no-default-features --features images -- -D warnings
    cargo clippy --all-targets --no-default-features --features images,embed-fonts -- -D warnings
    cargo clippy --all-targets --no-default-features --features svg -- -D warnings

test:
    cargo test --workspace
    cargo test --lib --no-default-features --features embed-fonts
    cargo test --lib --no-default-features --features images,embed-fonts

# Portability guard: needs `rustup target add x86_64-pc-windows-gnu aarch64-apple-darwin`.
check-other:
    cargo check --target x86_64-pc-windows-gnu
    cargo check --target aarch64-apple-darwin

size:
    cargo xtask size

# Every performance budget, measured on release builds (needs a display); fails on a miss.
budgets:
    cargo xtask budgets

# The image memory tests, one process each and in parallel (-j N, --runs K, --keep-going).
images *args:
    cargo xtask images {{args}}

# Fuzz a target (doc, image, anim, layout, settings, svg) on stable for SECONDS.
fuzz target seconds="60":
    scripts/fuzz.sh {{target}} {{seconds}}

# The release tarball in dist/.
dist:
    cargo xtask dist

# Build and check both AUR packages offline, without installing (commit first).
aur:
    scripts/aur-check.sh

# Select, copy and paste against Xvfb and a headless sway in containers (needs docker).
clipboard:
    scripts/clipboard-check.sh

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
