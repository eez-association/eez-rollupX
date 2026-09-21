# Disposable test image only: production monitoring images remain unchanged.
FROM grafana/alloy:v1.19.2@sha256:b8ec653c44235fbe910879145dac3597d66b0aaecf60bcbbe82580767771a839 AS alloy
FROM python:3.13-slim-trixie@sha256:9d2e5553305c7c7b0097999bb17187c69b921ccd6bc9d40e4bb5ebe652c00285
RUN apt-get update && apt-get install -y --no-install-recommends fuse2fs=1.47.2-3+b12 e2fsprogs=1.47.2-3+b12 fuse3=3.17.2-3 && rm -rf /var/lib/apt/lists/*
COPY --from=alloy /bin/alloy /usr/local/bin/alloy
