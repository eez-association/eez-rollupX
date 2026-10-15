// Application-side advisory pricing. The wallet still receives one ordinary
// eth_sendTransaction. Its network RPC must route broadcasts to the EEZ front.
const intentKeys = ["type", "chainId", "from", "to", "nonce", "gas", "value", "data"];
const feeKeys = intent => quantity(intent.type) === 0n ? ["gasPrice"] : ["maxPriorityFeePerGas", "maxFeePerGas"];
const quantityKeys = ["type", "chainId", "nonce", "gas", "value", "gasPrice", "maxPriorityFeePerGas", "maxFeePerGas"];

function quantity(value) {
  if (typeof value !== "string" || !/^0x(?:0|[1-9a-f][0-9a-f]*)$/i.test(value) || value.length > 66) {
    throw new Error("Expected a canonical hex quantity");
  }
  return BigInt(value);
}

// Wallet middleware can return a numeric pending nonce or zero-padded hex.
// Normalize observations only; signed intents and server quotes stay canonical.
function walletQuantity(value, field) {
  if (typeof value === "number" && Number.isSafeInteger(value) && value >= 0) return BigInt(value);
  if (typeof value === "string" && /^0x[0-9a-f]{1,64}$/i.test(value)) return BigInt(value);
  throw new Error(`Invalid wallet ${field}: expected a hex quantity or non-negative safe integer`);
}

function exactKeys(value, keys) {
  if (!value || Array.isArray(value) || Object.keys(value).length !== keys.length || keys.some(key => !(key in value))) {
    throw new Error("Unexpected transaction fields");
  }
}

function validateIntent(intent) {
  exactKeys(intent, intentKeys);
  for (const key of quantityKeys.filter(key => key in intent)) quantity(intent[key]);
  if (![0n, 2n].includes(quantity(intent.type)) || quantity(intent.chainId) === 0n) throw new Error("Expected a legacy or type-2 chain-specific intent");
  for (const key of ["from", "to"]) {
    if (!/^0x[0-9a-f]{40}$/i.test(intent[key])) throw new Error("Invalid address");
  }
  if (typeof intent.data !== "string" || !/^0x(?:[0-9a-f]{2})*$/i.test(intent.data) || intent.data.length > 2 + 24 * 1024) throw new Error("Invalid calldata");
  if (quantity(intent.gas) < 21000n || quantity(intent.gas) > 5000000n) throw new Error("Gas outside quote profile");
}

function sameField(key, actual, expected) {
  return quantityKeys.includes(key) ? quantity(actual) === quantity(expected) :
    typeof actual === "string" && actual.toLowerCase() === expected.toLowerCase();
}

function decimal(value) {
  if (typeof value !== "string" || !/^(0|[1-9][0-9]*)$/.test(value) || value.length > 78) throw new Error("Invalid quote amount");
  return BigInt(value);
}

/** Validate exact intent, expiry, quoted coverage and the app's maximum L2
 * execution fee (gas limit * gasPrice or maxFeePerGas). Returns a fresh wallet transaction.
 * maxTotalFee is a bigint in L2 native base units; no floating-point currency.
 * The quote server and its conversion policy remain trusted advisory inputs.
 */
export function quotedTransaction({ quote, intent, maxTotalFee, now = Date.now(), expectedPolicyId }) {
  validateIntent(intent);
  if (typeof maxTotalFee !== "bigint" || maxTotalFee < 0n) throw new Error("A bigint maxTotalFee is required");
  if (quote?.version !== 1 || quote.mode !== "advisory") throw new Error("Unsupported quote version or mode");
  if (!Number.isSafeInteger(quote.issuedAt) || !Number.isSafeInteger(quote.expiresAt) ||
      quote.expiresAt <= quote.issuedAt || quote.expiresAt - quote.issuedAt > 10 ||
      now >= quote.expiresAt * 1000 || quote.issuedAt * 1000 > now + 1000) throw new Error("Quote expired or invalid clock");
  exactKeys(quote.intent, intentKeys);
  const fees = feeKeys(intent);
  exactKeys(quote.transaction, [...intentKeys, ...fees]);
  for (const key of intentKeys) {
    if (!sameField(key, quote.intent[key], intent[key]) || !sameField(key, quote.transaction[key], intent[key])) throw new Error(`Quote changed ${key}`);
  }
  const context = quote.context;
  if (!Number.isSafeInteger(context?.l2ChainId) || BigInt(context.l2ChainId) !== quantity(intent.chainId) ||
      !Number.isSafeInteger(context.rollupId) || context.rollupId < 1 ||
      context.l2ChainId !== quote.policy?.assumptions?.l2_chain_id || context.rollupId !== quote.policy.assumptions.rollup_id) throw new Error("Quote chain or policy mismatch");
  if (!/^0x[0-9a-f]{64}$/i.test(quote.policy.id) || (expectedPolicyId && quote.policy.id.toLowerCase() !== expectedPolicyId.toLowerCase())) throw new Error("Quote policy changed");
  const estimate = quote.estimate;
  const base = decimal(estimate?.l2BaseFeePerGas);
  const legacy = quantity(intent.type) === 0n;
  const cap = quantity(legacy ? quote.transaction.gasPrice : quote.transaction.maxFeePerGas);
  const tip = legacy ? cap - base : quantity(quote.transaction.maxPriorityFeePerGas);
  if (estimate?.basis !== "isolated_native_l2_replay_and_projected_l1_post" ||
      !Number.isSafeInteger(estimate.sourceGasUsed) || estimate.sourceGasUsed <= 0 ||
      BigInt(estimate.sourceGasUsed) > quantity(intent.gas) || tip < 0n || cap !== tip + base ||
      tip * BigInt(estimate.sourceGasUsed) !== decimal(estimate.grossL2Tip) ||
      decimal(estimate.grossL2Tip) < decimal(estimate.requiredL2Tip)) throw new Error("Inconsistent fee estimate");
  if (cap * quantity(intent.gas) > maxTotalFee) throw new Error("Quoted maximum fee exceeds app limit");
  // Copy only validated fields, never forward arbitrary server properties.
  return Object.fromEntries([...intentKeys, ...fees].map(key => [key, quote.transaction[key]]));
}

/** Fetch a quote through a Composer RPC function (method, params), or the
 * existing HTTP route's complete URL. Use a direct RPC client: injected wallets
 * may filter custom methods. Neither transport signs, reserves or submits.
 */
export async function requestL2FeeQuote({ rpc, url, intent, maxTotalFee, expectedPolicyId, signal, fetchImpl = globalThis.fetch }) {
  validateIntent(intent);
  if ((rpc == null) === (url == null) || (rpc != null && typeof rpc !== "function")) throw new Error("Supply either a Composer RPC function or an HTTP quote URL");
  let quote;
  if (rpc) {
    quote = await rpc("eez_quoteL2ToL1Fee", [intent]);
  } else {
    const response = await fetchImpl(url, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(intent), signal });
    quote = await response.json();
    if (!response.ok) throw new Error(`Fee quote ${quote?.error?.code ?? response.status}: ${quote?.error?.message ?? "unavailable"}`);
  }
  quotedTransaction({ quote, intent, maxTotalFee, expectedPolicyId });
  return quote;
}

/** Recheck an authorized EIP-1193 wallet's chain, account, pending nonce/base fee,
 * then request its one normal transaction signature. Expiry is checked before
 * opening the wallet; approval delays, wallet edits and later state changes
 * require a new quote and remain outside this advisory guarantee.
 */
export async function sendQuotedTransaction({ wallet, quote, intent, maxTotalFee, expectedPolicyId }) {
  const validate = () => quotedTransaction({ quote, intent, maxTotalFee, expectedPolicyId });
  validate();
  const [chainId, accounts, nonce, pending] = await Promise.all([
    wallet.request({ method: "eth_chainId" }),
    wallet.request({ method: "eth_accounts" }),
    wallet.request({ method: "eth_getTransactionCount", params: [intent.from, "pending"] }),
    wallet.request({ method: "eth_getBlockByNumber", params: ["pending", false] }),
  ]);
  if (walletQuantity(chainId, "chain ID") !== quantity(intent.chainId)) throw new Error("Wallet is on a different chain");
  if (!Array.isArray(accounts) || !accounts.some(account => typeof account === "string" && account.toLowerCase() === intent.from.toLowerCase())) throw new Error("Wallet account changed");
  if (walletQuantity(nonce, "pending nonce") !== quantity(intent.nonce)) throw new Error("Wallet nonce changed; request a new quote");
  if (walletQuantity(pending?.baseFeePerGas, "pending base fee") !== decimal(quote.estimate.l2BaseFeePerGas)) throw new Error("Base fee changed; request a new quote");
  return wallet.request({ method: "eth_sendTransaction", params: [validate()] });
}
