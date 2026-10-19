#!/bin/sh
# install.sh — Download and install the latest devo binary for Linux / macOS.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh -s -- --version v0.2.0

set -eu

APP="devo"
REPO="7df-lab/devo"
RG_APP="rg"
RG_REPO="BurntSushi/ripgrep"
INSTALL_DIR_DEFAULT="${HOME}/.devo/bin"

MUTED="$(printf '\033[0;2m')"
RED="$(printf '\033[0;31m')"
ORANGE="$(printf '\033[38;5;214m')"
NC="$(printf '\033[0m')"

requested_version="${VERSION:-}"
binary_path=""
no_modify_path="false"
offline_mode="false"
install_dir="${DEVO_INSTALL_DIR:-$INSTALL_DIR_DEFAULT}"
skip_app_install="false"

usage() {
    cat <<EOF
devo Installer

Usage: install.sh [options]

Options:
    -h, --help              Display this help message
    -v, --version <version> Install a specific version (for example: v0.2.0)
    -b, --binary <path>     Install from a local binary instead of downloading
        --install-dir <dir> Install into a custom directory
        --offline           Install from assets placed next to install.sh without network access
        --no-modify-path    Don't modify shell config files

Environment:
    VERSION                 Same as --version
    DEVO_INSTALL_DIR        Same as --install-dir
    DEVO_RUNTIME_CACHE      Shared private runtime cache (default: XDG cache/devo/runtimes)
    DEVO_RELEASE_BASE_URL   Release mirror/CDN base, followed by /<version>/<asset>
    DEVO_RUNTIME_BASE_URL   Optional runtime CDN base, followed by /<runtime-asset>
    DEVO_SKIP_RG_INSTALL=1 Skip installing the ripgrep sidecar

Examples:
    curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh
    curl -fsSL https://raw.githubusercontent.com/7df-lab/devo/main/install.sh | sh -s -- --version v0.2.0
    sh ./install.sh --offline
    ./install.sh --binary ./target/release/devo
EOF
}

print_message() {
    level="$1"
    message="$2"

    case "$level" in
        info) color="$NC" ;;
        warning) color="$ORANGE" ;;
        error) color="$RED" ;;
        *) color="$NC" ;;
    esac

    printf '%b%s%b\n' "$color" "$message" "$NC"
}

die() {
    print_message error "$1" >&2
    exit 1
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        -h|--help)
            usage
            exit 0
            ;;
        -v|--version)
            if [ -n "${2:-}" ]; then
                requested_version="$2"
                shift 2
            else
                die "Error: --version requires a version argument"
            fi
            ;;
        -b|--binary)
            if [ -n "${2:-}" ]; then
                binary_path="$2"
                shift 2
            else
                die "Error: --binary requires a path argument"
            fi
            ;;
        --install-dir)
            if [ -n "${2:-}" ]; then
                install_dir="$2"
                shift 2
            else
                die "Error: --install-dir requires a directory argument"
            fi
            ;;
        --offline)
            offline_mode="true"
            shift
            ;;
        --no-modify-path)
            no_modify_path="true"
            shift
            ;;
        *)
            print_message warning "Warning: Unknown option '$1'" >&2
            shift
            ;;
    esac
done

require_command() {
    command_name="$1"
    hint="$2"

    if ! command -v "$command_name" >/dev/null 2>&1; then
        die "$hint"
    fi
}

is_truthy() {
    case "${1:-}" in
        1|true|TRUE|yes|YES|on|ON) return 0 ;;
        *) return 1 ;;
    esac
}



normalize_version() {
    version="$1"
    version="${version#v}"
    printf 'v%s\n' "$version"
}

detect_target() {
    raw_os="$(uname -s)"
    raw_arch="$(uname -m)"

    case "$raw_os" in
        Linux) os="unknown-linux-musl" ;;
        Darwin) os="apple-darwin" ;;
        *)
            die "Unsupported OS: $raw_os. This installer supports Linux and macOS. For Windows, use install.ps1."
            ;;
    esac

    case "$raw_arch" in
        x86_64|amd64) arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        *)
            die "Unsupported architecture: $raw_arch"
            ;;
    esac

    printf '%s-%s\n' "$arch" "$os"
}

detect_rg_target() {
    raw_os="$(uname -s)"
    raw_arch="$(uname -m)"

    case "$raw_os" in
        Linux)
            case "$raw_arch" in
                x86_64|amd64) printf '%s\n' "x86_64-unknown-linux-musl" ;;
                aarch64|arm64) printf '%s\n' "aarch64-unknown-linux-gnu" ;;
                *) die "Unsupported architecture for ripgrep: $raw_arch" ;;
            esac
            ;;
        Darwin)
            case "$raw_arch" in
                x86_64|amd64) printf '%s\n' "x86_64-apple-darwin" ;;
                aarch64|arm64) printf '%s\n' "aarch64-apple-darwin" ;;
                *) die "Unsupported architecture for ripgrep: $raw_arch" ;;
            esac
            ;;
        *)
            die "Unsupported OS for ripgrep: $raw_os"
            ;;
    esac
}

resolve_latest_version() {
    require_command curl "Error: 'curl' is required but not installed."
    require_command sed "Error: 'sed' is required but not installed."

    latest="$(
        curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
            | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' \
            | sed -n '1p'
    )"

    if [ -z "$latest" ]; then
        die "Failed to resolve the latest release version"
    fi

    printf '%s\n' "$latest"
}

resolve_latest_ripgrep_version() {
    require_command curl "Error: 'curl' is required but not installed."
    require_command sed "Error: 'sed' is required but not installed."

    latest="$(
        curl -fsSL "https://api.github.com/repos/${RG_REPO}/releases/latest" \
            | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' \
            | sed -n '1p'
    )"

    if [ -z "$latest" ]; then
        die "Failed to resolve the latest ripgrep release version"
    fi

    printf '%s\n' "$latest"
}

path_contains() {
    case ":${PATH:-}:" in
        *:"$1":*) return 0 ;;
        *) return 1 ;;
    esac
}

can_install_to_dir() {
    dir="$1"

    if [ -d "$dir" ]; then
        [ -w "$dir" ]
        return
    fi

    parent="$(dirname "$dir")"
    while [ ! -d "$parent" ]; do
        next_parent="$(dirname "$parent")"
        if [ "$next_parent" = "$parent" ]; then
            return 1
        fi
        parent="$next_parent"
    done

    [ -w "$parent" ]
}

choose_shell_profile() {
    shell_name="${SHELL##*/}"
    xdg_config_home="${XDG_CONFIG_HOME:-$HOME/.config}"

    case "$shell_name" in
        zsh)
            for candidate in "${ZDOTDIR:-$HOME}/.zshrc" "${ZDOTDIR:-$HOME}/.zshenv" "$xdg_config_home/zsh/.zshrc" "$xdg_config_home/zsh/.zshenv"; do
                if [ -f "$candidate" ]; then
                    printf '%s\n' "$candidate"
                    return
                fi
            done
            printf '%s\n' "${ZDOTDIR:-$HOME}/.zshrc"
            ;;
        bash)
            for candidate in "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.profile" "$xdg_config_home/bash/.bashrc" "$xdg_config_home/bash/.bash_profile"; do
                if [ -f "$candidate" ]; then
                    printf '%s\n' "$candidate"
                    return
                fi
            done
            printf '%s\n' "$HOME/.bashrc"
            ;;
        sh|dash|ksh|ash)
            if [ -f "$HOME/.profile" ]; then
                printf '%s\n' "$HOME/.profile"
            else
                printf '%s\n' "$HOME/.profile"
            fi
            ;;
        fish)
            if [ -f "$HOME/.config/fish/config.fish" ]; then
                printf '%s\n' "$HOME/.config/fish/config.fish"
            else
                printf '%s\n' "$HOME/.config/fish/config.fish"
            fi
            ;;
        *)
            printf '\n'
            ;;
    esac
}

ensure_path_in_profile() {
    target_install_dir="$1"
    profile="$2"
    shell_name="${SHELL##*/}"

    if [ -z "$profile" ]; then
        return 1
    fi

    if [ -e "$profile" ] && [ ! -w "$profile" ]; then
        return 1
    fi

    mkdir -p "$(dirname "$profile")"

    case "$shell_name" in
        fish)
            path_line="fish_add_path $target_install_dir"
            ;;
        *)
            path_line="export PATH=\"$target_install_dir:\$PATH\""
            ;;
    esac

    if [ -f "$profile" ] && grep -F "$path_line" "$profile" >/dev/null 2>&1; then
        return 0
    fi

    {
        printf '\n'
        printf '# added by devo installer\n'
        printf '%s\n' "$path_line"
    } >> "$profile"
}

print_path_hint() {
    target_install_dir="$1"

    if path_contains "$target_install_dir"; then
        return
    fi

    if [ "$no_modify_path" = "true" ]; then
        print_message warning "Add ${target_install_dir} to your PATH to run devo from any terminal:"
        if [ "${SHELL##*/}" = "fish" ]; then
            print_message info "  fish_add_path ${target_install_dir}"
        else
            print_message info "  export PATH=\"${target_install_dir}:\$PATH\""
        fi
        return
    fi

    profile="$(choose_shell_profile)"
    if ensure_path_in_profile "$target_install_dir" "$profile"; then
        print_message info "${MUTED}Updated PATH in ${NC}${profile}"
        print_message info "${MUTED}Open a new terminal or run:${NC}"
        print_message info "  . \"$profile\""
        return
    fi

    print_message warning "Couldn't update your shell profile automatically."
    if [ "${SHELL##*/}" = "fish" ]; then
        print_message info "Add this line to your shell config:"
        print_message info "  fish_add_path ${target_install_dir}"
    else
        print_message info "Add this line to your shell config:"
        print_message info "  export PATH=\"${target_install_dir}:\$PATH\""
    fi
}

existing_devo_path() {
    if [ -x "${install_dir}/${APP}" ]; then
        printf '%s\n' "${install_dir}/${APP}"
        return 0
    fi

    command -v "$APP" 2>/dev/null || return 1
}

normalize_devo_version_output() {
    raw_version="$1"

    for part in $raw_version; do
        case "$part" in
            v[0-9]*.[0-9]*.[0-9]*)
                printf '%s\n' "$part"
                return
                ;;
            [0-9]*.[0-9]*.[0-9]*)
                printf 'v%s\n' "$part"
                return
                ;;
        esac
    done

    if [ -n "$raw_version" ]; then
        printf '%s\n' "$raw_version"
    else
        printf 'unknown\n'
    fi
}

installed_devo_version() {
    installed_path="$1"
    raw_version="$("$installed_path" --version 2>/dev/null || printf '')"
    normalize_devo_version_output "$raw_version"
}

print_version_transition() {
    target_version="$1"
    installed_path="$(existing_devo_path || true)"

    if [ -n "$installed_path" ]; then
        current_version="$(installed_devo_version "$installed_path")"
    else
        current_version="not installed"
    fi

    print_message info "${MUTED}Version: ${NC}${current_version}${MUTED} -> ${NC}${target_version}"
}

check_version() {
    expected_version="$1"
    if [ -x "$install_dir/devo" ] && bundle_complete "$install_dir"; then
        if [ "$(installed_devo_version "$install_dir/devo")" = "$expected_version" ]; then
            print_message info "Devo $expected_version and its runtimes are already installed."
            skip_app_install="true"
        fi
    fi
}

find_extracted_binary() {
    search_dir="$1"
    binary_name="${2:-$APP}"
    found_binary="$(find "$search_dir" -name "$binary_name" -type f | sed -n '1p')"

    if [ -z "$found_binary" ]; then
        die "Failed to locate the ${binary_name} binary inside the downloaded archive"
    fi

    printf '%s\n' "$found_binary"
}

find_extracted_optional_binary() {
    search_dir="$1"
    binary_name="$2"
    find "$search_dir" -name "$binary_name" -type f | sed -n '1p'
}

find_extracted_rg_binary() {
    search_dir="$1"
    found_binary="$(find "$search_dir" -name "$RG_APP" -type f | sed -n '1p')"

    if [ -z "$found_binary" ]; then
        die "Failed to locate the ${RG_APP} binary inside the downloaded ripgrep archive"
    fi

    printf '%s\n' "$found_binary"
}

bundle_complete() {
    for component in runtime/manifest.json runtime/python-site/dill/__init__.py runtime/python-site/rlm/repl.py tui/src/index.js rg devo; do
        [ -f "$1/$component" ] || return 1
    done
    for kind in node python; do
        runtime_root="$1/runtime/$kind"
        if [ -f "$1/runtime/$kind.path" ]; then
            runtime_root="$(cat "$1/runtime/$kind.path")"
            case "$runtime_root" in /*) ;; *) return 1 ;; esac
        fi
        case "$kind" in node) binary="bin/node" ;; python) binary="bin/python3" ;; esac
        [ -x "$runtime_root/$binary" ] || return 1
        if [ -f "$1/runtime/$kind.path" ]; then
            cache_entry="$(dirname "$(dirname "$runtime_root")")"
            [ -f "$cache_entry/.complete" ] && [ -f "$cache_entry/.binary.sha256" ] || return 1
            [ "$(cat "$cache_entry/.complete")" = "$(basename "$cache_entry")" ] || return 1
            [ "$(sha256_file "$runtime_root/$binary")" = "$(cat "$cache_entry/.binary.sha256")" ] || return 1
        fi
    done
}

# Custom install directories can contain other programs; only swap Devo-owned paths.
install_bundle() (
    bundle_source="$1"
    bundle_complete "$bundle_source" || die "Incomplete Devo archive. Download the complete runtime bundle; the TUI requires its private Node and Python files."
    mkdir -p "$install_dir"
    bundle_stage="$(mktemp -d "$install_dir/.devo-install.XXXXXX")"
    mkdir "$bundle_stage/new" "$bundle_stage/old"
    bundle_changed=""
    bundle_success="false"
    rollback_bundle() {
        if [ "$bundle_success" != "true" ]; then
            for component in $bundle_changed; do
                if [ -e "$install_dir/$component" ]; then rm -r "$install_dir/$component"; fi
                if [ -e "$bundle_stage/old/$component" ]; then mv "$bundle_stage/old/$component" "$install_dir/$component"; fi
            done
        fi
        rm -r "$bundle_stage"
    }
    trap rollback_bundle EXIT
    bundle_components="runtime tui rg"
    bundle_components="$bundle_components devo"
    for component in $bundle_components; do cp -R "$bundle_source/$component" "$bundle_stage/new/$component"; done
    for component in $bundle_components; do
        if [ -e "$install_dir/$component" ]; then mv "$install_dir/$component" "$bundle_stage/old/$component"; fi
        bundle_changed="$component $bundle_changed"
        mv "$bundle_stage/new/$component" "$install_dir/$component"
    done
    bundle_success="true"
)

install_from_binary() {
    source_binary="$1"
    [ -f "$source_binary" ] || die "Binary not found at ${source_binary}"
    source_dir="$(dirname "$source_binary")"
    if [ -f "$source_dir/runtime/manifest.json" ]; then
        install_bundle "$source_dir"
    else
        print_message warning "Installing a development backend only. The terminal UI requires the full release bundle."
        mkdir -p "$install_dir"
        cp "$source_binary" "${install_dir}/${APP}"
        chmod 755 "${install_dir}/${APP}"
    fi
}

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        die "SHA-256 verification requires sha256sum or shasum."
    fi
}

verify_archive_checksum() {
    archive_file="$1"
    checksum_file="$2"
    asset_name="$3"
    expected_checksum="$(awk -v name="$asset_name" '$2 == name { print $1; exit }' "$checksum_file")"
    [ "${#expected_checksum}" -eq 64 ] || die "Missing SHA-256 for $asset_name"
    actual_checksum="$(sha256_file "$archive_file")"
    [ "$expected_checksum" = "$actual_checksum" ] || die "Release archive SHA-256 verification failed: $asset_name"
}

cached_runtime() (
    kind="$1"; digest="$2"; asset="$3"; base_url="${DEVO_RUNTIME_BASE_URL:-$4}"
    base_url="${base_url%/}"
    cache="${DEVO_RUNTIME_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/devo/runtimes}"
    mkdir -p "$cache"
    cache="$(cd "$cache" && pwd -P)"
    destination="$cache/$digest"
    lock="$cache/$digest.lock"
    attempts=0
    while ! mkdir "$lock" 2>/dev/null; do
        attempts=$((attempts + 1))
        [ "$attempts" -lt 300 ] || die "Timed out waiting for runtime cache lock: $lock"
        if [ -f "$lock/pid" ]; then
            owner="$(cat "$lock/pid")"
            case "$owner" in ''|*[!0-9]*) ;; *)
                if ! kill -0 "$owner" 2>/dev/null; then rm -f "$lock/pid"; rmdir "$lock" 2>/dev/null || true; fi ;;
            esac
        fi
        sleep 1
    done
    stage=""
    trap 'if [ -n "$stage" ]; then rm -rf "$stage"; fi; rm -f "$lock/pid"; rmdir "$lock"' EXIT
    trap 'exit 1' INT TERM
    printf '%s\n' "$$" > "$lock/pid"
    case "$kind" in node) executable="bin/node" ;; python) executable="bin/python3" ;; esac
    binary="$destination/runtime/$kind/$executable"
    if [ -f "$destination/.complete" ] && [ "$(cat "$destination/.complete")" = "$digest" ] && [ -x "$binary" ] && [ -f "$destination/.binary.sha256" ]; then
        if [ "$(sha256_file "$binary")" = "$(cat "$destination/.binary.sha256")" ]; then
            printf 'Using cached %s runtime.\n' "$kind" >&2
            printf '%s\n' "$destination/runtime/$kind"
            exit 0
        fi
    fi
    stage="$(mktemp -d "$cache/.stage.XXXXXX")"
    printf 'Downloading %s runtime ...\n' "$kind" >&2
    curl -fL --retry 3 --connect-timeout 20 "$base_url/$asset" -o "$stage/pack.tar.gz"
    [ "$(sha256_file "$stage/pack.tar.gz")" = "$digest" ] || die "Runtime SHA-256 verification failed: $asset"
    tar -xzf "$stage/pack.tar.gz" -C "$stage"
    rm "$stage/pack.tar.gz"
    [ -x "$stage/runtime/$kind/$executable" ] || die "Incomplete $kind runtime pack"
    sha256_file "$stage/runtime/$kind/$executable" > "$stage/.binary.sha256"
    printf '%s\n' "$digest" > "$stage/.complete"
    if [ -e "$destination" ]; then mv "$destination" "$(mktemp -d "$cache/.damaged.XXXXXX")/previous"; fi
    mv "$stage" "$destination"
    stage=""
    printf '%s\n' "$destination/runtime/$kind"
)

install_online_packs() (
    index="$1"; base_url="$2"; work="$3"
    awk 'NR==1 {if ($0!="devo-install-v1") exit 1; next}
         NR>1 {if (NF!=3 || $1!=(NR==2?"app":NR==3?"node":"python") || length($2)!=64 || $2!~/^[a-f0-9]+$/ || $3!~/^[a-zA-Z0-9_.-]+[.]tar[.]gz$/) exit 1}
         END {if (NR!=4) exit 1}' "$index" || die "Invalid runtime installation index"
    # Download independent runtimes concurrently; collect both results before
    # installing so no partial application becomes visible after a failure.
    read -r _ node_digest node_asset <<EOF
$(sed -n '3p' "$index")
EOF
    read -r _ python_digest python_asset <<EOF
$(sed -n '4p' "$index")
EOF
    cached_runtime node "$node_digest" "$node_asset" "$base_url" > "$work/node-root" & node_pid=$!
    cached_runtime python "$python_digest" "$python_asset" "$base_url" > "$work/python-root" & python_pid=$!
    failed=0
    wait "$node_pid" || failed=1
    wait "$python_pid" || failed=1
    [ "$failed" -eq 0 ] || die "Runtime installation failed; existing application was preserved."
    read -r _ app_digest app_asset <<EOF
$(sed -n '2p' "$index")
EOF
    curl -fL --retry 3 --connect-timeout 20 "$base_url/$app_asset" -o "$work/app.tar.gz"
    [ "$(sha256_file "$work/app.tar.gz")" = "$app_digest" ] || die "App SHA-256 verification failed"
    mkdir "$work/app"
    tar -xzf "$work/app.tar.gz" -C "$work/app"
    for kind in node python; do cp "$work/$kind-root" "$work/app/runtime/$kind.path"; done
    install_bundle "$work/app"
)

download_and_install() {
    target="$1"
    version_tag="$2"

    require_command curl "Error: 'curl' is required but not installed."
    require_command tar "Error: 'tar' is required but not installed."
    require_command find "Error: 'find' is required but not installed."

    release_origin="${DEVO_RELEASE_BASE_URL:-https://github.com/${REPO}/releases/download}"
    base_url="${release_origin%/}/${version_tag}"

    print_message info ""
    print_message info "${MUTED}Installing ${NC}${APP} ${MUTED}version: ${NC}${version_tag}"
    print_message info "${MUTED}Target: ${NC}${target}"

    tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/${APP}-install.XXXXXX")"
    trap 'rm -rf "$tmp_dir"' EXIT INT TERM

    curl -fsSL --retry 3 "$base_url/SHA256SUMS.txt" -o "$tmp_dir/SHA256SUMS.txt"
    # Public Linux names omit Rust's vendor field. Older releases retain it.
    asset_target="$(printf '%s\n' "$target" | sed 's/-unknown-linux-/-linux-/')"
    if [ "$asset_target" != "$target" ] && ! awk \
        -v index_name="devo-tui-${version_tag}-${asset_target}.install.txt" \
        -v archive="devo-tui-${version_tag}-${asset_target}.tar.gz" \
        '$2 == index_name || $2 == archive { found = 1 } END { exit !found }' "$tmp_dir/SHA256SUMS.txt"; then
        asset_target="$target"
    fi
    archive_name="${APP}-tui-${version_tag}-${asset_target}.tar.gz"
    index_name="devo-tui-${version_tag}-${asset_target}.install.txt"
    status="$(curl -sSL --retry 3 --connect-timeout 20 -o "$tmp_dir/$index_name" -w '%{http_code}' "$base_url/$index_name")"
    case "$status" in
        200)
            verify_archive_checksum "$tmp_dir/$index_name" "$tmp_dir/SHA256SUMS.txt" "$index_name"
            install_online_packs "$tmp_dir/$index_name" "$base_url" "$tmp_dir"
            ;;
        404)
            # Compatibility with releases created before runtime packs.
            curl -fL --retry 3 --progress-bar "$base_url/$archive_name" -o "$tmp_dir/$archive_name"
            verify_archive_checksum "$tmp_dir/$archive_name" "$tmp_dir/SHA256SUMS.txt" "$archive_name"
            tar -xzf "$tmp_dir/$archive_name" -C "$tmp_dir"
            extracted_binary="$(find_extracted_binary "$tmp_dir")"
            install_bundle "$(dirname "$extracted_binary")"
            ;;
        *) die "Failed to fetch runtime installation index (HTTP $status)" ;;
    esac

    rm -rf "$tmp_dir"
    trap - EXIT INT TERM
}

install_ripgrep_sidecar() {
    if [ "${DEVO_SKIP_RG_INSTALL:-}" = "1" ]; then
        print_message warning "Skipping ripgrep sidecar install because DEVO_SKIP_RG_INSTALL=1."
        return
    fi

    if [ -x "${install_dir}/${RG_APP}" ]; then
        print_message info "${MUTED}ripgrep sidecar is already installed at ${NC}${install_dir}/${RG_APP}"
        return
    fi

    require_command curl "Error: 'curl' is required but not installed."
    require_command tar "Error: 'tar' is required but not installed."
    require_command find "Error: 'find' is required but not installed."

    rg_target="$(detect_rg_target)"
    rg_version="$(resolve_latest_ripgrep_version)"
    archive_name="ripgrep-${rg_version}-${rg_target}.tar.gz"
    archive_url="https://github.com/${RG_REPO}/releases/download/${rg_version}/${archive_name}"

    print_message info ""
    print_message info "${MUTED}Installing ripgrep sidecar ${NC}${rg_version}${MUTED} target: ${NC}${rg_target}"

    tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/${APP}-rg-install.XXXXXX")"
    trap 'rm -rf "$tmp_dir"' EXIT INT TERM

    curl -fL --progress-bar "$archive_url" -o "$tmp_dir/$archive_name"
    tar -xzf "$tmp_dir/$archive_name" -C "$tmp_dir"

    extracted_binary="$(find_extracted_rg_binary "$tmp_dir")"

    mkdir -p "$install_dir"
    install -m 755 "$extracted_binary" "${install_dir}/${RG_APP}"

    rm -rf "$tmp_dir"
    trap - EXIT INT TERM
}



installer_asset_dir() {
    dir_name="$(dirname "$0")"
    if ! asset_dir="$(cd "$dir_name" && pwd -P)"; then
        die "Failed to resolve installer asset directory from ${dir_name}"
    fi
    printf '%s\n' "$asset_dir"
}

find_offline_file() {
    asset_dir="$1"
    pattern="$2"

    for candidate in "$asset_dir"/$pattern; do
        if [ -f "$candidate" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done

    return 1
}

install_offline_devo() {
    asset_dir="$1"
    target="$2"

    if [ -n "$binary_path" ]; then
        print_message info "${MUTED}Installing ${NC}${APP} ${MUTED}from local binary: ${NC}${binary_path}"
        install_from_binary "$binary_path"
        return
    fi

    if [ -f "${asset_dir}/${APP}" ]; then
        print_message info "${MUTED}Installing ${NC}${APP} ${MUTED}from local binary: ${NC}${asset_dir}/${APP}"
        install_from_binary "${asset_dir}/${APP}"
        return
    fi

    asset_target="$(printf '%s\n' "$target" | sed 's/-unknown-linux-/-linux-/')"
    archive_path="$(find_offline_file "$asset_dir" "${APP}-tui-v*-${asset_target}.tar.gz" || true)"
    if [ -z "$archive_path" ] && [ "$asset_target" != "$target" ]; then
        archive_path="$(find_offline_file "$asset_dir" "${APP}-tui-v*-${target}.tar.gz" || true)"
    fi
    if [ -z "$archive_path" ]; then
        die "Offline devo asset not found. Place ${APP}-tui-*-${asset_target}.tar.gz or ${APP} next to install.sh."
    fi

    require_command tar "Error: 'tar' is required but not installed."
    require_command find "Error: 'find' is required but not installed."

    print_message info "${MUTED}Installing ${NC}${APP} ${MUTED}from offline archive: ${NC}${archive_path}"

    tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/${APP}-offline-install.XXXXXX")"
    trap 'rm -rf "$tmp_dir"' EXIT INT TERM

    if [ -f "$asset_dir/SHA256SUMS.txt" ]; then
        verify_archive_checksum "$archive_path" "$asset_dir/SHA256SUMS.txt" "$(basename "$archive_path")"
    fi
    tar -xzf "$archive_path" -C "$tmp_dir"
    extracted_binary="$(find_extracted_binary "$tmp_dir")"
    install_bundle "$(dirname "$extracted_binary")"

    rm -rf "$tmp_dir"
    trap - EXIT INT TERM
}

install_offline_ripgrep_sidecar() {
    asset_dir="$1"

    if [ "${DEVO_SKIP_RG_INSTALL:-}" = "1" ]; then
        print_message warning "Skipping ripgrep sidecar install because DEVO_SKIP_RG_INSTALL=1."
        return
    fi

    if [ -x "${install_dir}/${RG_APP}" ]; then
        print_message info "${MUTED}ripgrep sidecar is already installed at ${NC}${install_dir}/${RG_APP}"
        return
    fi

    if [ -f "${asset_dir}/${RG_APP}" ]; then
        print_message info "${MUTED}Installing ripgrep sidecar from ${NC}${asset_dir}/${RG_APP}"
        mkdir -p "$install_dir"
        install -m 755 "${asset_dir}/${RG_APP}" "${install_dir}/${RG_APP}"
        return
    fi

    rg_target="$(detect_rg_target)"
    archive_path="$(find_offline_file "$asset_dir" "ripgrep-*-${rg_target}.tar.gz" || true)"
    if [ -z "$archive_path" ]; then
        die "Offline ripgrep asset not found. Place ripgrep-*-${rg_target}.tar.gz or ${RG_APP} next to install.sh."
    fi

    require_command tar "Error: 'tar' is required but not installed."
    require_command find "Error: 'find' is required but not installed."

    print_message info "${MUTED}Installing ripgrep sidecar from offline archive: ${NC}${archive_path}"

    tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/${APP}-offline-rg-install.XXXXXX")"
    trap 'rm -rf "$tmp_dir"' EXIT INT TERM

    tar -xzf "$archive_path" -C "$tmp_dir"
    extracted_binary="$(find_extracted_rg_binary "$tmp_dir")"

    mkdir -p "$install_dir"
    install -m 755 "$extracted_binary" "${install_dir}/${RG_APP}"

    rm -rf "$tmp_dir"
    trap - EXIT INT TERM
}



print_banner() {
    printf '\n'
    printf '%b%s%b\n' "$MUTED" "██████╗  ███████╗██╗   ██╗ ██████╗" "$NC"
    printf '%b%s%b\n' "$MUTED" "██╔══██╗ ██╔════╝██║   ██║██╔═══██╗" "$NC"
    printf '%b%s%b\n' "$MUTED" "██║  ██║ █████╗  ██║   ██║██║   ██║" "$NC"
    printf '%b%s%b\n' "$MUTED" "██║  ██║ ██╔══╝  ╚██╗ ██╔╝██║   ██║" "$NC"
    printf '%b%s%b\n' "$MUTED" "██████╔╝ ███████╗ ╚████╔╝ ╚██████╔╝" "$NC"
    printf '%b%s%b\n' "$MUTED" "╚═════╝  ╚══════╝  ╚═══╝   ╚═════╝" "$NC"
    printf '\n'
}

main() {
    print_banner


    if [ "$offline_mode" = "true" ]; then
        asset_dir="$(installer_asset_dir)"
        print_message info "${MUTED}Offline asset directory: ${NC}${asset_dir}"
        target="$(detect_target)"
        install_offline_devo "$asset_dir" "$target"
        install_offline_ripgrep_sidecar "$asset_dir"
    elif [ -n "$binary_path" ]; then
        print_message info ""
        print_message info "${MUTED}Installing ${NC}${APP} ${MUTED}from local binary: ${NC}${binary_path}"
        install_from_binary "$binary_path"
        install_ripgrep_sidecar
    else
        target="$(detect_target)"

        if [ -z "$requested_version" ]; then
            version_tag="$(resolve_latest_version)"
        else
            version_tag="$(normalize_version "$requested_version")"
        fi

        print_version_transition "$version_tag"
        check_version "$version_tag"
        if [ "$skip_app_install" != "true" ]; then
            download_and_install "$target" "$version_tag"
        fi

        install_ripgrep_sidecar
    fi

    print_path_hint "$install_dir"

    if [ -n "${GITHUB_ACTIONS:-}" ] && [ "$GITHUB_ACTIONS" = "true" ] && [ -n "${GITHUB_PATH:-}" ]; then
        printf '%s\n' "$install_dir" >> "$GITHUB_PATH"
        print_message info "${MUTED}Added ${NC}${install_dir}${MUTED} to \$GITHUB_PATH${NC}"
    fi

    print_message info "${MUTED}${APP} is ready.${NC}"
    print_message info ""
    print_message info "  cd <project>    ${MUTED}# open your workspace${NC}"
    print_message info "  devo            ${MUTED}# first-run setup${NC}"
    print_message info ""
    print_message info "${MUTED}Docs: ${NC}https://github.com/${REPO}#readme"
}

if ! can_install_to_dir "$install_dir"; then
    die "Install directory is not writable or cannot be created: ${install_dir}"
fi

main
