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
  if (quantity(intent.gas) < 21000n || quantity(intent.gas) > 16777216n) throw new Error("Gas outside quote profile");
}

function compactIntent(intent) {
  if (!intent || Array.isArray(intent) || Object.keys(intent).some(key => !intentKeys.includes(key))) throw new Error("Unexpected intent fields");
  const normalized = { type: "0x2", chainId: "0x1", nonce: "0x0", gas: "0x1000000", value: "0x0", data: "0x", ...intent };
  validateIntent(normalized);
  return Object.fromEntries(Object.entries(normalized).map(([key, value]) => [key,
    quantityKeys.includes(key) ? `0x${quantity(value).toString(16)}` : value.toLowerCase()]));
}

/** One Composer call; omitted gas/nonce are resolved by the server. Does not
 * sign or silently fall back to an unquoted transaction on old servers/errors.
 */
export async function requestFeeQuote({ rpc, intent, transactionTypes }) {
  compactIntent(intent);
  if (typeof rpc !== "function") throw new Error("A Composer RPC function is required");
  const types = transactionTypes ?? (intent.type ? [intent.type] : ["0x0", "0x2"]);
  if (!Array.isArray(types) || types.length < 1 || types.length > 2 ||
      types.some(type => !["0x0", "0x2"].includes(type)) || new Set(types).size !== types.length ||
      (intent.type && (types.length !== 1 || quantity(types[0]) !== quantity(intent.type)))) throw new Error("Invalid transactionTypes");
  const quote = await rpc("eez_quoteFee", [intent, { transactionTypes: types }]);
  if (!Array.isArray(quote?.options) || quote.options.length !== types.length ||
      quote.options.some((option, i) => option.type !== types[i])) throw new Error("Quote changed requested transaction types");
  return quote;
}

/** Select exactly one compact option and bind it to the caller's original
 * intent. keccak256 receives a UTF-8 string and returns its 0x-prefixed hash;
 * inject the application's existing Ethereum library (no extra SDK dependency).
 * chainId is the application's expected source network, not a server default.
 */
export function feeQuoteTransaction({ quote, intent, type, chainId, keccak256, maxTotalFee, now = Date.now() }) {
  const normalized = compactIntent(intent);
  if (quote?.version !== 1 || !["advisory", "enforced_tariff"].includes(quote.recoveryMode)) throw new Error("Unsupported fee quote");
  if (typeof maxTotalFee !== "bigint" || maxTotalFee < 0n) throw new Error("A bigint maxTotalFee is required");
  if (quantity(chainId) !== quantity(quote.chainId) || (intent.chainId && quantity(intent.chainId) !== quantity(chainId))) throw new Error("Quote chain mismatch");
  const expiry = quantity(quote.expiresAt) * 1000n;
  if (!Number.isSafeInteger(now) || expiry <= BigInt(now) || expiry > BigInt(now) + 11_000n) throw new Error("Quote expired or invalid clock");
  if (!["0x0", "0x2"].includes(type) || (intent.type && quantity(intent.type) !== quantity(type))) throw new Error("Quote type mismatch");
  if (!Array.isArray(quote.options) || quote.options.length < 1 || quote.options.length > 2 ||
      quote.options.some(option => !["0x0", "0x2"].includes(option.type)) ||
      new Set(quote.options.map(option => option.type)).size !== quote.options.length) throw new Error("Invalid quote options");
  const option = quote.options.find(option => option.type === type);
  if (!option || option.error) throw new Error(`Fee option unavailable: ${option?.error?.code ?? "missing"}`);
  const fees = feeKeys({ type });
  exactKeys(option, ["type", "gas", "intentHash", "estimatedFee", ...fees]);
  for (const key of ["gas", "nonce"]) {
    const resolved = key === "gas" ? option.gas : quote.nonce;
    if (intent[key] != null && quantity(intent[key]) !== quantity(resolved)) throw new Error(`Quote changed ${key}`);
    normalized[key] = `0x${quantity(resolved).toString(16)}`;
  }
  normalized.type = type;
  normalized.chainId = `0x${quantity(chainId).toString(16)}`;
  validateIntent(normalized);
  if (quantity(normalized.nonce) > (1n << 64n) - 1n || quantity(normalized.chainId) > (1n << 64n) - 1n) throw new Error("Quantity exceeds u64");
  const canonical = JSON.stringify(Object.fromEntries([...intentKeys].sort().map(key => [key, normalized[key]])));
  if (typeof keccak256 !== "function" || typeof option.intentHash !== "string" ||
      !/^0x[0-9a-f]{64}$/i.test(option.intentHash) || keccak256(canonical).toLowerCase() !== option.intentHash.toLowerCase()) throw new Error("Quote intent binding mismatch");
  const cap = quantity(type === "0x0" ? option.gasPrice : option.maxFeePerGas);
  if (cap > (1n << 128n) - 1n || (type === "0x2" && quantity(option.maxPriorityFeePerGas) > cap)) throw new Error("Invalid fee fields");
  const maximum = quantity(option.gas) * cap;
  if (quantity(option.estimatedFee) > maximum) throw new Error("Inconsistent estimated fee");
  if (maximum > maxTotalFee) throw new Error("Quoted maximum fee exceeds app limit");
  return { ...normalized, ...Object.fromEntries(fees.map(key => [key, option[key]])) };
}

/** Wallet approval can outlive the quote. Native admission remains authoritative;
 * a rejected/expired transaction needs a new quote, never an unquoted retry.
 */
export async function sendFeeQuotedTransaction({ wallet, ...args }) {
  const validate = () => feeQuoteTransaction({ ...args, now: Date.now() });
  const tx = validate();
  const [chain, accounts, nonce] = await Promise.all([
    wallet.request({ method: "eth_chainId" }), wallet.request({ method: "eth_accounts" }),
    wallet.request({ method: "eth_getTransactionCount", params: [tx.from, "pending"] }),
  ]);
  if (walletQuantity(chain, "chain ID") !== quantity(tx.chainId)) throw new Error("Wallet is on a different chain");
  if (!Array.isArray(accounts) || !accounts.some(account => typeof account === "string" && account.toLowerCase() === tx.from)) throw new Error("Wallet account changed");
  if (walletQuantity(nonce, "pending nonce") !== quantity(tx.nonce)) throw new Error("Wallet nonce changed; request a new quote");
  return wallet.request({ method: "eth_sendTransaction", params: [validate()] });
}
