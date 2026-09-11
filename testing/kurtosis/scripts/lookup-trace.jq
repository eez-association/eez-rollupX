# Geth/Reth callTracer result for the canonical user transaction. Require the
# actual registry frame and exact returned/reverted bytes, not a caller flag
# that would also be true after ExecutionNotFound or an out-of-gas error.
def lower: ascii_downcase;
def frames: ., (.calls[]? | frames);
select(.type == "CALL" and (.to | lower) == $source
       and (.input | lower) == $trigger_input and (.error // "") == "")
| select(([frames | select((.to // "" | lower) == $registry)] | length) == 1)
| [.calls[]? | select((.to // "" | lower) == $proxy and (.from | lower) == $source)]
| if length != 1 then error("expected one direct call to the destination proxy") else .[0] end
| [.calls[]? | select((.to // "" | lower) == $registry)]
| if length != 1 then error("expected exactly one executed registry lookup") else .[0] end
| select((.from | lower) == $proxy and (.value // "0x0") == "0x0"
    and (.input | lower) == $lookup_input
    and (if $static then .type == "STATICCALL" and (.error // "") == ""
         else .type == "CALL" and .error == "execution reverted" end)
    and (.output | lower) == $output)
| {verified:true, registry:$registry, proxy:$proxy, input:.input,
   output:.output, reverted:($static | not)}
