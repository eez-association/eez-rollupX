#!/usr/bin/env bash
# Sourced by the staged runner: a shell function receives large arguments before
# execve can reject them. Other Forge commands retain their original arguments.
export EEZ_CHIADO_CONTRACTS_ROOT
EEZ_CHIADO_CONTRACTS_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../contracts" && pwd)"

forge() (
    local -a args=("$@")
    local i input_file
    if [[ ${args[0]:-} == script &&
          ${args[1]:-} == script/e2e/scenarios/shared/Verify.s.sol:VerifyL1BatchCalldata ]]; then
        for ((i = 2; i + 2 < ${#args[@]}; i++)); do
            if [[ ${args[i]} == --sig &&
                  ${args[i+1]} == 'run(bytes,address,bytes,bytes,bytes,bool)' &&
                  ${#args[i+2]} -ge 120000 ]]; then
                # Foundry permits reading its artifact directory. The file holds
                # the exact public calldata; expected tables stay unchanged.
                umask 077
                mkdir -p "$EEZ_CHIADO_CONTRACTS_ROOT/out"
                input_file=$(mktemp "$EEZ_CHIADO_CONTRACTS_ROOT/out/eez-batch-input.XXXXXX") || exit
                trap 'rm -f -- "$input_file"' EXIT
                printf '%s' "${args[i+2]}" > "$input_file" || exit
                args[1]="$EEZ_CHIADO_CONTRACTS_ROOT/script/VerifyL1BatchFromFile.s.sol:VerifyL1BatchFromFile"
                args[i+1]='runFromFile(string,address,bytes,bytes,bytes,bool)'
                args[i+2]="$input_file"
                command forge "${args[@]}" --root "$EEZ_CHIADO_CONTRACTS_ROOT"
                exit $?
            fi
        done
    fi
    command forge "$@"
)
export -f forge
