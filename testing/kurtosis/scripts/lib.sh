#!/usr/bin/env bash
# Shared helpers for the kurtosis verification scripts. Source, do not execute.
#
# Callers must already have set: K (the testing/kurtosis dir), REPO (repo root),
# L2 and EEZL2_ADDRESS if they use create_l2_proxy.
# Requires cast, forge, jq, and curl in PATH; quote signing also uses bc.

# Ordinary setup/funding priority fee; cross-chain workloads request quotes.
PRIORITY_GAS_PRICE="${PRIORITY_GAS_PRICE:-${EEZ_TEST_PRIORITY_GAS_PRICE_WEI:-1}}"

load_deployment_artifact() { # <destination directory>
    if [[ "${EEZ_USE_LOCAL_DEPLOYMENTS:-0}" == "1" ]]; then
        cp "${EEZ_DEPLOYMENTS_FILE:-$REPO/deployments.env}" "$1/deployments.env"
    else
        kurtosis files download "$ENCLAVE" eez-deployments "$1" >/dev/null
    fi
}

capture_runtime_log() { # <service> <output file>
    if [[ "${EEZ_USE_LOCAL_DEPLOYMENTS:-0}" == "1" ]]; then
        local container
        case "$1" in
            eez-node) container="${EEZ_NODE_CONTAINER:?set EEZ_NODE_CONTAINER}" ;;
            eez-proof-signer) container="${EEZ_SIGNER_CONTAINER:?set EEZ_SIGNER_CONTAINER}" ;;
            *) echo "unsupported runtime service: $1" >&2; return 1 ;;
        esac
        timeout 30s docker logs "$container" >"$2" 2>&1
    else
        timeout 30s kurtosis service logs -a "$ENCLAVE" "$1" >"$2" 2>&1
    fi
}

raw_transaction_for() { # <rpc> <hash> <scratch directory> → signed envelope
    local rpc="$1" hash="$2" scratch="$3" raw transaction block_hash index actual
    raw=$(cast rpc eth_getRawTransactionByHash "$hash" --rpc-url "$rpc" 2>/dev/null | jq -er '.') || raw=""
    if [[ -z "$raw" ]]; then
        # Some Gnosis clients return null for mined raw transactions although
        # both the transaction metadata and complete raw block are available.
        transaction=$(cast rpc eth_getTransactionByHash "$hash" --rpc-url "$rpc") || return 1
        block_hash=$(jq -er '.blockHash' <<<"$transaction") || return 1
        index=$(jq -er '.transactionIndex' <<<"$transaction") || return 1
        [[ "$block_hash" =~ ^0x[[:xdigit:]]{64}$ && "$index" =~ ^0x[[:xdigit:]]+$ ]] || return 1
        # Pipe full blocks instead of passing them as shell arguments: a
        # public block can exceed the operating system's argument-size limit.
        cast rpc debug_getRawBlock "$block_hash" --rpc-url "$rpc" \
            | jq -er '.' | cast from-rlp >"$scratch/raw-block.json" || return 1
        jq -e --argjson index "$((index))" '.[1][$index] // error("missing transaction")' \
            "$scratch/raw-block.json" >"$scratch/raw-transaction.json" || return 1
        if jq -e 'type == "array"' "$scratch/raw-transaction.json" >/dev/null; then
            raw=$(jq -c '.' "$scratch/raw-transaction.json" | cast to-rlp) || return 1
        else
            raw=$(jq -er 'select(type == "string")' "$scratch/raw-transaction.json") || return 1
        fi
    fi
    [[ "$raw" =~ ^0x([[:xdigit:]]{2})+$ ]] || return 1
    actual=$(printf '%s' "$raw" | cast keccak) || return 1
    [[ "${actual,,}" == "${hash,,}" ]] || { echo "raw transaction hash mismatch: $hash" >&2; return 1; }
    printf '%s\n' "$raw"
}

yaml_value() { # <key> → scalar from the kurtosis args file
    # `-m1` not `| head -1`: head closes the pipe, grep takes SIGPIPE, and
    # pipefail then fails the whole pipeline.
    grep -m1 -E "^[[:space:]]*$1:" "${KURTOSIS_ARGS_FILE:-$K/args.yaml}" 2>/dev/null \
        | sed -E 's/^[^:]*:[[:space:]]*//; s/[[:space:]]*#.*$//; s/^"//; s/"$//'
}

fee_collection_enabled() { # in|out; workload expectation from the selected profile
    local name=EEZ_FEES_L1_TO_L2_ENABLED value
    [[ "$1" != out ]] || name=EEZ_FEES_L2_TO_L1_ENABLED
    value="${!name-}"
    [[ -v "$name" ]] || value=$(yaml_value "$name" || true)
    [[ "$value" == true ]]
}

expect_fee_admission_rejection() { # <front> <source RPC> <raw> <hash> <evidence path>
    local front="$1" rpc="$2" raw="$3" hash="$4" evidence="$5" response
    mkdir -p "$(dirname "$evidence")" || return 1
    response=$(curl --fail -sS --max-time 15 "$front" -H 'Content-Type: application/json' \
        --data "$(jq -nc --arg raw "$raw" '{jsonrpc:"2.0",id:1,method:"eth_sendRawTransaction",params:[$raw]}')") || return 1
    printf '%s\n' "$response" >"$evidence" || return 1
    jq -e '.error.code == -32003 and .error.data.code == "unsupported_transaction"' \
        <<<"$response" >/dev/null || {
        echo "expected execution-profile rejection at fee admission: $response" >&2; return 1;
    }
    [[ $(cast rpc --rpc-url "$front" eez_getCrossChainTransaction "$hash") == null ]] || return 1
    [[ $(receipt_status "$hash" "$rpc") == missing ]] || return 1
}

retry() { # <cmd...> → stdout; survives transient RPC hiccups under load
    local attempts=0 max="${RETRY_MAX:-6}" delay="${RETRY_DELAY:-3}" output rc
    while :; do
        # `if`, not a bare `out=$(...)`: under `set -e` a failure would abort
        # the script instead of reaching the retry below.
        if output=$("$@" 2>&1); then
            printf '%s' "$output"
            return 0
        else
            rc=$?
        fi
        (( ++attempts >= max )) && {
            echo "retry: '$*' failed after $attempts attempts: $output" >&2
            return "$rc"
        }
        sleep "$delay"
    done
}

gas_price_for() { # <rpc> → max fee in wei
    local rpc="$1" gas_price base_hex base minimum
    gas_price=$(cast gas-price --rpc-url "$rpc" 2>/dev/null || echo 1000000000)
    gas_price="${EEZ_TEST_GAS_PRICE_WEI:-$gas_price}"
    base_hex=$(cast block latest --field baseFeePerGas --rpc-url "$rpc" 2>/dev/null || echo 0)
    base=$(cast to-dec "$base_hex" 2>/dev/null || echo 0)
    minimum=$((2 * base + PRIORITY_GAS_PRICE))
    (( gas_price < minimum )) && gas_price="$minimum"
    echo "$gas_price"
}

sign_fee_quoted() { # <front> <read RPC> <key> <prepared raw> <evidence path> → signed envelope
    # Reuse the protocol runner's checked quote/signing path. It preserves the
    # prepared nonce and payload, retains malformed responses and never falls
    # back to node fees when a quote is rejected.
    source "$REPO/eez-core-protocol/script/e2e/lib/network-fees.sh"
    local front="$1" rpc="$2" key="$3" raw="$4" evidence="$5"
    local tx plan balance sender nonce current deadline
    tx=$(cast decode-transaction "$raw" | jq 'if type == "string" then fromjson else . end') || return 1
    sender=$(jq -er .signer <<<"$tx") || return 1
    nonce=$(cast to-dec "$(jq -er .nonce <<<"$tx")") || return 1
    if [[ "$rpc" == "$L2" ]]; then
        # L2 quotes cannot replay held predecessors or a source nonce changed
        # by unsettled execution. Wait for its canonical settlement first.
        deadline=$((SECONDS + ${EEZ_QUOTE_TIMEOUT:-120}))
        while :; do
            current=$(cast nonce "$sender" --block safe --rpc-url "$rpc") || return 1
            (( current == nonce )) && break
            if (( current > nonce || SECONDS >= deadline )); then
                echo "quoted wave nonce unavailable: expected=$nonce current=$current" >&2
                return 1
            fi
            sleep 1
        done
    fi
    plan=$(network_fee_quote "$tx" "$front" "$rpc" "" "" "$evidence") || return 1
    balance=$(cast balance "$sender" --rpc-url "$rpc") || return 1
    plan=$(network_sign_quote "$plan" "$key" "$balance") || return 1
    jq 'del(.raw)' <<<"$plan" >"$evidence" || return 1
    jq -er .raw <<<"$plan"
}

fund() { # <rpc> <key> <to> — configurable native-token amount
    local rpc="$1" key="$2" to="$3" from nonce amount
    amount="${EEZ_TEST_FUND_AMOUNT_ETH:-10}"
    [[ "$amount" =~ ^[0-9]+([.][0-9]{1,18})?$ && "$amount" =~ [1-9] ]] || {
        echo "EEZ_TEST_FUND_AMOUNT_ETH must be a positive decimal with at most 18 decimal places" >&2
        return 1
    }
    from=$(cast wallet address --private-key "$key")
    nonce=$(retry cast nonce "$from" --rpc-url "$rpc")
    cast send "$to" --value "${amount}ether" --private-key "$key" --nonce "$nonce" \
        --gas-price "$(gas_price_for "$rpc")" --priority-gas-price "$PRIORITY_GAS_PRICE" \
        --rpc-url "$rpc" >/dev/null
}

forge_deploy() { # <rpc> <key> <script:contract> <sig> <args...> → echoes forge stdout
    local rpc="$1" key="$2" script="$3" signature="$4" gas_price output
    shift 4
    gas_price=$(gas_price_for "$rpc")
    if ! output=$(cd "$REPO/contracts" && forge script "script/$script" --sig "$signature" "$@" \
        --rpc-url "$rpc" --broadcast --private-key "$key" --gas-price "$gas_price" \
        --priority-gas-price "$PRIORITY_GAS_PRICE" --skip-simulation 2>&1); then
        printf '%s\n' "$output" >&2
        return 1
    fi
    printf '%s\n' "$output"
}

grab_address() {
    # Drain the stream even after finding the address. Closing early can give
    # forge_deploy's printf SIGPIPE and fail a successful deployment under pipefail.
    grep -oE "$1=0x[0-9a-fA-F]{40}" | sed -n '1p' | cut -d= -f2
}
strip_ansi() { sed 's/\x1b\[[0-9;]*m//g'; }

receipt_json() { # <hash> <rpc>
    curl -s --max-time 3 -X POST -H 'Content-Type: application/json' \
        --data "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getTransactionReceipt\",\"params\":[\"$1\"],\"id\":1}" \
        "$2" 2>/dev/null
}

receipt_status() { # <hash> <rpc> → "1" mined-ok, "0x0" reverted, "missing" not mined
    jq -r '.result.status // "missing"' <<<"$(receipt_json "$1" "$2")" 2>/dev/null \
        | sed 's/^0x1$/1/'
}

send_front() { # <front_url> <raw_tx> <expected_hash> — eth_sendRawTransaction to a front
    # A predecessor can enter composition after quoting. These explicit temporary
    # refusals acquire no ownership, so retry identical bytes under one deadline.
    # Tariff/profile failures need caller action; ambiguous transport failures and
    # changed hashes remain fatal rather than reporting a submission as successful.
    local front="$1" raw="$2" expected="$3" resp rc returned left
    local deadline=$((SECONDS + 120))
    while (( SECONDS < deadline )); do
        left=$((deadline - SECONDS)); (( left <= 10 )) || left=10
        rc=0
        resp=$(curl -sS --max-time "$left" -X POST "$front" -H 'Content-Type: application/json' \
            -d "{\"jsonrpc\":\"2.0\",\"method\":\"eth_sendRawTransaction\",\"params\":[\"$raw\"],\"id\":1}" 2>/dev/null) || rc=$?
        # Empty body = no answer. Without this the checks below miss and a tx
        # that was NEVER SENT reports success.
        (( rc == 0 )) && [[ -n "$resp" ]] \
            || { echo "    ✗ submit failed (curl rc=$rc, ${#resp} byte body)" >&2; return 1; }
        if grep -q '"error"' <<<"$resp"; then
            if ! grep -q 'starting up' <<<"$resp" && ! jq -e '
                .error | .code == -32003 and (
                    .data.code == "quote_busy" or .data.code == "stale_context" or
                    (.data.code == "quote_unavailable" and (.data.message ==
                        "quote unavailable: queued predecessor is entering composition" or .data.message ==
                        "quote unavailable: settlement reconciliation pending" or .data.message ==
                        "quote unavailable: unsettled native history requires reconciliation")))
                ' <<<"$resp" >/dev/null 2>&1; then
                echo "    ✗ front rejected tx: $resp" >&2; return 1
            fi
            echo "    retrying unchanged source $expected after temporary refusal: $resp" >&2
            sleep 1
            continue
        fi
        returned=$(jq -er '.result // error("missing transaction hash")' <<<"$resp" 2>/dev/null || true)
        [[ "${returned,,}" == "${expected,,}" ]] \
            || { echo "    ✗ front changed the submitted transaction: $resp" >&2; return 1; }
        return 0
    done
    echo "    ✗ front admission retry deadline reached after 120s" >&2
    return 1
}

create_l2_proxy() { # <target_on_L1> <deployer_key> <rollup_id> → proxy address
    local target="$1" key="$2" rollup_id="$3" proxy code nonce chain_id deployer receipt
    deployer=$(cast wallet address --private-key "$key") || return 1
    chain_id=$(cast chain-id --rpc-url "$L2") || return 1
    proxy=$(cast call "$EEZL2_ADDRESS" 'computeCrossChainProxyAddress(address,uint64)(address)' \
        "$target" "$rollup_id" --rpc-url "$L2" | tr -d '[:space:]') || return 1
    code=$(cast code "$proxy" --rpc-url "$L2") || return 1
    if [[ "$code" == "0x" || -z "$code" ]]; then
        nonce=$(retry cast nonce "$deployer" --rpc-url "$L2") || return 1
        if ! receipt=$(cast send --rpc-url "$L2" --chain-id "$chain_id" \
            --private-key "$key" --nonce "$nonce" --gas-limit 1500000 \
            --gas-price "$(gas_price_for "$L2")" --priority-gas-price "$PRIORITY_GAS_PRICE" \
            --timeout 30 --json \
            "$EEZL2_ADDRESS" 'createCrossChainProxy(address,uint64)' "$target" "$rollup_id"); then
            echo "L2 proxy creation failed: target=$target proxy=$proxy nonce=$nonce" >&2
            return 1
        fi
        if ! jq -e '.status == "0x1"' <<<"$receipt" >/dev/null; then
            echo "L2 proxy creation did not succeed: proxy=$proxy receipt=$receipt" >&2
            return 1
        fi
        code=$(cast code "$proxy" --rpc-url "$L2") || return 1
    fi
    [[ "$code" =~ ^0x([[:xdigit:]]{2})+$ ]] || {
        echo "L2 proxy has no deployed bytecode: target=$target proxy=$proxy" >&2
        return 1
    }
    echo "$proxy"
}
