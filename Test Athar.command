#!/bin/zsh
# Every automated check: JavaScript syntax, the signal-processing tests, the library tests.
cd "${0:A:h}"
fail=0
for f in athar/static/*.js; do node --check "$f" || fail=1; done
node --test tests/ || fail=1
uv run pytest -q tests || fail=1
[[ $fail == 0 ]] && echo "All checks passed" || echo "Some checks failed"
exit $fail
