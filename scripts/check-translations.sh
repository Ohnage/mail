#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
command -v slint-tr-extractor >/dev/null || {
  echo "slint-tr-extractor 1.18.1 is required" >&2
  exit 1
}
command -v msgmerge >/dev/null || {
  echo "GNU gettext is required" >&2
  exit 1
}

work_dir=$(mktemp -d)
trap 'rm -rf -- "$work_dir"' EXIT
cd "$repo_root"

# Preview and screenshot scripts use temporary hidden .slint files.
find ui -type f -name '*.slint' ! -name '.*' -print0 \
  | sort -z \
  | xargs -0 slint-tr-extractor --no-default-translation-context \
      --package-name flectar-mail --package-version 0.1.0 \
      -o "$work_dir/flectar-mail.pot"

# Extraction timestamps are intentionally volatile. Fold only PO string
# continuations; preserve every other catalog line for strict comparison.
sed '/^"POT-Creation-Date:/d' lang/flectar-mail.pot \
  | python3 scripts/normalize-po-wrapping.py > "$work_dir/reference.pot"
sed '/^"POT-Creation-Date:/d' "$work_dir/flectar-mail.pot" \
  | python3 scripts/normalize-po-wrapping.py > "$work_dir/generated.pot"
diff -u "$work_dir/reference.pot" "$work_dir/generated.pot"

for language in da de es fr it ja nl ru tr zh_Hans; do
  msgmerge --quiet --no-fuzzy-matching \
    --output-file "$work_dir/flectar-mail-$language.po" \
    "lang/$language/LC_MESSAGES/flectar-mail.po" "$work_dir/flectar-mail.pot"
  sed '/^"POT-Creation-Date:/d' "lang/$language/LC_MESSAGES/flectar-mail.po" \
    | python3 scripts/normalize-po-wrapping.py > "$work_dir/reference-$language.po"
  sed '/^"POT-Creation-Date:/d' "$work_dir/flectar-mail-$language.po" \
    | python3 scripts/normalize-po-wrapping.py > "$work_dir/generated-$language.po"
  diff -u "$work_dir/reference-$language.po" "$work_dir/generated-$language.po"
done

bash scripts/check-ui-message-catalog.sh
