FROM ghcr.io/blockscout/blockscout@sha256:7659f168e4e2f6b73dd559ae5278fe96ba67bc2905ea01b57a814c68adf5a9dc
# The bundled Mint adapter opens a connection per request. Catch-up can exhaust
# the local ephemeral port range. Hackney is already included in this release.
USER root
RUN printf '\nconfig :tesla, adapter: Tesla.Adapter.Hackney\n' >> /app/releases/9.0.2/runtime.exs
USER blockscout:blockscout
