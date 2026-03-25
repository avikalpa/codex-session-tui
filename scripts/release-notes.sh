#!/usr/bin/env sh
set -eu

if [ $# -ne 1 ]; then
  echo "usage: $0 <version-or-tag>" >&2
  exit 1
fi

version="$1"
version=${version#v}

extract_section() {
  section="$1"
  awk -v section="$section" '
    function header_matches(line, section) {
      return line ~ ("^## \\[" section "\\]") || line ~ ("^## " section "([[:space:]]+-|$)")
    }
    BEGIN {
      in_section = 0
      found = 0
    }
    header_matches($0, section) {
      in_section = 1
      found = 1
    }
    in_section {
      if (NR > 1 && $0 ~ /^## / && !header_matches($0, section)) {
        exit
      }
      print
    }
    END {
      if (!found) {
        exit 2
      }
    }
  ' CHANGELOG.md
}

if extract_section "$version"; then
  exit 0
fi

extract_section "Unreleased"
