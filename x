#!/bin/sh
# One repository entry point. Keep CI, contributor docs, and local checks on the
# same commands so no environment has a private definition of "green".
set -eu
cd "$(dirname "$0")"

# Print the command contract; errors exit 2 and explicit help exits 0.
usage() {
    cat >&2 <<'EOF'
usage: ./x [check|build|fmt|lint|test|guard|shell|serve|stdio] [args...]
  check        default: formatting, lint, workspace tests, shell syntax, guard
  build        cargo build arguments, e.g. --release
  fmt          cargo fmt arguments; --check leaves sources unchanged
  lint         cargo clippy arguments before --; warnings are denied
  test         cargo test arguments, e.g. -p search guard
  guard|shell  security-surface audit or shell syntax; no arguments
  serve|stdio  build and run Search, forwarding application arguments
  help         show this help
EOF
    exit "${1:-2}"
}

command=${1-check}
if [ "$#" -gt 0 ]; then
    shift
fi

case "$command" in
    build)
        cargo build --locked "$@"
        ;;
    fmt)
        cargo fmt --all "$@"
        ;;
    lint)
        cargo clippy --locked --all-targets --all-features "$@" -- -D warnings
        ;;
    test)
        cargo test --locked --workspace "$@"
        ;;
    check)
        [ "$#" -eq 0 ] || usage
        ./x fmt --check
        ./x lint
        ./x test
        ./x shell
        ./x guard
        ;;
    guard)
        [ "$#" -eq 0 ] || usage
        sh scripts/guard.sh
        ;;
    # Check release-script syntax before it reaches a release.
    shell)
        [ "$#" -eq 0 ] || usage
        for script in x install.sh scripts/*.sh; do
            sh -n "$script"
        done
        echo "shell: ok"
        ;;
    serve)
        cargo build --locked
        exec ./target/debug/search serve "$@"
        ;;
    stdio)
        cargo build --locked
        exec ./target/debug/search "$@"
        ;;
    -h|--help|help)
        [ "$#" -eq 0 ] || usage
        usage 0
        ;;
    *)
        usage
        ;;
esac
