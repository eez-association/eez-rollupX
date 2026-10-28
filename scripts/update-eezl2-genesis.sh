#!/usr/bin/env bash

set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
PROTOCOL="$REPO/eez-core-protocol"
GENERATOR="$REPO/scripts/solidity/GenerateEEZL2Genesis.s.sol"
HISTORY_CONTRACT="$REPO/contracts/src/L2/L1BlockhashContract.sol"
FEE_VAULT_CONTRACT="$REPO/contracts/src/L2/FeeVault.sol"
FIXTURE_PROFILE="$REPO/testing/kurtosis/l2-genesis-profile.json"
EXPECTED_FOUNDRY_VERSION="1.7.1"
USE_GAS_LEFT=false
# EEZL2 is an upgradeable predeploy, laid out as the protocol's
# deployment/Deploy.s.sol:_deployL2 leaves it: an EEZProxy (the manager address
# every consumer uses), its ProxyAdmin at CREATE(proxy, 1), and the EEZL2
# implementation holding the deployment's immutables.
EEZL2_ADDRESS="0x4B2760F84B83d6b3829E30A7573B0172B6d6402C"
EEZL2_IMPLEMENTATION_ADDRESS="0x4C3D971b1079E58843D0E1af155A2e54b9E54dbA"
HISTORY_ADDRESS="0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0077"
HISTORY_IMPLEMENTATION_ADDRESS="0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee007a"
FEE_VAULT_ADDRESS="0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0078"
FEE_VAULT_IMPLEMENTATION_ADDRESS="0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0079"
INITIALIZABLE_SLOT="0xf0c57e16840df040f15088dc2f81fe391c3923bec73e23a9662efc9c229c6a00"
OWNABLE_SLOT="0x9016d09d72d40fdae2fd8ceac6b6234c7706214fd39c1cd1e609a0528c199300"
PENDING_OWNER_SLOT="0x237e158222e3e6968b72b9db0d8043aacf074ad9f650f0d1606b4d82ee432c00"
ERC1967_IMPLEMENTATION_SLOT="0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"
ERC1967_ADMIN_SLOT="0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103"

# Independent fixed vector for the canonical CrossChainProxy CREATE2 formula.
PROXY_VECTOR_ORIGINAL_ADDRESS="0x11223344556677889900aabbccddeeff00112233"
PROXY_VECTOR_ORIGINAL_ROLLUP_ID=0
EXPECTED_PROXY_SALT="0x7c432a8c413604983b9f189bb5b6650220b6b9b038267802fef07276a796e168"
EXPECTED_PROXY_INIT_CODE_HASH="0x26f77730044fbffb547593b1a676eb53cbbb9a57879c3f2cf090cee88a283a4a"
EXPECTED_PROXY_ADDRESS="0x37bf538cadaffd0421ff26d10269a7084be60a2d"

usage() {
    cat >&2 <<'EOF'
usage:
  scripts/update-eezl2-genesis.sh
  scripts/update-eezl2-genesis.sh --check
  scripts/update-eezl2-genesis.sh --render --rollup-id ID --system-address ADDRESS \
      --recovery-address ADDRESS --upgrade-owner ADDRESS \
      --treasury-owner ADDRESS --treasury-recipient ADDRESS --output PATH \
      [--base PATH] [--profile-output PATH]

The default mode regenerates the committed genesis files (genesis.dev.json,
the ETH-free production base genesis.json and the E2E fixture) from
testing/kurtosis/l2-genesis-profile.json. --render creates a deployment-specific
genesis without modifying committed files. Private keys are never accepted;
the deployment uses the reserved native system address. The recovery address
receives ether swept from proxies at construction; deployments use the L1
poster account, and the fixtures use the public development poster. The upgrade
owner initially owns the separate EEZL2 and L1 history ProxyAdmins and can
replace either implementation with an L2 transaction.
The fixed FeeVault predeploy collects L2 base fees. Its initial owner and L2
recipient are explicit inputs; the owner can rotate the recipient or transfer
ownership using ordinary L2 transactions. The same initial owner controls its
dedicated ProxyAdmin; both authorities must be transferred separately.
EOF
}

mode="update"
rollup_id=""
system_address=""
recovery_address=""
upgrade_owner=""
fee_collector="$FEE_VAULT_ADDRESS"
treasury_owner=""
treasury_recipient=""
treasury_owner_seen=false
treasury_recipient_seen=false
base_genesis=""
output=""
profile_output=""

while (( $# > 0 )); do
    case "$1" in
        --check)
            [[ "$mode" == "update" ]] || { usage; exit 2; }
            mode="check"
            shift
            ;;
        --render)
            [[ "$mode" == "update" ]] || { usage; exit 2; }
            mode="render"
            shift
            ;;
        --rollup-id)
            rollup_id="${2:-}"
            shift 2
            ;;
        --system-address)
            system_address="${2:-}"
            shift 2
            ;;
        --recovery-address)
            recovery_address="${2:-}"
            shift 2
            ;;
        --upgrade-owner)
            upgrade_owner="${2:-}"
            shift 2
            ;;
        --treasury-owner)
            [[ "$treasury_owner_seen" == false ]] || { echo "--treasury-owner must occur exactly once" >&2; exit 2; }
            treasury_owner_seen=true
            treasury_owner="${2:-}"
            shift 2
            ;;
        --treasury-recipient)
            [[ "$treasury_recipient_seen" == false ]] || { echo "--treasury-recipient must occur exactly once" >&2; exit 2; }
            treasury_recipient_seen=true
            treasury_recipient="${2:-}"
            shift 2
            ;;
        --base)
            base_genesis="${2:-}"
            shift 2
            ;;
        --output)
            output="${2:-}"
            shift 2
            ;;
        --profile-output)
            profile_output="${2:-}"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage
            exit 2
            ;;
    esac
done

# Resolve once: CI must consume the producer's verified executable, never
# silently start an unconfigured second Rust build in a contract-only job.
genesis_tool="$(command -v eez-genesis-state-root || true)"
if [[ "${GITHUB_ACTIONS:-false}" == true && ( -z "$genesis_tool" || ! -x "$genesis_tool" ) ]]; then
    echo "CI requires the verified eez-genesis-state-root tool; refusing Cargo fallback" >&2
    exit 1
fi

if [[ "$mode" == "render" ]]; then
    [[ -n "$rollup_id" && -n "$system_address" && -n "$recovery_address" && -n "$upgrade_owner" \
        && -n "$treasury_owner" && -n "$treasury_recipient" && -n "$output" ]] || {
        usage
        exit 2
    }
    base_genesis="${base_genesis:-$REPO/genesis.json}"
else
    [[ "$treasury_owner_seen" == false && "$treasury_recipient_seen" == false && -z "$rollup_id$system_address$recovery_address$upgrade_owner$base_genesis$output$profile_output" ]] || {
        usage
        exit 2
    }
    [[ -f "$FIXTURE_PROFILE" ]] || {
        echo "fixture profile is missing: $FIXTURE_PROFILE" >&2
        exit 1
    }
    rollup_id="$(jq -er '.rollupId | tostring' "$FIXTURE_PROFILE")"
    system_address="$(jq -er '.systemAddress' "$FIXTURE_PROFILE")"
    recovery_address="$(jq -er '.recoveryAddress' "$FIXTURE_PROFILE")"
    upgrade_owner="$(jq -er '.upgradeOwner' "$FIXTURE_PROFILE")"
    jq -e '.schemaVersion == 5' "$FIXTURE_PROFILE" >/dev/null || {
        echo "fixture profile must use schema version 5" >&2; exit 1;
    }
    treasury_owner="$(jq -er '.treasuryOwner' "$FIXTURE_PROFILE")"
    treasury_recipient="$(jq -er '.treasuryRecipient' "$FIXTURE_PROFILE")"
fi

python3 - "$rollup_id" <<'PY'
import sys

try:
    value = int(sys.argv[1], 10)
except ValueError:
    raise SystemExit("rollup ID must be a decimal integer")
if not 0 < value <= (1 << 64) - 1:
    raise SystemExit("rollup ID must be in 1..=u64::MAX")
PY

if [[ ! "$system_address" =~ ^0x[0-9a-fA-F]{40}$ ]]; then
    echo "system address must contain exactly 20 hexadecimal bytes" >&2
    exit 1
fi
system_address="$(cast to-check-sum-address "$system_address")"
if [[ "${system_address,,}" != "0xfffffffffffffffffffffffffffffffffffffffe" ]]; then
    echo "system address must be the reserved native address 0xfffffffffffffffffffffffffffffffffffffffe" >&2
    exit 1
fi
for field in treasury_owner treasury_recipient; do
    value="${!field}"
    if [[ ! "$value" =~ ^0x[0-9a-fA-F]{40}$ ]]; then
        echo "$field must contain exactly 20 hexadecimal bytes" >&2
        exit 1
    fi
    value="${value,,}"
    if [[ "$value" == "0x0000000000000000000000000000000000000000" ||
          "$value" == "$fee_collector" || "$value" == "${system_address,,}" || "$value" == "${EEZL2_ADDRESS,,}" ]]; then
        echo "$field must be nonzero and differ from FeeVault, SYSTEM and EEZL2" >&2
        exit 1
    fi
    printf -v "$field" '%s' "$value"
done
if [[ ! "$recovery_address" =~ ^0x[0-9a-fA-F]{40}$ ]]; then
    echo "recovery address must contain exactly 20 hexadecimal bytes" >&2
    exit 1
fi
recovery_address="$(cast to-check-sum-address "$recovery_address")"
# EEZL2 rejects zero. Distinct values also keep the immutable identification
# below unambiguous; the protocol separates recovery from system funding.
if [[ "${recovery_address,,}" == "0x0000000000000000000000000000000000000000" ||
      "${recovery_address,,}" == "${system_address,,}" ]]; then
    echo "recovery address must be nonzero and differ from the system address" >&2
    exit 1
fi
if [[ ! "$upgrade_owner" =~ ^0x[0-9a-fA-F]{40}$ ]]; then
    echo "upgrade owner must contain exactly 20 hexadecimal bytes" >&2
    exit 1
fi
upgrade_owner="$(cast to-check-sum-address "$upgrade_owner")"
# The system address cannot send ordinary transactions, so it could never upgrade.
if [[ "${upgrade_owner,,}" == "0x0000000000000000000000000000000000000000" ||
      "${upgrade_owner,,}" == "${system_address,,}" ]]; then
    echo "upgrade owner must be nonzero and differ from the system address" >&2
    exit 1
fi

forge_version="$(forge --version | sed -n 's/^forge Version: //p')"
if [[ "$forge_version" != "$EXPECTED_FOUNDRY_VERSION" ]]; then
    echo "Foundry $EXPECTED_FOUNDRY_VERSION is required; found ${forge_version:-unknown}" >&2
    exit 1
fi

for required in "$PROTOCOL/foundry.toml" "$PROTOCOL/remappings.txt" "$PROTOCOL/src" \
    "$PROTOCOL/lib" "$GENERATOR" "$HISTORY_CONTRACT" "$FEE_VAULT_CONTRACT"
do
    if [[ ! -e "$required" ]]; then
        echo "required canonical protocol input is missing: $required" >&2
        exit 1
    fi
done

build_root="$(mktemp -d)"
cleanup() {
    rm -rf "$build_root"
}
trap cleanup EXIT

cp "$PROTOCOL/foundry.toml" "$PROTOCOL/remappings.txt" "$build_root/"
[[ ! -f "$PROTOCOL/foundry.lock" ]] || cp "$PROTOCOL/foundry.lock" "$build_root/"
cp -a "$PROTOCOL/src" "$build_root/src"
cp -a "$PROTOCOL/lib" "$build_root/lib"
# Compile the RollupX predeploy at its canonical source name. Source paths enter
# Solidity metadata and therefore the genesis/profile commitment. Only this
# disposable build tree is extended; the pinned protocol checkout stays intact.
if [[ -e "$build_root/src/L2/L1BlockhashContract.sol" ]]; then
    echo "pinned protocol conflicts with the RollupX history source" >&2
    exit 1
fi
cp "$HISTORY_CONTRACT" "$build_root/src/L2/L1BlockhashContract.sol"
[[ ! -e "$build_root/src/L2/FeeVault.sol" ]] || { echo "pinned protocol conflicts with the RollupX fee vault source" >&2; exit 1; }
cp "$FEE_VAULT_CONTRACT" "$build_root/src/L2/FeeVault.sol"
mkdir "$build_root/script"
cp "$GENERATOR" "$build_root/script/GenerateEEZL2Genesis.s.sol"

if ! forge_output="$({
    cd "$build_root"
    forge script script/GenerateEEZL2Genesis.s.sol:GenerateEEZL2Genesis \
        --sig "run(uint64,address,bool,address,address,address,address,address,address,address,address)" \
        "$rollup_id" "$system_address" "$USE_GAS_LEFT" "$recovery_address" \
        "$upgrade_owner" "$EEZL2_ADDRESS" "$EEZL2_IMPLEMENTATION_ADDRESS" \
        "$fee_collector" "$FEE_VAULT_IMPLEMENTATION_ADDRESS" "$treasury_owner" "$treasury_recipient"
} 2>&1)"; then
    printf '%s\n' "$forge_output" >&2
    echo "EEZL2 genesis generator failed" >&2
    exit 1
fi

# Collect the generator's accounts and storage writes, then require exactly the
# three implementations, proxies and ProxyAdmins with exactly the expected state.
proxy_admin_address="$(cast compute-address --nonce 1 "$EEZL2_ADDRESS" | sed -n 's/^Computed Address: //p')"
[[ "$proxy_admin_address" =~ ^0x[0-9a-fA-F]{40}$ ]] || {
    echo "could not derive the EEZL2 ProxyAdmin address" >&2
    exit 1
}
proxy_admin_address="$(cast to-check-sum-address "$proxy_admin_address")"
declare -A account_nonce=() account_code=() storage=()
while read -r kind address first second; do
    address="${address,,}"
    case "$kind" in
        GENESIS_ACCOUNT)
            [[ -z "${account_code[$address]:-}" ]] || { echo "duplicate generated account $address" >&2; exit 1; }
            account_nonce[$address]="$first"
            account_code[$address]="${second,,}"
            ;;
        GENESIS_STORAGE)
            [[ -z "${storage[$address $first]:-}" ]] || { echo "slot written twice: $address $first" >&2; exit 1; }
            storage[$address $first]="${second,,}"
            ;;
    esac
done < <(sed -n 's/^  \(GENESIS_[A-Z]* .*\)$/\1/p' <<<"$forge_output")

impl_key="${EEZL2_IMPLEMENTATION_ADDRESS,,}"
proxy_key="${EEZL2_ADDRESS,,}"
admin_key="${proxy_admin_address,,}"
fee_impl_key="$FEE_VAULT_IMPLEMENTATION_ADDRESS"
fee_admin_key="$(cast compute-address --nonce 1 "$fee_collector" | sed -n 's/^Computed Address: //p')"
fee_admin_key="${fee_admin_key,,}"
[[ "$fee_admin_key" =~ ^0x[0-9a-f]{40}$ ]] || { echo "invalid FeeVault ProxyAdmin address" >&2; exit 1; }
history_impl_key="$HISTORY_IMPLEMENTATION_ADDRESS"
history_admin_key="$(cast compute-address --nonce 1 "$HISTORY_ADDRESS" | sed -n 's/^Computed Address: //p')"
history_admin_key="${history_admin_key,,}"
[[ "$history_admin_key" =~ ^0x[0-9a-f]{40}$ ]] || { echo "invalid history ProxyAdmin address" >&2; exit 1; }
if (( ${#account_code[@]} != 9 )) || [[ -z "${account_code[$impl_key]:-}" ||
    -z "${account_code[$proxy_key]:-}" || -z "${account_code[$admin_key]:-}" || -z "${account_code[$fee_collector]:-}" || -z "${account_code[$fee_impl_key]:-}" || -z "${account_code[$fee_admin_key]:-}" || -z "${account_code[$HISTORY_ADDRESS]:-}" || -z "${account_code[$history_impl_key]:-}" || -z "${account_code[$history_admin_key]:-}" ]]
then
    printf '%s\n' "$forge_output" >&2
    echo "expected exactly the EEZL2, FeeVault and history implementations, proxies and ProxyAdmins" >&2
    exit 1
fi
for nonce_expected in "$impl_key|1" "$proxy_key|2" "$admin_key|1" "$fee_collector|2" "$fee_impl_key|1" "$fee_admin_key|1" "$HISTORY_ADDRESS|2" "$history_impl_key|1" "$history_admin_key|1"; do
    IFS='|' read -r account expected <<<"$nonce_expected"
    [[ "${account_nonce[$account]}" == "$expected" ]] || {
        echo "unexpected generated nonce for $account: ${account_nonce[$account]}" >&2
        exit 1
    }
done
address_word() {
    local hex="${1#0x}"
    printf '0x%064s\n' "${hex,,}" | tr ' ' 0
}
impl_word="$(address_word "$impl_key")"
admin_word="$(address_word "$admin_key")"
owner_word="$(address_word "$upgrade_owner")"
zero_slot="0x0000000000000000000000000000000000000000000000000000000000000000"
treasury_owner_word="$(address_word "$treasury_owner")"
treasury_recipient_word="$(address_word "$treasury_recipient")"
fee_impl_word="$(address_word "$fee_impl_key")"
fee_admin_word="$(address_word "$fee_admin_key")"
history_admin_word="$(address_word "$history_admin_key")"
initialized_word="$(printf '0x%064x' 1)"
locked_word="0x000000000000000000000000000000000000000000000000ffffffffffffffff"
declare -A expected_storage=(
    ["$proxy_key $ERC1967_IMPLEMENTATION_SLOT"]="$impl_word"
    ["$proxy_key $ERC1967_ADMIN_SLOT"]="$admin_word"
    ["$admin_key $zero_slot"]="$owner_word"
    ["$HISTORY_ADDRESS $ERC1967_IMPLEMENTATION_SLOT"]="$(address_word "$history_impl_key")"
    ["$HISTORY_ADDRESS $ERC1967_ADMIN_SLOT"]="$history_admin_word"
    ["$history_admin_key $zero_slot"]="$owner_word"
    ["$fee_collector $ERC1967_IMPLEMENTATION_SLOT"]="$fee_impl_word"
    ["$fee_collector $ERC1967_ADMIN_SLOT"]="$fee_admin_word"
    ["$fee_collector $INITIALIZABLE_SLOT"]="$initialized_word"
    ["$fee_collector $OWNABLE_SLOT"]="$treasury_owner_word"
    ["$fee_collector $PENDING_OWNER_SLOT"]="$zero_slot"
    ["$fee_collector $zero_slot"]="$treasury_recipient_word"
    ["$fee_impl_key $INITIALIZABLE_SLOT"]="$locked_word"
    ["$fee_admin_key $zero_slot"]="$treasury_owner_word"
)
if (( ${#storage[@]} != ${#expected_storage[@]} )); then
    echo "unexpected EEZL2 genesis storage writes: ${!storage[*]}" >&2
    exit 1
fi
for key in "${!expected_storage[@]}"; do
    [[ "${storage[$key]:-missing}" == "${expected_storage[$key]}" ]] || {
        echo "unexpected EEZL2 genesis storage $key: ${storage[$key]:-missing}" >&2
        exit 1
    }
done

runtime="${account_code[$impl_key]}"
proxy_runtime="${account_code[$proxy_key]}"
admin_runtime="${account_code[$admin_key]}"
fee_vault_runtime="${account_code[$fee_collector]}"
fee_impl_runtime="${account_code[$fee_impl_key]}"
fee_vault_artifact="$build_root/out/FeeVault.sol/FeeVault.json"
jq -e '.metadata.settings.compilationTarget == {"src/L2/FeeVault.sol": "FeeVault"}
       and (.deployedBytecode.immutableReferences // {}) == {}' "$fee_vault_artifact" >/dev/null || {
    echo "FeeVault must have canonical metadata and no immutables" >&2; exit 1;
}
[[ "$fee_impl_runtime" == "$(jq -er '.deployedBytecode.object | ascii_downcase' "$fee_vault_artifact")" ]] || {
    echo "generated FeeVault runtime differs from its compiler artifact" >&2; exit 1;
}
fee_vault_runtime_hash="$(cast keccak "$fee_vault_runtime")"
fee_impl_runtime_hash="$(cast keccak "$fee_impl_runtime")"

# The proxy runtime is the canonical EEZProxy with only its ProxyAdmin
# immutable filled in; the ProxyAdmin runtime has no immutables at all.
eez_proxy_artifact="$build_root/out/EEZProxy.sol/EEZProxy.json"
proxy_admin_artifact="$build_root/out/ProxyAdmin.sol/ProxyAdmin.json"
jq -e '.metadata.settings.compilationTarget == {"src/proxy/EEZProxy.sol": "EEZProxy"}' \
    "$eez_proxy_artifact" >/dev/null || {
    echo "EEZProxy was not compiled under the canonical protocol source path" >&2
    exit 1
}
jq -e '(.deployedBytecode.immutableReferences // {}) == {}' "$proxy_admin_artifact" >/dev/null || {
    echo "ProxyAdmin unexpectedly has immutables" >&2
    exit 1
}
[[ "$admin_runtime" == "$(jq -er '.deployedBytecode.object | ascii_downcase' "$proxy_admin_artifact")" ]] || {
    echo "generated ProxyAdmin runtime differs from the compiled artifact" >&2
    exit 1
}
patched_proxy_runtime() {
    python3 - "$1" "$2" <<'PY'
import json
import sys

artifact = json.load(open(sys.argv[1]))
code = bytearray.fromhex(artifact["deployedBytecode"]["object"].removeprefix("0x"))
references = artifact["deployedBytecode"]["immutableReferences"]
if len(references) != 1:
    raise SystemExit(f"expected one proxy admin immutable set, found {len(references)}")
word = bytes.fromhex(sys.argv[2].removeprefix("0x"))
for reference in next(iter(references.values())):
    if reference["length"] != 32:
        raise SystemExit("unexpected proxy admin immutable length")
    code[reference["start"]:reference["start"] + 32] = word
print("0x" + code.hex())
PY
}
expected_proxy_runtime="$(patched_proxy_runtime "$eez_proxy_artifact" "$admin_word")"
[[ "$proxy_runtime" == "$expected_proxy_runtime" ]] || {
    echo "generated EEZProxy runtime differs from the artifact with its ProxyAdmin immutable" >&2
    exit 1
}

fee_proxy_artifact="$build_root/out/TransparentUpgradeableProxy.sol/TransparentUpgradeableProxy.json"
[[ "$fee_vault_runtime" == "$(patched_proxy_runtime "$fee_proxy_artifact" "$fee_admin_word")" ]] || {
    echo "generated FeeVault proxy differs from its canonical artifact and admin immutable" >&2; exit 1;
}
[[ "${account_code[$fee_admin_key]}" == "$admin_runtime" ]] || {
    echo "generated FeeVault ProxyAdmin runtime differs from the canonical artifact" >&2; exit 1;
}

artifact="$build_root/out/EEZL2.sol/EEZL2.json"
if ! jq -e \
    '.metadata.settings.compilationTarget == {"src/L2/EEZL2.sol": "EEZL2"}' \
    "$artifact" >/dev/null
then
    echo "EEZL2 was not compiled under the canonical protocol source path" >&2
    exit 1
fi

# Bind the canonical no-argument proxy creation bytecode and CREATE2 formula
# to an independent fixed vector. The proxy binds its manager from msg.sender.
proxy_artifact="$build_root/out/CrossChainProxy.sol/CrossChainProxy.json"
proxy_creation_code="$(jq -er \
    '.bytecode.object | select(type == "string" and startswith("0x"))' \
    "$proxy_artifact")"
proxy_salt="$(cast keccak "$(
    cast abi-encode --packed "f(uint64,address)" \
        "$PROXY_VECTOR_ORIGINAL_ROLLUP_ID" "$PROXY_VECTOR_ORIGINAL_ADDRESS"
)")"
proxy_init_code="$proxy_creation_code"
proxy_init_code_hash="$(cast keccak "$proxy_init_code")"
create2_preimage="$(cast concat-hex \
    0xff "$proxy_key" "$proxy_salt" "$proxy_init_code_hash")"
proxy_digest="$(cast keccak "$create2_preimage")"
proxy_address="0x${proxy_digest: -40}"

for actual_expected_label in \
    "$proxy_salt|$EXPECTED_PROXY_SALT|CrossChainProxy salt" \
    "$proxy_init_code_hash|$EXPECTED_PROXY_INIT_CODE_HASH|CrossChainProxy init-code hash" \
    "$proxy_address|$EXPECTED_PROXY_ADDRESS|CrossChainProxy CREATE2 address"
do
    IFS='|' read -r actual expected label <<<"$actual_expected_label"
    if [[ "$actual" != "$expected" ]]; then
        echo "unexpected $label: $actual" >&2
        echo "expected: $expected" >&2
        exit 1
    fi
done

# EEZBase caches the argument-free proxy init-code hash. It is independent of
# the generator's temporary manager address, so validate rather than patch it.
# Identify every immutable set and fail closed on compiler/layout drift.
rollup_word="$(cast abi-encode 'f(uint64)' "$rollup_id")"
system_word="$(cast abi-encode 'f(address)' "$system_address")"
use_gas_left_word="$(cast abi-encode 'f(bool)' "$USE_GAS_LEFT")"
recovery_word="$(cast abi-encode 'f(address)' "$recovery_address")"
mapfile -t immutable_ids < <(jq -er '.deployedBytecode.immutableReferences | keys[]' "$artifact")
if (( ${#immutable_ids[@]} != 5 )); then
    echo "expected exactly five EEZL2 immutable reference sets; found ${#immutable_ids[@]}" >&2
    exit 1
fi

runtime_body="${runtime#0x}"
proxy_immutable_id=""
declare -A matched_public_immutables=()
for immutable_id in "${immutable_ids[@]}"; do
    mapfile -t references < <(jq -er \
        --arg id "$immutable_id" \
        '.deployedBytecode.immutableReferences[$id][] | "\(.start) \(.length)"' \
        "$artifact")
    (( ${#references[@]} > 0 )) || {
        echo "EEZL2 immutable $immutable_id has no runtime references" >&2
        exit 1
    }

    reference_value=""
    for reference in "${references[@]}"; do
        read -r start length <<<"$reference"
        [[ "$length" == "32" ]] || {
            echo "EEZL2 immutable $immutable_id has unexpected length $length" >&2
            exit 1
        }
        value="0x${runtime_body:$((start * 2)):$((length * 2))}"
        if [[ -n "$reference_value" && "$value" != "$reference_value" ]]; then
            echo "EEZL2 immutable $immutable_id has inconsistent runtime values" >&2
            exit 1
        fi
        reference_value="$value"
    done

    case "${reference_value,,}" in
        "${rollup_word,,}") matched_public_immutables[rollup]=1 ;;
        "${system_word,,}") matched_public_immutables[system]=1 ;;
        "${use_gas_left_word,,}") matched_public_immutables[use_gas_left]=1 ;;
        "${recovery_word,,}") matched_public_immutables[recovery]=1 ;;
        *)
            [[ -z "$proxy_immutable_id" ]] || {
                echo "more than one unknown EEZL2 immutable reference set" >&2
                exit 1
            }
            [[ "$reference_value" == "$proxy_init_code_hash" ]] || {
                echo "EEZL2 proxy init-code immutable differs from canonical creation code" >&2
                exit 1
            }
            proxy_immutable_id="$immutable_id"
            ;;
    esac
done

for public_immutable in rollup system use_gas_left recovery; do
    [[ "${matched_public_immutables[$public_immutable]:-}" == "1" ]] || {
        echo "could not identify EEZL2 $public_immutable immutable references" >&2
        exit 1
    }
done
[[ -n "$proxy_immutable_id" ]] || {
    echo "could not identify EEZL2 PROXY_INIT_CODE_HASH immutable references" >&2
    exit 1
}

runtime_hash="$(cast keccak "$runtime")"
proxy_runtime_hash="$(cast keccak "$proxy_runtime")"

# Bind the stable proxy identity separately from the genesis implementation.
history_runtime="${account_code[$HISTORY_ADDRESS]}"
history_impl_runtime="${account_code[$history_impl_key]}"
history_runtime_hash="$(cast keccak "$history_runtime")"
history_impl_runtime_hash="$(cast keccak "$history_impl_runtime")"
[[ "$history_runtime" == "$(patched_proxy_runtime "$eez_proxy_artifact" "$history_admin_word")" &&
   "${account_code[$history_admin_key]}" == "$admin_runtime" ]] || {
    echo "history proxy/admin differs from canonical artifacts" >&2; exit 1;
}
history_artifact="$build_root/out/L1BlockhashContract.sol/L1BlockhashContract.json"
jq -e '.metadata.settings.compilationTarget == {"src/L2/L1BlockhashContract.sol": "L1BlockhashContract"}
       and (.deployedBytecode.immutableReferences // {}) == {}' "$history_artifact" >/dev/null || {
    echo "history implementation must have canonical metadata and no immutables" >&2; exit 1;
}
[[ "$history_impl_runtime" == "$(jq -er '.deployedBytecode.object | ascii_downcase' "$history_artifact")" ]] || {
    echo "history implementation differs from canonical artifact" >&2; exit 1;
}
python3 - "$mode" "$REPO" "$history_runtime" "$history_runtime_hash" "$history_impl_runtime" "$admin_runtime" <<'PY_HISTORY_CODE'
from pathlib import Path
import sys
mode, root, runtime, digest, implementation, admin = sys.argv[1:]
root = Path(root)
outputs = {
    root / 'crates/eez-protocol/src/l1_history/runtime.bin': bytes.fromhex(runtime.removeprefix('0x')),
    root / 'crates/eez-protocol/src/l1_history/implementation.bin': bytes.fromhex(implementation.removeprefix('0x')),
    root / 'crates/eez-protocol/src/l1_history/proxy_admin.bin': bytes.fromhex(admin.removeprefix('0x')),
    root / 'contracts/src/generated/L1HistoryCode.sol': (
        '// SPDX-License-Identifier: MIT\npragma solidity 0.8.34;\n\n'
        '// Generated by scripts/update-eezl2-genesis.sh under canonical protocol paths.\n'
        '// Identifies the history proxy, including its immutable ProxyAdmin address.\n'
        'library L1HistoryCode {\n'
        f'    bytes32 internal constant HASH = {digest};\n'
        '}\n'
    ).encode(),
}
for path, content in outputs.items():
    if mode == 'update':
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
    elif not path.exists() or path.read_bytes() != content:
        raise SystemExit(f'stale generated history code identity: {path}; run scripts/update-eezl2-genesis.sh')
PY_HISTORY_CODE


genesis_state_root() {
    local genesis="$1"
    if [[ -n "$genesis_tool" ]]; then
        "$genesis_tool" "$genesis"
    else
        (
            cd "$REPO"
            cargo +1.95.0 run --quiet --locked --package eez-node --example genesis_state_root -- "$genesis"
        )
    fi
}

render_genesis() {
    local base="$1" destination="$2"
    [[ -f "$base" ]] || { echo "base genesis is missing: $base" >&2; exit 1; }
    mkdir -p "$(dirname "$destination")"
    local tmp
    tmp="$(mktemp "$build_root/genesis.XXXXXX")"
    if ! jq -e --arg predeploy "${EEZL2_ADDRESS,,}" \
        '.alloc | type == "object" and has($predeploy) and (.[$predeploy].code | type == "string")' \
        "$base" >/dev/null
    then
        echo "$base does not contain the EEZL2 proxy predeploy at $EEZL2_ADDRESS" >&2
        exit 1
    fi
    jq --arg system_address "${system_address,,}" --arg fee_collector "$fee_collector" \
        '.config.rollupX.feeCollector = $fee_collector | del(.alloc[$system_address])' "$base" >"$tmp"
    for account in "${!account_code[@]}"; do
        jq --arg account "$account" --arg code "${account_code[$account]}" \
            --arg nonce "$(printf '0x%x' "${account_nonce[$account]}")" \
            '.alloc[$account] = {balance: "0x0", nonce: $nonce, code: $code}' "$tmp" >"$tmp.next"
        mv "$tmp.next" "$tmp"
    done
    for key in "${!expected_storage[@]}"; do
        read -r account slot <<<"$key"
        [[ "${expected_storage[$key]}" != "$zero_slot" ]] || continue
        jq --arg account "$account" --arg slot "$slot" --arg value "${expected_storage[$key]}" \
            '.alloc[$account].storage[$slot] = $value' "$tmp" >"$tmp.next"
        mv "$tmp.next" "$tmp"
    done
    python3 - "$tmp" <<'PY_HISTORY_STATE'
import json
import sys
path, = sys.argv[1:]
with open(path) as f:
    genesis = json.load(f)
config = genesis['config']['rollupX']
reference = config['l1Reference']
def integer(value):
    if isinstance(value, bool):
        raise ValueError('boolean is not an integer')
    value = int(value, 0) if isinstance(value, str) else value
    if not isinstance(value, int) or not 0 <= value < 1 << 64:
        raise ValueError('history coordinate must fit uint64')
    return value
integer(reference['number'])  # Validate the logical origin without storing a bootstrap record.
cutoff = integer(genesis['timestamp'])
step = integer(config['blockIntervalSeconds'])
positions = integer(config['syncIntervalBlocks'])
q = integer(config['historyUpdateOffset'])
period, delay = step * positions, step * (q + 1)
if step == 0 or positions < 2 or q >= positions or period >= 1 << 64 or cutoff < period or cutoff + delay >= 1 << 64:
    raise SystemExit('invalid new-network history schedule')
if integer(reference['timestamp']) > cutoff - period:
    raise SystemExit('bootstrap source is after its required cutoff')
hash_bytes = bytes.fromhex(reference['hash'].removeprefix('0x'))
if len(hash_bytes) != 32 or not any(hash_bytes):
    raise SystemExit('invalid history bootstrap hash')
if genesis['mixHash'].lower() != reference['prevRandao'].lower():
    raise SystemExit('genesis PREVRANDAO differs from bootstrap source')
PY_HISTORY_STATE
    chmod --reference="$base" "$tmp"
    printf '%s\n' "$tmp"
}

write_profile() {
    local destination="$1" state_root="$2"
    mkdir -p "$(dirname "$destination")"
    local tmp
    tmp="$(mktemp "$build_root/profile.XXXXXX")"
    jq -n \
        --argjson rollup_id "$rollup_id" \
        --arg system_address "$system_address" \
        --arg recovery_address "$recovery_address" \
        --arg upgrade_owner "$upgrade_owner" \
        --arg fee_collector "$fee_collector" \
        --arg eezl2_address "$EEZL2_ADDRESS" \
        --arg treasury_owner "$treasury_owner" \
        --arg treasury_recipient "$treasury_recipient" \
        --arg fee_vault_runtime_hash "$fee_vault_runtime_hash" \
        --arg fee_impl "$fee_impl_key" \
        --arg fee_admin "$fee_admin_key" \
        --arg fee_impl_runtime_hash "$fee_impl_runtime_hash" \
        --arg history_address "$HISTORY_ADDRESS" \
        --arg history_impl "$history_impl_key" \
        --arg history_admin "$history_admin_key" \
        --arg history_runtime_hash "$history_runtime_hash" \
        --arg history_impl_runtime_hash "$history_impl_runtime_hash" \
        --arg implementation_address "$EEZL2_IMPLEMENTATION_ADDRESS" \
        --arg proxy_admin_address "$proxy_admin_address" \
        --arg runtime_hash "$runtime_hash" \
        --arg proxy_runtime_hash "$proxy_runtime_hash" \
        --arg state_root "$state_root" \
        --arg foundry_version "$EXPECTED_FOUNDRY_VERSION" \
        '{
            schemaVersion: 5,
            rollupId: $rollup_id,
            systemAddress: $system_address,
            recoveryAddress: $recovery_address,
            upgradeOwner: $upgrade_owner,
            feeCollector: $fee_collector,
            treasuryOwner: $treasury_owner,
            treasuryRecipient: $treasury_recipient,
            feeVaultRuntimeCodeHash: $fee_vault_runtime_hash,
            feeVaultImplementationAddress: $fee_impl,
            feeVaultProxyAdminAddress: $fee_admin,
            feeVaultImplementationCodeHash: $fee_impl_runtime_hash,
            historyAddress: $history_address,
            historyImplementationAddress: $history_impl,
            historyProxyAdminAddress: $history_admin,
            historyRuntimeCodeHash: $history_runtime_hash,
            historyImplementationCodeHash: $history_impl_runtime_hash,
            useGasLeft: false,
            eezL2Address: $eezl2_address,
            eezL2ImplementationAddress: $implementation_address,
            eezL2ProxyAdminAddress: $proxy_admin_address,
            runtimeCodeHash: $runtime_hash,
            proxyRuntimeCodeHash: $proxy_runtime_hash,
            genesisStateRoot: $state_root,
            foundryVersion: $foundry_version
        }' >"$tmp"
    if [[ -e "$destination" ]]; then
        chmod --reference="$destination" "$tmp"
    else
        chmod 0644 "$tmp"
    fi
    printf '%s\n' "$tmp"
}

if [[ "$mode" == "render" ]]; then
    rendered="$(render_genesis "$base_genesis" "$output")"
    state_root="$(genesis_state_root "$rendered")"
    mv "$rendered" "$output"
    if [[ -n "$profile_output" ]]; then
        rendered_profile="$(write_profile "$profile_output" "$state_root")"
        mv "$rendered_profile" "$profile_output"
    fi
    printf 'EEZ_L2_SYSTEM_ADDRESS=%s\n' "$system_address"
    printf 'EEZ_L2_RECOVERY_ADDRESS=%s\n' "$recovery_address"
    printf 'EEZ_L2_UPGRADE_OWNER=%s\n' "$upgrade_owner"
    printf 'EEZ_L2_TREASURY_OWNER=%s\n' "$treasury_owner"
    printf 'EEZ_L2_TREASURY_RECIPIENT=%s\n' "$treasury_recipient"
    printf 'EEZ_L2_IMPLEMENTATION_ADDRESS=%s\n' "$EEZL2_IMPLEMENTATION_ADDRESS"
    printf 'EEZ_L2_PROXY_ADMIN=%s\n' "$proxy_admin_address"
    printf 'EEZ_L2_EEZL2_CODE_HASH=%s\n' "$runtime_hash"
    printf 'EEZ_L2_GENESIS_STATE_ROOT=%s\n' "$state_root"
    exit 0
fi

# The profile describes the first file: the development genesis that Kurtosis
# deploys. genesis.json is the production base and must never allocate ETH:
# every L2 wei is minted against value locked on L1.
genesis_files=(
    "$REPO/genesis.dev.json"
    "$REPO/genesis.json"
    "$REPO/crates/eez-node-e2e/tests/fixtures/genesis.json"
)
rendered_files=()
for genesis in "${genesis_files[@]}"; do
    rendered_files+=("$(render_genesis "$genesis" "$genesis")")
done
if ! jq -e '[.alloc[] | .balance // "0x0" | ltrimstr("0x") | test("^0*$")] | all' \
    "${rendered_files[1]}" >/dev/null
then
    echo "production base genesis.json must not allocate ETH" >&2
    exit 1
fi
state_root="$(genesis_state_root "${rendered_files[0]}")"
rendered_profile="$(write_profile "$FIXTURE_PROFILE" "$state_root")"

if [[ "$mode" == "check" ]]; then
    stale=0
    for index in "${!genesis_files[@]}"; do
        if ! cmp -s "${rendered_files[$index]}" "${genesis_files[$index]}"; then
            echo "stale generated EEZL2 genesis: ${genesis_files[$index]}" >&2
            stale=1
        fi
    done
    if ! cmp -s "$rendered_profile" "$FIXTURE_PROFILE"; then
        echo "stale generated EEZL2 fixture profile: $FIXTURE_PROFILE" >&2
        stale=1
    fi
    if (( stale != 0 )); then
        echo "run scripts/update-eezl2-genesis.sh to update generated artifacts" >&2
        exit 1
    fi
    echo "EEZL2 genesis artifacts are current; state root: $state_root"
    exit 0
fi

for index in "${!genesis_files[@]}"; do
    mv "${rendered_files[$index]}" "${genesis_files[$index]}"
done
mv "$rendered_profile" "$FIXTURE_PROFILE"
echo "updated all EEZL2 genesis predeploys; state root: $state_root"
