# Geth/Reth callTracer result for the canonical user transaction. Require the
# actual registry frame and exact returned/reverted bytes, not a caller flag
# that would also be true after ExecutionNotFound or an out-of-gas error.
def lower: ascii_downcase;
def frames: ., (.calls[]? | frames);
# The registry is a transparent proxy: a call into it may contain only its own
# forwarding DELEGATECALL to the implementation (callTracer reports the
# implementation as `to`), with the same input and no further calls.
def only_forwarded($input):
  (.calls // []) | length == 0
    or (length == 1 and (.[0] | .type == "DELEGATECALL" and (.from | lower) == $registry
        and (.input | lower) == $input and (.error // "") == "" and ((.calls // []) | length) == 0));
# Callback inputs are optional: callback-free lookups omit them.
($ARGS.named.callback_proxy // "") as $callback_proxy
| ($ARGS.named.callback_target // "") as $callback_target
| ($ARGS.named.callback_input // "") as $callback_input
| ($ARGS.named.callback_output // "") as $callback_output
| ($ARGS.named.proxy_query_input // "") as $proxy_query_input
| ($ARGS.named.proxy_query_output // "") as $proxy_query_output
| select(.type == "CALL" and (.to | lower) == $source
       and (.input | lower) == $trigger_input and (.error // "") == "")
# The pinned proxy first reads the registry's detector gas cap, then performs
# the lookup. Account for both exact frames; arbitrary extra registry calls
# must not make a missing or failed lookup appear valid. The callback scenario's
# reader additionally queries the callback source proxy's address first: admit
# exactly that one read-only query, with its exact input and returned address.
| select(([frames | select((.to // "" | lower) == $registry)] | length)
    == (if $callback_proxy == "" then 2 else 3 end))
| select($callback_proxy == ""
    or ([.calls[]? | select((.to // "" | lower) == $registry)] as $queries
        | ($queries | length) == 1
        and ($queries[0] | .type == "STATICCALL" and (.from | lower) == $source
             and (.value // "0x0") == "0x0" and (.error // "") == ""
             and (.input | lower) == $proxy_query_input
             and (.output | lower) == $proxy_query_output
             and only_forwarded($proxy_query_input))))
| [.calls[]? | select((.to // "" | lower) == $proxy and (.from | lower) == $source)]
| if length != 1 then error("expected one direct call to the destination proxy") else .[0] end
| [.calls[]? | select((.to // "" | lower) == $registry)]
| if length != 2 then error("expected the gas-cap read and one executed registry lookup") else . end
| select(.[0] | .type == "STATICCALL" and (.from | lower) == $proxy
    and (.input | lower) == $static_check_gas_input and (.error // "") == ""
    and (.value // "0x0") == "0x0" and only_forwarded($static_check_gas_input)
    and (.output | test("^0x[0-9a-fA-F]{64}$")))
| .[1]
| select((.from | lower) == $proxy and (.value // "0x0") == "0x0"
    and (.input | lower) == $lookup_input
    and (if $static then .type == "STATICCALL" and (.error // "") == ""
         else .type == "CALL" and .error == "execution reverted" end)
    and (.output | lower) == $output)
# With a static callback, the lookup frame must contain exactly one call from
# the registry into the callback's source proxy, which forwards exactly one
# read to the L1 producer and returns the read result.
| select($callback_proxy == ""
    or ([frames | select((.to // "" | lower) == $callback_proxy)] as $callbacks
        | ($callbacks | length) == 1
        and ($callbacks[0] | .type == "STATICCALL" and (.from | lower) == $registry
             and (.error // "") == ""
             and ([.calls[]? | select((.to // "" | lower) == $callback_target)] as $reads
                  | ($reads | length) == 1
                  # The proxy forwards with CALL; the enclosing STATICCALL keeps it read-only.
                  and ($reads[0] | (.type == "STATICCALL" or .type == "CALL")
                       and (.value // "0x0") == "0x0" and (.from | lower) == $callback_proxy
                       and (.input | lower) == $callback_input and (.error // "") == ""
                       and (.output | lower) == $callback_output)))))
| {verified:true, registry:$registry, proxy:$proxy, input:.input,
   output:.output, reverted:($static | not), callback_traced:($callback_proxy != "")}
