# Sourced helper: probe every eth_sendBundle relay listed in a comma-separated
# EEZ_L1_BUILDER_RPC_URL value (the format the Composer accepts).
#
# Relay URLs may carry credentials, so output names a relay only by its
# position and host. The Composer validates the list itself; this probe only
# checks reachability and method support before startup.
#
# Policy: at least one relay must accept the deliberately empty probe bundle
# (a result, or any JSON-RPC error other than -32601). Other relays that are
# unreachable or lack eth_sendBundle only warn, because one acknowledging
# relay is sufficient for each dispatch. Probe success does not prove that a
# relay will include a real bundle.
#
# Usage: probe_bundle_relays "$EEZ_L1_BUILDER_RPC_URL"   (requires curl, jq)

builder_relay_host() {
    local host="${1#*://}"
    host="${host%%/*}"
    host="${host%%\?*}"
    host="${host##*@}"
    case "$host" in
        \[*\]) ;;
        *) host="${host%:*}" ;;
    esac
    printf '%s\n' "${host:-<unknown>}"
}

probe_bundle_relays() {
    local list="$1" url label probe index=0 healthy=0
    local -a urls
    if [[ -z "${list//[[:space:]]/}" ]]; then
        echo "✗ EEZ_L1_BUILDER_RPC_URL must list at least one eth_sendBundle relay" >&2
        return 1
    fi
    IFS=',' read -r -a urls <<<"$list"
    for url in "${urls[@]}"; do
        url="${url#"${url%%[![:space:]]*}"}"
        url="${url%"${url##*[![:space:]]}"}"
        if [[ -z "$url" ]]; then
            echo "✗ EEZ_L1_BUILDER_RPC_URL entry $index is empty" >&2
            return 1
        fi
        label="builder[$index] ($(builder_relay_host "$url"))"
        index=$((index + 1))
        if ! probe="$(curl --fail --silent --max-time 15 \
            -H 'content-type: application/json' \
            --data '{"jsonrpc":"2.0","method":"eth_sendBundle","params":[{"txs":[],"blockNumber":"0x0"}],"id":1}' \
            "$url")"; then
            echo "⚠ $label is unreachable" >&2
            continue
        fi
        if ! jq -e 'has("result") or (has("error") and .error.code != -32601)' >/dev/null 2>&1 <<<"$probe"; then
            echo "⚠ $label does not expose eth_sendBundle" >&2
            continue
        fi
        echo "==> $label eth_sendBundle endpoint available"
        healthy=$((healthy + 1))
    done
    if ((healthy == 0)); then
        echo "✗ none of the $index configured builder relays serves eth_sendBundle" >&2
        return 1
    fi
    if ((healthy < index)); then
        echo "⚠ $healthy of $index builder relays available; bundles still go to all of them" >&2
    fi
}
