# shellcheck shell=bash
# Shared by the Bash fixture capture scripts. Hash stdin so the output
# doesn't depend on how the hash utility escapes the filename.
fixture_sha256() {
  local digest
  if command -v shasum >/dev/null 2>&1; then
    digest=$(shasum -a 256 < "$1") || return 1
  elif command -v sha256sum >/dev/null 2>&1; then
    digest=$(sha256sum < "$1") || return 1
  else
    printf '%s\n' 'fixture capture requires shasum or sha256sum' >&2
    return 1
  fi
  printf '%s\n' "${digest%% *}"
}
