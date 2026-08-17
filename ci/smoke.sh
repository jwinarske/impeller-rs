#!/usr/bin/env bash
# Verification run: format, lint, build, test, and the feature matrix.
#
# Every step's exit status is captured directly rather than through a pipe.
# Piping cargo into `tail` hands the pipeline `tail`'s status, which is always
# zero, so a broken build reports success -- this script previously did exactly
# that and passed while the workspace did not compile.
set -uo pipefail

cd "$(dirname "$0")/.."
fail=0

step() {
    local name="$1"
    shift
    local output
    if output=$("$@" 2>&1); then
        printf '  %-28s ok\n' "$name"
    else
        printf '  %-28s FAILED\n' "$name"
        printf '%s\n' "$output" | tail -25 | sed 's/^/      /'
        fail=1
    fi
}

echo "verification"
step "rustfmt" cargo fmt --all -- --check
step "clippy" cargo clippy --workspace --all-targets -- -D warnings
step "build" cargo build --workspace --all-targets
step "test" cargo test --workspace

echo "feature matrix"
for features in vulkan gles vulkan,gles,drm; do
    step "$features" cargo check -p impeller --no-default-features --features "$features"
done

# The facade's default features compile only Vulkan, so the tests that compare
# the two backends through the public API skip in the workspace run above.
# Running them again with both compiled in is what makes those tests real
# rather than a pair of early returns that always pass.
step "both backends" cargo test -p impeller --features gles

if [ "$fail" -eq 0 ]; then
    total=$(cargo test --workspace 2>&1 |
        awk -F'[ ;]' '/test result/ {s+=$4} END {print s}')
    echo "SMOKE: PASS (${total} tests)"
else
    echo "SMOKE: FAIL"
fi
exit "$fail"
