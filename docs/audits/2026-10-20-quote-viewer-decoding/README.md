# Quote viewer annotations and failure summaries

| Field | Value |
| --- | --- |
| Status | Implemented and locally validated; no deployment performed |
| Fork baseline | `eez-association/eez-rollupX` `1ab1e2e3386a8832c7b18224e1eed82fa7c0b074` |
| Protocol gitlink | `abcf9d8ffc4e0ccb5df271fc19efdf9a7fb57774`, unchanged |
| Candidate identity | Baseline plus the non-audit patch SHA-256 `9593fcd365965754d94a1003c33fa925c2a0581d3eec5d4e3880d02d21b21b20` |
| Toolchain | Node v26.7.0, locked Playwright 1.63.0 |
| Chromium SHA-256 | `8c599d43aec53f2460a31ae2f4af6bd863f8258b34ff519564bc5d4726bfaa1e` |
| Contract | [Authorized dashboard extension](../../proposals/quote-call-tracing.md#dashboard-decoding-extension-authorized-2026-10-20) |

The patch digest covers `git diff --binary` for
`docs/configurable-settlement-policy.md`, `docs/proposals/quote-call-tracing.md`
and `tools/quote-trace-viewer` at the baseline, before this record was added.
The work used a clean worktree and preserved the operator's existing root checkout.

## Result

The viewer accepts direct quote responses and decoded text annotations from a
same-origin embedding dashboard through a private MessageChannel. It retains
the opaque sandbox, offline CSP, local file import, immutable raw export, and
explicit execution outcomes. Sequence checks prevent obsolete results or
annotations from replacing newer imports/selections, including pending file reads.

The summary distinguishes failed frames, caught failures and ancestor rollback.
It links the last recorded child without asserting causation. Truncation names
the affected fields and counts omitted calls/runs separately. Embedded controls
collapse after import; selecting a frame no longer collapses its children.
Left/Right keys and the disclosure arrow control expansion.

## Executed checks

| Check | Evidence | Result |
| --- | --- | --- |
| Baseline model suite | Node unit tests | 10 passed |
| Final `npm test --prefix tools/quote-trace-viewer` | Node unit tests | 12 passed, no skips |
| Final browser suite | Chromium, local files and an owned HTTP server | 3 passed, no skips |
| Syntax and whitespace | `node --check` on model/viewer; `git diff --check` | Passed |

Browser cases cover offline hostile-text imports, nested rollback, keyboard
import/expansion, overlapping file reads, private channel import, stale sequences,
late conditional loads, duplicate/oversized annotations and unchanged exports.
Browser execution used the installed full Chromium binary rather than headless
shell, with a short, owned temporary directory. From the repository root:

```bash
EEZ_TEST_TMP=$(mktemp -d artifacts/qv-XXXXXX)
TMPDIR="$PWD/$EEZ_TEST_TMP" node <<'JS'
const chromium = require('./tools/quote-trace-viewer/node_modules/playwright').chromium;
const launch = chromium.launch.bind(chromium);
chromium.launch = options => launch({ ...options, executablePath: chromium.executablePath() });
require('./tools/quote-trace-viewer/tests/browser.test.cjs');
JS
```

Use a short absolute temporary directory if a deeply nested checkout would
exceed Chromium's Unix socket path limit. The executed temporary directories
were in the containing workspace's `artifacts/qv-*` directory.

No Rust, Solidity, RPC calculation or protocol bytes changed, so runtime/process,
signer, replay and settlement suites were not run. The browser checks do not
establish historical bytecode identity, source-line execution or deployment.
The dashboard owns ABI lookup, direct quoting and its separate integration tests.
