# Transfer to eez-association and make public

Target: `eez-association/rollupX`. Ownership and visibility changes are separate
from deploying or restarting nodes.

- [ ] Confirm publication rights and finish the private-content review before
  making source or packages public. Publication authority is still unconfirmed.
- [ ] Transfer the repository while private. Verify the destination name,
  maintainer/team access, Actions settings, secrets/variables and GHCR permissions.
  Update [CODEOWNERS](../../.github/CODEOWNERS) for the intended organization team.
- [ ] Remove this repository's access to homelab runners before making it public,
  including historical workflow reruns. The machines can remain for other private
  work; public PRs use GitHub-hosted `ubuntu-24.04` runners.
- [ ] Once publication is approved, make the repository public and validate
  organization CI and an external fork PR. Neither owner has private Actions
  minutes available, so do local checks until then. Standard public runners are
  [free](https://docs.github.com/en/billing/concepts/product-billing/github-actions);
  retain spending limits and check artifact storage separately.
- [ ] Require review and the **CI Result** check on main. Merge CI fixes and
  promote deployment images only after all required lanes pass. Fork CI must
  use its existing no-publication path without write tokens or repository secrets.
- [ ] Verify the tested organization image digests with the
  [artifact verifier](../../scripts/ci/prepare-runtime-artifacts.sh), then confirm
  anonymous pulls of node, signer and deploy images. Only then update the image
  defaults in [Chiado](../../.env.chiado.example),
  [Ethereum](../../.env.ethereum.example),
  [Compose](../../docker-compose.chiado-node.yml) and the
  [deployment runbook](chiado-deployment.md). Existing deployments retain their
  current image pins and configuration.
- [ ] Verify anonymous cloning with recursive submodules, enable private
  vulnerability reporting and secret protection, and confirm contributor access.

The remaining CI failure still needs investigation; local passing diagnostics
do not replace a successful hosted run. If validation fails, keep fixes in draft
and block image promotion and deployment-reference changes.
