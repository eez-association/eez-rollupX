#!/usr/bin/env bash
# Mint short-lived identities for one disposable enclave. Only the named role
# directories become Kurtosis artifacts. The issuing key stays in this process.
set -euo pipefail
umask 077
out=${1:?output directory}
mkdir -p "$out"
out=$(realpath "$out")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 7 \
    -subj "/CN=fixture-ca-${work##*/}" -keyout ca.key -out ca.pem
issue() {
    local name=$1 purpose=$2 destination=$3
    mkdir -p "$destination"
    openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
        -subj "/CN=$name" -keyout "$destination/key.pem" -out request.pem
    printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=%s\nsubjectAltName=DNS:%s\n' \
        "$purpose" "$name" > extensions
    openssl x509 -req -in request.pem -CA ca.pem -CAkey ca.key -CAcreateserial \
        -days 7 -extfile extensions -out "$destination/cert.pem"
    cp ca.pem "$destination/ca.pem"
}
fingerprint() {
    openssl x509 -in "$1/cert.pem" -outform DER | sha256sum | cut -d' ' -f1
}
mode=${2:?direct, shared or capture}
signer=${3:?signer authority}
issue "${signer%:*}" serverAuth "$out/signer"
if [[ $mode == capture ]]; then
    capture=${4:?capture authority}
    issue composer clientAuth "$out/composer/$capture"
    issue "${capture%:*}" serverAuth "$out/capture-server"
    issue capture clientAuth "$out/capture-client/$signer"
    fingerprint "$out/composer/$capture" > "$out/capture-server/allowed-clients.sha256"
    fingerprint "$out/capture-client/$signer" > "$out/signer/allowed-clients.sha256"
elif [[ $mode == direct || $mode == shared ]]; then
    issue composer clientAuth "$out/composer/$signer"
    fingerprint "$out/composer/$signer" > "$out/signer/allowed-clients.sha256"
    if [[ $mode == shared ]]; then
        issue composer-b clientAuth "$out/composer-b/$signer"
        fingerprint "$out/composer-b/$signer" >> "$out/signer/allowed-clients.sha256"
    fi
else
    echo "expected direct, shared or capture" >&2
    exit 2
fi
