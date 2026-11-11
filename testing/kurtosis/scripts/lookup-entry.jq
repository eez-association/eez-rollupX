# Input: cast decode-calldata --json postAndVerifyBatch(...).
# Roots are bound later to authenticated blocks or independently reconstructed
# execution prefixes. The actual call trace proves which lookup was resolved.
def zero_hash: "0x" + ("0" * 64);
def hash: type == "string" and test("^0x[0-9a-fA-F]{64}$");
.[0] as $batch
| if $scenario == "staticLocalWrite" then
    # Local writes leave the key and root unchanged. Preserve pool order and
    # every complete row, including identical duplicates; each read restarts
    # at the first matching candidate (STATIC_ENTRY §4.4).
    [$batch[2][$batch[4]:][] | select((.[1] | ascii_downcase) == $key)] as $rows
    | select(($rows | length) >= 2)
    | select(all($rows[];
        length == 7 and (.[0] | length) == 1 and (.[0][0] | length) == 2 and .[0][0][0] == $rollup
        and (.[0][0][1] | hash) and .[0] == $rows[0][0]
        and (.[2] | length) == 1
        and (.[2][0] | length == 8 and .[0] == 0 and .[1] == true and .[2] == 0
             and (.[3] | ascii_downcase) == $callback_source and .[4] == $rollup
             and (.[5] | ascii_downcase) == $callback_target and .[6] == 0
             and (.[7] | ascii_downcase) == $callback_data)
        and .[4] == $rollup and .[5] == true
        and (((.[6] | ascii_downcase) == $result and
              (.[3] | ascii_downcase) == $callback_rolling_hash)
          or ((.[6] | ascii_downcase) == $result_two and
              (.[3] | ascii_downcase) == $callback_rolling_hash_two))))
    | [$rows[] | .[6] | ascii_downcase] as $results
    | select($results[0] == $result and ($results | index($result_two)) != null)
    | {proxy_entry_hash: $key, root: $rows[0][0][0][1], candidates: $rows,
       # These indices refer to same-key candidates, not unrelated pool rows.
       selected_candidate_indices: [0, ($results | index($result_two)), 0],
       return_data: [$result, $result_two, $result]}
  elif $scenario == "topLevelStaticReentrantCounter" then
    # The same lookup, with exactly the one expected static callback to L1 and
    # its untagged result fold.
    [$batch[2][$batch[4]:][] | select((.[1] | ascii_downcase) == $key)]
    | if length != 1 then error("expected one static lookup") else .[0] end
    | select(length == 7 and (.[0] | length) == 1 and .[0][0][0] == $rollup
             and (.[0][0][1] | hash) and (.[2] | length) == 1
             and (.[2][0] | length == 8 and .[0] == 0 and .[1] == true and .[2] == 0
                  and (.[3] | ascii_downcase) == $callback_source and .[4] == $rollup
                  and (.[5] | ascii_downcase) == $callback_target and .[6] == 0
                  and (.[7] | ascii_downcase) == $callback_data)
             and (.[3] | ascii_downcase) == $callback_rolling_hash
             and .[4] == $rollup and .[5] == true and (.[6] | ascii_downcase) == $result)
    | {proxy_entry_hash: .[1], root: .[0][0][1], success: .[5], return_data: .[6],
       callbacks: .[2], rolling_hash: .[3]}
  elif $scenario == "topLevelStaticCounter" then
    # The user trigger resolves the persistent pool, not the meta-hook prefix.
    [$batch[2][$batch[4]:][] | select((.[1] | ascii_downcase) == $key)]
    | if length != 1 then error("expected one static lookup") else .[0] end
    | select(length == 7 and (.[0] | length) == 1 and .[0][0][0] == $rollup
             and (.[0][0][1] | hash) and .[2] == [] and .[3] == zero_hash
             and .[4] == $rollup and .[5] == true and (.[6] | ascii_downcase) == $result)
    | {proxy_entry_hash: .[1], root: .[0][0][1], success: .[5], return_data: .[6]}
  elif $scenario == "revertCounter" then
    # Shared queues are allowed, but the scenario's identity must be unique.
    # Trace verification separately rejects a caught wrong-lookup revert.
    [$batch[1][$batch[3]:][] | select(.[5] == $rollup and (.[1] | ascii_downcase) == $key)]
    | if length != 1 then error("expected one reverting lookup") else .[0] end
    | select(length == 8 and (.[1] | ascii_downcase) == $key
             and (.[0] | length) == 1 and .[0][0][0] == $rollup
             and .[0][0][1] == 0 and (.[0][0][2] | hash)
             and .[0][0][2] == .[0][0][3] and .[2] == [] and .[3] == []
             and (.[4] | hash) and .[5] == $rollup and .[6] == false and (.[7] | ascii_downcase) == $result)
    | {proxy_entry_hash: .[1], root: .[0][0][2], rolling_hash: .[4], success: .[6], return_data: .[7]}
  else error("unsupported lookup scenario") end
