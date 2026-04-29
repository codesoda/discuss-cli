#!/bin/sh
set -eu

BINARY_NAME="discuss"
REPO_URL="https://github.com/codesoda/discuss-cli"
RAW_REPO_URL="https://raw.githubusercontent.com/codesoda/discuss-cli"
INSTALL_DIR="${HOME}/.discuss/bin"
LINK_DIR="${HOME}/.local/bin"
SKILL_INSTALL_DIR="${HOME}/.discuss/skills/discuss"
INSTALLED_BINARY="${INSTALL_DIR}/${BINARY_NAME}"
LINK_BINARY="${LINK_DIR}/${BINARY_NAME}"
TMP_DIR=""

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  BOLD="$(printf '\033[1m')"
  RED="$(printf '\033[31m')"
  YELLOW="$(printf '\033[33m')"
  RESET="$(printf '\033[0m')"
else
  BOLD=""
  RED=""
  YELLOW=""
  RESET=""
fi

cleanup() {
  if [ -n "${TMP_DIR}" ] && [ -d "${TMP_DIR}" ]; then
    rm -rf "${TMP_DIR}"
  fi
}

trap cleanup EXIT HUP INT TERM

status() {
  printf '%s%s%s\n' "${BOLD}" "$*" "${RESET}"
}

warn() {
  printf '%swarning:%s %s\n' "${YELLOW}" "${RESET}" "$*" >&2
}

die() {
  printf '%serror:%s %s\n' "${RED}" "${RESET}" "$*" >&2
  exit 1
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "required command '$1' was not found"
}

script_dir() {
  if [ -f "$0" ]; then
    (CDPATH=; cd "$(dirname "$0")" && pwd -P)
  else
    pwd -P
  fi
}

detect_target() {
  OS="$(uname -s)"
  ARCH="$(uname -m)"

  case "${OS}:${ARCH}" in
    Darwin:arm64 | Darwin:aarch64)
      printf 'aarch64-apple-darwin'
      ;;
    Darwin:x86_64)
      printf 'x86_64-apple-darwin'
      ;;
    Linux:x86_64 | Linux:amd64)
      printf 'x86_64-unknown-linux-gnu'
      ;;
    *)
      die "unsupported platform ${OS}/${ARCH}; supported targets are aarch64-apple-darwin, x86_64-apple-darwin, and x86_64-unknown-linux-gnu"
      ;;
  esac
}

latest_release_tag() {
  LATEST_URL="${REPO_URL}/releases/latest"
  EFFECTIVE_URL="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "${LATEST_URL}")" \
    || die "failed to resolve latest release from ${LATEST_URL}"
  TAG="${EFFECTIVE_URL##*/}"

  case "${TAG}" in
    v[0-9]*)
      printf '%s' "${TAG}"
      ;;
    *)
      die "could not determine latest release tag from ${EFFECTIVE_URL}"
      ;;
  esac
}

download_file() {
  URL="$1"
  DEST="$2"
  HTTP_STATUS="$(curl -L -sS --connect-timeout 10 --retry 2 -w '%{http_code}' -o "${DEST}" "${URL}")" \
    || die "failed to download ${URL}"

  case "${HTTP_STATUS}" in
    2??)
      ;;
    *)
      rm -f "${DEST}"
      die "failed to download ${URL}: HTTP ${HTTP_STATUS}"
      ;;
  esac
}

install_binary() {
  SOURCE="$1"
  mkdir -p "${INSTALL_DIR}" "${LINK_DIR}" \
    || die "failed to create install directories ${INSTALL_DIR} and ${LINK_DIR}"
  install -m 0755 "${SOURCE}" "${INSTALLED_BINARY}" \
    || die "failed to install ${SOURCE} to ${INSTALLED_BINARY}"
  ln -sfn "${INSTALLED_BINARY}" "${LINK_BINARY}" \
    || die "failed to link ${LINK_BINARY} to ${INSTALLED_BINARY}"

  if [ "$(uname -s)" = "Darwin" ]; then
    if command -v xattr >/dev/null 2>&1; then
      xattr -d com.apple.quarantine "${INSTALLED_BINARY}" 2>/dev/null || true
    else
      warn "xattr is unavailable; if macOS blocks ${BINARY_NAME}, run: xattr -d com.apple.quarantine ${INSTALLED_BINARY}"
    fi
  fi
}

run_source_install() {
  SCRIPT_DIR="$1"
  SOURCE_BINARY="${SCRIPT_DIR}/target/release/${BINARY_NAME}"

  require_cmd cargo
  require_cmd install

  status "Building ${BINARY_NAME} with warnings denied..."
  (cd "${SCRIPT_DIR}" && RUSTFLAGS="-D warnings" cargo build --release) \
    || die "cargo build failed"

  [ -x "${SOURCE_BINARY}" ] || die "expected built binary at ${SOURCE_BINARY}"
  install_binary "${SOURCE_BINARY}"
}

run_download_install() {
  require_cmd curl
  require_cmd tar
  require_cmd uname
  require_cmd awk
  require_cmd install
  require_cmd mktemp

  TARGET="$(detect_target)"
  TAG="$(latest_release_tag)"
  ASSET="discuss-${TAG}-${TARGET}.tar.gz"
  URL="${REPO_URL}/releases/download/${TAG}/${ASSET}"

  TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/discuss-install.XXXXXX")" \
    || die "failed to create a temporary directory"
  ARCHIVE="${TMP_DIR}/${ASSET}"

  status "Downloading ${ASSET}..."
  download_file "${URL}" "${ARCHIVE}"

  BINARY_PATH="$(tar -tzf "${ARCHIVE}" | awk -v name="${BINARY_NAME}" '
    {
      n = split($0, parts, "/")
      if (parts[n] == name) {
        print $0
        exit
      }
    }
  ')"
  [ -n "${BINARY_PATH}" ] || die "archive ${ASSET} did not contain a ${BINARY_NAME} binary"

  tar -xzf "${ARCHIVE}" -C "${TMP_DIR}" "${BINARY_PATH}" \
    || die "failed to extract ${BINARY_PATH} from ${ASSET}"
  [ -f "${TMP_DIR}/${BINARY_PATH}" ] \
    || die "extracted archive did not produce ${TMP_DIR}/${BINARY_PATH}"

  install_binary "${TMP_DIR}/${BINARY_PATH}"
}

path_contains_link_dir() {
  case ":${PATH:-}:" in
    *":${LINK_DIR}:"*) return 0 ;;
    *) return 1 ;;
  esac
}

print_path_hint() {
  SHELL_NAME="$(basename "${SHELL:-sh}")"

  case "${SHELL_NAME}" in
    fish)
      RC_FILE="${HOME}/.config/fish/config.fish"
      warn "${LINK_DIR} is not on PATH; add this line to ${RC_FILE}:"
      printf '  fish_add_path %s\n' "${LINK_DIR}" >&2
      ;;
    zsh)
      RC_FILE="${HOME}/.zshrc"
      warn "${LINK_DIR} is not on PATH; add this line to ${RC_FILE}:"
      printf '  export PATH="%s:%s"\n' "${LINK_DIR}" "\$PATH" >&2
      ;;
    bash)
      if [ "$(uname -s)" = "Darwin" ]; then
        RC_FILE="${HOME}/.bash_profile"
      else
        RC_FILE="${HOME}/.bashrc"
      fi
      warn "${LINK_DIR} is not on PATH; add this line to ${RC_FILE}:"
      printf '  export PATH="%s:%s"\n' "${LINK_DIR}" "\$PATH" >&2
      ;;
    *)
      RC_FILE="${HOME}/.profile"
      warn "${LINK_DIR} is not on PATH; add this line to ${RC_FILE}:"
      printf '  export PATH="%s:%s"\n' "${LINK_DIR}" "\$PATH" >&2
      ;;
  esac
}

install_skill_symlinks() {
  SKILL_SOURCE="$1"

  if [ ! -d "${SKILL_SOURCE}" ]; then
    warn "skill source not found at ${SKILL_SOURCE}; skipping skill install"
    return 0
  fi

  for AGENT_ROOT in "${HOME}/.claude" "${HOME}/.codex" "${HOME}/.agents"; do
    [ -d "${AGENT_ROOT}" ] || continue

    SKILLS_DIR="${AGENT_ROOT}/skills"
    mkdir -p "${SKILLS_DIR}" || {
      warn "failed to create ${SKILLS_DIR}; skipping skill link"
      continue
    }

    TARGET="${SKILLS_DIR}/discuss"
    if [ -L "${TARGET}" ]; then
      rm "${TARGET}"
    elif [ -e "${TARGET}" ]; then
      warn "${TARGET} exists and is not a symlink; skipping"
      continue
    fi

    ln -s "${SKILL_SOURCE}" "${TARGET}" || {
      warn "failed to link ${TARGET} -> ${SKILL_SOURCE}"
      continue
    }
    status "Linked skill ${TARGET} -> ${SKILL_SOURCE}"
  done
}

fetch_to() {
  URL="$1"
  DEST="$2"
  HTTP_STATUS="$(curl -L -sS --connect-timeout 10 --retry 2 -w '%{http_code}' -o "${DEST}" "${URL}" 2>/dev/null)" || HTTP_STATUS="000"
  case "${HTTP_STATUS}" in
    2??) return 0 ;;
    *)
      rm -f "${DEST}"
      printf '%s' "${HTTP_STATUS}"
      return 1
      ;;
  esac
}

install_skill_from_download() {
  REF="$1"
  STAGING="${TMP_DIR}/skill-staging/discuss"
  mkdir -p "${STAGING}" || {
    warn "failed to stage skill download; skipping skill install"
    return 0
  }

  RAW_BASE="${RAW_REPO_URL}/${REF}/skills/discuss"
  MANIFEST_DEST="${STAGING}/manifest.txt"

  status "Fetching skill manifest from ${RAW_BASE}/manifest.txt"
  if ! HTTP_STATUS="$(fetch_to "${RAW_BASE}/manifest.txt" "${MANIFEST_DEST}")"; then
    warn "failed to fetch skill manifest (HTTP ${HTTP_STATUS}); skipping skill install"
    return 0
  fi

  while IFS= read -r FILE_PATH || [ -n "${FILE_PATH}" ]; do
    # Strip CR (in case of CRLF) and surrounding whitespace
    FILE_PATH="$(printf '%s' "${FILE_PATH}" | tr -d '\r' | awk '{$1=$1};1')"
    case "${FILE_PATH}" in
      ''|'#'*) continue ;;
    esac
    DEST_DIR="$(dirname "${STAGING}/${FILE_PATH}")"
    mkdir -p "${DEST_DIR}" || {
      warn "failed to create ${DEST_DIR}; skipping skill install"
      return 0
    }
    if ! HTTP_STATUS="$(fetch_to "${RAW_BASE}/${FILE_PATH}" "${STAGING}/${FILE_PATH}")"; then
      warn "failed to fetch skill file ${FILE_PATH} (HTTP ${HTTP_STATUS}); skipping skill install"
      return 0
    fi
  done < "${MANIFEST_DEST}"

  mkdir -p "${HOME}/.discuss/skills" || {
    warn "failed to create ${HOME}/.discuss/skills; skipping skill install"
    return 0
  }
  rm -rf "${SKILL_INSTALL_DIR}"
  cp -R "${STAGING}" "${SKILL_INSTALL_DIR}" || {
    warn "failed to install skill to ${SKILL_INSTALL_DIR}"
    return 0
  }
  install_skill_symlinks "${SKILL_INSTALL_DIR}"
}

verify_install() {
  status "Installed ${BINARY_NAME} to ${INSTALLED_BINARY}"
  status "Linked ${LINK_BINARY} to ${INSTALLED_BINARY}"
  "${LINK_BINARY}" --version || die "installed binary failed to run from ${LINK_BINARY}"

  if path_contains_link_dir; then
    RESOLVED="$(command -v "${BINARY_NAME}" || true)"
    if [ -n "${RESOLVED}" ]; then
      "${BINARY_NAME}" --version || die "${BINARY_NAME} failed to run from PATH"
      if [ "${RESOLVED}" != "${LINK_BINARY}" ]; then
        warn "${BINARY_NAME} resolves to ${RESOLVED}; move ${LINK_DIR} earlier in PATH to use ${LINK_BINARY}"
      fi
    else
      print_path_hint
    fi
  else
    print_path_hint
  fi
}

SCRIPT_DIR="$(script_dir)" || die "failed to determine script directory"

if [ -f "${SCRIPT_DIR}/install.sh" ] && [ -f "${SCRIPT_DIR}/Cargo.toml" ]; then
  run_source_install "${SCRIPT_DIR}"
  install_skill_symlinks "${SCRIPT_DIR}/skills/discuss"
else
  run_download_install
  install_skill_from_download "${TAG}"
fi

verify_install
