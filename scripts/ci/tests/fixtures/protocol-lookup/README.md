# Captured protocol lookup logs

Scenario logs from the homelab Kurtosis run (trailing whitespace normalized):
https://github.com/eduadiez/rollupX/actions/runs/34220104933

- PR head: `6cd52b12316b1ed1fbb0ff166579888c27dd64f1`
- Tested merge revision: `1f6bf42cf398a5249aff7bdcf74b147fe5307e76`
- Protocol revision: `3c0cdf4955941a5c23639346932b55b33babebbd`
- Artifact paths: `protocol-e2e/topLevelStaticCounter.log` and
  `protocol-e2e/revertCounter.log`

The collector tests substitute transaction hashes and contract addresses to
match mocked RPC responses, but retain upstream's real verification messages.
Static lookups use a static-table marker. The reverting lookup does not emit a
rolling-hash replay marker; the collector must verify that hash independently.
