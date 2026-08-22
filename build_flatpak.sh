#!/usr/bin/env bash
#
# Build the Flatpak locally the same way Flathub does, and check beforehand that
# the release is actually in a state Flathub can build.
#
# The manifest pulls its source from GitHub at `tag:`, so a local build only
# proves anything if that tag is pushed and matches this working tree. The
# preflight checks below cover exactly the mistakes that have broken a Flathub
# PR before — a stale `commit:`, an unpushed tag, a manifest lagging behind
# Cargo.toml.
#
# Usage: ./build_flatpak.sh [--no-install] [--no-bundle] [--skip-checks]

set -euo pipefail

cd "$(dirname "$(readlink -f "$0")")"

MANIFEST="me.dumke.Reinschrift.yml"
APP_ID="me.dumke.Reinschrift"
METAINFO="me.dumke.Reinschrift.metainfo.xml"
BUILD_DIR="build-dir"
REPO_DIR="repo"
BUNDLE="reinschrift.flatpak"
# A build tree plus the exported repo needs a few GB; running out halfway
# leaves a half-written repo that fails in confusing ways.
MIN_FREE_MB=4096

usage() {
    cat <<'USAGE'
Build the Flatpak locally the same way Flathub does.

Usage: ./build_flatpak.sh [OPTION]...

  --no-install    build and bundle, but do not install
  --no-bundle     do not create the .flatpak bundle (implies --no-install)
  --skip-checks   skip the preflight checks (tag pushed, versions in sync)
  -h, --help      show this help
USAGE
}

do_install=1
do_bundle=1
do_checks=1
for arg in "$@"; do
    case "$arg" in
        --no-install) do_install=0 ;;
        --no-bundle)  do_bundle=0; do_install=0 ;;
        --skip-checks) do_checks=0 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown option: $arg" >&2; usage >&2; exit 2 ;;
    esac
done

info()  { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
ok()    { printf '    \033[32m✓\033[0m %s\n' "$*"; }
warn()  { printf '    \033[33m!\033[0m %s\n' "$*"; }
note()  { printf '      %s\n' "$*"; }
fail()  { printf '    \033[31m✗\033[0m %s\n' "$*" >&2; exit 1; }

# Without this, a failure deep inside flatpak-builder just stops with whatever
# that tool printed and no indication of which step was running.
current_step="startup"
on_error() {
    local status=$?
    printf '\n\033[31m✗ failed during: %s (exit %d)\033[0m\n' "$current_step" "$status" >&2
    exit "$status"
}
trap on_error ERR

step() { current_step="$*"; info "$*"; }

current_step="checking prerequisites"
for tool in flatpak flatpak-builder git python3 awk df; do
    command -v "$tool" >/dev/null || fail "$tool is not installed"
done
[[ -f "$MANIFEST" ]] || fail "$MANIFEST not found"
[[ -f Cargo.toml ]] || fail "Cargo.toml not found"

# Read the fields we care about straight out of the manifest, so nothing in this
# script can drift away from it the way the hardcoded runtime version used to.
# The parse runs in its own step: an `eval` of a failed command substitution is
# silent, and the first symptom would be an "unbound variable" further down.
if ! manifest_fields=$(
    python3 - "$MANIFEST" <<'PY'
import re
import shlex
import sys

try:
    import yaml
except ImportError:
    sys.exit(
        "PyYAML is not installed — install it with "
        "'dnf install python3-pyyaml' or 'pip install PyYAML'"
    )

try:
    with open(sys.argv[1], encoding="utf-8") as fh:
        manifest = yaml.safe_load(fh)
except (OSError, yaml.YAMLError) as exc:
    sys.exit(f"could not parse {sys.argv[1]}: {exc}")

if not isinstance(manifest, dict):
    sys.exit(f"{sys.argv[1]} does not contain a manifest mapping")

git_source = next(
    (s for m in manifest.get("modules", [])
       if isinstance(m, dict)
       for s in m.get("sources", [])
       if isinstance(s, dict) and s.get("type") == "git"),
    {},
)

# The workspace version is the one the manifest tag has to agree with.
cargo_version = "-"
try:
    text = open("Cargo.toml", encoding="utf-8").read()
    _, _, section = text.partition("[workspace.package]")
    match = re.search(r'^version\s*=\s*"([^"]+)"', section, re.M)
    if match:
        cargo_version = match.group(1)
except OSError as exc:
    sys.exit(f"could not read Cargo.toml: {exc}")

fields = {
    "RUNTIME": manifest.get("runtime", "-"),
    "RUNTIME_VERSION": str(manifest.get("runtime-version", "-")),
    "SDK": manifest.get("sdk", "-"),
    # flatpak-builder defaults to "master" when the manifest names no branch.
    "BRANCH": str(manifest.get("branch") or "master"),
    "SOURCE_URL": git_source.get("url", "-"),
    "SOURCE_TAG": git_source.get("tag", "-"),
    "SOURCE_COMMIT": git_source.get("commit", "-"),
    "SDK_EXTENSIONS": " ".join(manifest.get("sdk-extensions", [])),
    "CARGO_VERSION": cargo_version,
}
for key, value in fields.items():
    print(f"{key}={shlex.quote(str(value))}")
PY
); then
    fail "could not read $MANIFEST — see the message above"
fi
eval "$manifest_fields"

[[ "$RUNTIME" != "-" && "$RUNTIME_VERSION" != "-" ]] \
    || fail "$MANIFEST declares no runtime/runtime-version"

if (( do_checks )); then
    step "Checking the release is buildable by Flathub"
    echo "    manifest: $RUNTIME//$RUNTIME_VERSION, source $SOURCE_TAG"

    [[ "$SOURCE_TAG" != "-" ]] || fail "the manifest has no tag: — Flathub builds from a tag"
    [[ "$SOURCE_URL" != "-" ]] || fail "the manifest git source has no url:"
    [[ "$CARGO_VERSION" != "-" ]] \
        || fail "no version found under [workspace.package] in Cargo.toml"

    # The manifest tag must describe this working tree, not the previous release.
    [[ "$SOURCE_TAG" == "v$CARGO_VERSION" ]] \
        || fail "manifest tag $SOURCE_TAG does not match Cargo.toml version $CARGO_VERSION"
    ok "manifest tag matches Cargo.toml ($CARGO_VERSION)"

    git rev-parse --git-dir >/dev/null 2>&1 || fail "not inside a git repository"
    tag_commit=$(git rev-parse -q --verify "refs/tags/$SOURCE_TAG^{commit}") \
        || fail "tag $SOURCE_TAG does not exist locally — create it before building"
    ok "tag $SOURCE_TAG resolves to ${tag_commit:0:9}"

    # flatpak-builder fetches from GitHub, so an unpushed tag fails there, not
    # here. Keep "could not reach the remote" apart from "the tag is missing" —
    # they need different fixes.
    if ! ls_remote=$(git ls-remote "$SOURCE_URL" "refs/tags/$SOURCE_TAG^{}" 2>&1); then
        warn "could not reach $SOURCE_URL to verify the tag is pushed"
        note "${ls_remote%%$'\n'*}"
        note "the build below will fail the same way if the tag really is missing"
    else
        remote_tag=$(printf '%s' "$ls_remote" | awk 'NR==1 {print $1}')
        [[ -n "$remote_tag" ]] || fail "tag $SOURCE_TAG is not pushed to $SOURCE_URL"
        [[ "$remote_tag" == "$tag_commit" ]] \
            || fail "remote tag $SOURCE_TAG points at ${remote_tag:0:9}, locally it is ${tag_commit:0:9}"
        ok "tag is pushed and matches the remote"
    fi

    # This is what broke Flathub PR #24: tag bumped, commit: left on the old release.
    if [[ "$SOURCE_COMMIT" != "-" ]]; then
        [[ "$SOURCE_COMMIT" == "$tag_commit" ]] \
            || fail "commit: ${SOURCE_COMMIT:0:9} does not match tag $SOURCE_TAG (${tag_commit:0:9})"
        ok "commit: matches the tag"
    else
        note "no commit: here by design — add 'commit: $tag_commit' when copying into the Flathub repo"
    fi

    if [[ -n "$(git status --porcelain)" ]]; then
        note "working tree is dirty; the build uses $SOURCE_TAG from GitHub, not these files"
    fi

    free_mb=$(df -Pm . | awk 'NR==2 {print $4}')
    if [[ -n "$free_mb" ]] && (( free_mb < MIN_FREE_MB )); then
        warn "only ${free_mb} MB free here; a build plus repo wants about ${MIN_FREE_MB} MB"
    fi

    if command -v appstreamcli >/dev/null; then
        if appstreamcli validate --no-net "$METAINFO" >/dev/null 2>&1; then
            ok "$METAINFO validates"
        else
            note "appstreamcli reported issues in $METAINFO:"
            appstreamcli validate --no-net "$METAINFO" 2>&1 | sed 's/^/      /' || true
        fi
    fi
fi

# Let flatpak-builder resolve runtime, SDK and every sdk-extension from the
# manifest itself — hardcoding versions here is how this script ended up asking
# for GNOME 49 while the manifest had already moved to 50.
step "Installing runtime and SDK declared by the manifest"

if ! flatpak remotes --columns=name 2>/dev/null | grep -qx flathub; then
    fail "no 'flathub' remote configured — add it with:
      flatpak remote-add --if-not-exists --user flathub https://dl.flathub.org/repo/flathub.flatpakrepo"
fi

# Is everything the manifest declares already installed, in any installation?
deps_present() {
    flatpak info "$RUNTIME//$RUNTIME_VERSION" >/dev/null 2>&1 || return 1
    flatpak info "$SDK//$RUNTIME_VERSION" >/dev/null 2>&1 || return 1
    local ext
    for ext in $SDK_EXTENSIONS; do
        flatpak list --columns=application 2>/dev/null | grep -qx "$ext" || return 1
    done
}

# A failure here is handled below, so keep the ERR trap out of it.
deps_status=0
flatpak-builder --user --install-deps-from=flathub --install-deps-only \
    --force-clean "$BUILD_DIR" "$MANIFEST" || deps_status=$?

if (( deps_status != 0 )); then
    # flatpak-builder has been seen to segfault here rather than exit cleanly,
    # which makes a Flathub-side hiccup look like a broken manifest.
    if deps_present; then
        warn "could not refresh dependencies, but all of them are installed — building with those"
        note "(a 404 on a .filez object is missing on Flathub's side; nothing local fixes it)"
    else
        cat >&2 <<'HINT'

    Installing the dependencies failed and they are not all present.

    If this is a 404 on a .filez object, that object is missing on Flathub's
    side and no local repair helps — retry later. Otherwise try:

        flatpak repair --user && flatpak update --user

HINT
        exit 1
    fi
fi

step "Building $APP_ID from $SOURCE_TAG"
flatpak-builder --force-clean --repo="$REPO_DIR" "$BUILD_DIR" "$MANIFEST"

if (( do_bundle )); then
    step "Creating bundle $BUNDLE"
    flatpak build-bundle "$REPO_DIR" "$BUNDLE" "$APP_ID" "$BRANCH"
    ok "$BUNDLE ($(du -h "$BUNDLE" | cut -f1))"
fi

if (( do_install )); then
    # Installing does not touch an instance that is already running; without
    # this note the new build looks like it did not take effect.
    if flatpak ps --columns=application 2>/dev/null | grep -qx "$APP_ID"; then
        warn "$APP_ID is running — quit and restart it to pick up this build"
    fi

    step "Installing $BUNDLE"
    flatpak install --user --noninteractive --or-update "$BUNDLE"

    installed=$(LC_ALL=C flatpak info --user "$APP_ID//$BRANCH" 2>/dev/null \
        | awk -F': *' '/^ *Version:/ {print $2; exit}')
    if [[ "$installed" == "$CARGO_VERSION" ]]; then
        ok "installed version $installed"
    elif [[ -n "$installed" ]]; then
        warn "installed version is $installed, expected $CARGO_VERSION"
    fi
fi

step "Done"
run_cmd="flatpak run $APP_ID"
# Several branches installed side by side (a Flathub 'stable' next to this
# 'master') make a bare `flatpak run` ambiguous.
branch_count=$(flatpak list --columns=application,branch 2>/dev/null \
    | awk -v app="$APP_ID" '$1 == app' | wc -l)
if (( branch_count > 1 )); then
    run_cmd="flatpak run --user --branch=$BRANCH $APP_ID"
    note "$branch_count branches of $APP_ID are installed, so name the one you mean"
fi
echo "    Run with:  $run_cmd"
if [[ "$SOURCE_COMMIT" == "-" && -n "${tag_commit:-}" ]]; then
    echo "    Flathub:   commit: $tag_commit"
fi
