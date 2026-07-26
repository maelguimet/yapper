#!/usr/bin/env bash
# Audit the exact security-critical ML packages.
#
# Narrow reachability exceptions:
# - PYSEC-2026-139 / CVE-2026-4538: unreviewed, local-only PT2 loader report
#   with no fixed release. Yapper never loads PT2 packages.
# - PYSEC-2025-194 / CVE-2025-3000: torch.jit.script crash. Yapper and its
#   pinned Chatterbox source do not invoke torch.jit.script.
#
# Do not broaden these ignores. New advisories must fail this check.
set -euo pipefail

pip-audit \
  --no-deps \
  --disable-pip \
  --ignore-vuln PYSEC-2026-139 \
  --ignore-vuln PYSEC-2025-194 \
  --requirement python/security-lock.txt
