/** Centralized configuration — overridable via URL params */

const params = new URLSearchParams(window.location.search);
const ORIGIN = window.location.origin;
const absolute = (path: string) => new URL(path, ORIGIN).toString();

export const config = {
  /** L1 RPC endpoint */
  l1Rpc: params.get("l1") || absolute("/rpc/l1"),
  /** L2 RPC endpoint (builder) */
  l2Rpc: params.get("l2") || absolute("/rpc/l2"),
  /** L1 composer RPC — used only for cross-chain transaction submission */
  l1ProxyRpc: params.get("l1proxy") || absolute("/composer/l1"),
  /** L2 composer RPC — used only for L2→L1 cross-chain transaction submission */
  l2ProxyRpc: params.get("l2proxy") || absolute("/composer/l2"),

  /** BasedRollup contract address — loaded from /shared/rollup.env or URL param */
  rollupsAddress: params.get("rollups") || "",
  /** Rollup ID for state root queries */
  rollupId: params.get("rollupId") || "1",
  /** Block explorer base URLs (Blockscout frontends) */
  l1Explorer: params.get("l1explorer") || "",
  l2Explorer: params.get("l2explorer") || "",
  /** Blockscout backend API (for ABI fetching etc.) */
  l2ExplorerApi: params.get("l2explorerapi") || "",
  /** Bridge contract addresses */
  l1Bridge: params.get("l1bridge") || "",
  l2Bridge: params.get("l2bridge") || "",
  /** Flash loan contract addresses (loaded from rollup.env) */
  flashExecutorL1: params.get("flashExecutorL1") || "",
  flashTokenAddress: params.get("flashTokenAddress") || "",
  flashPoolAddress: params.get("flashPoolAddress") || "",
  flashNftAddress: params.get("flashNftAddress") || "",
  flashExecutorL2: params.get("flashExecutorL2") || "",
  flashWrappedTokenL2: params.get("flashWrappedTokenL2") || "",
  /** Reverse flash loan contract addresses (L2→L1 direction, loaded from rollup.env) */
  reverseExecutorL2: params.get("reverseExecutorL2") || "",
  reverseNftL1: params.get("reverseNftL1") || "",
  reverseExecutorL1: params.get("reverseExecutorL1") || "",
  /** Faucet address — loaded from /shared/rollup.env or URL param */
  faucetAddress: params.get("faucetAddress") || "",
  /** Local-only Kurtosis faucet signer; never configured on public networks. */
  demoPrivateKey: "",
  /** L2 CrossChainManager address — loaded from rollup.env */
  ccmL2Address: params.get("ccmL2") || "",
  /** Aggregator contract addresses (loaded from rollup.env) */
  aggWeth: params.get("aggWeth") || "",
  aggUsdc: params.get("aggUsdc") || "",
  aggL1Amm: params.get("aggL1Amm") || "",
  aggAggregator: params.get("aggAggregator") || "",
  aggL2Executor: params.get("aggL2Executor") || "",
  aggL2Amm: params.get("aggL2Amm") || "",
  aggL2ExecutorProxy: params.get("aggL2ExecutorProxy") || "",
  aggWrappedWethL2: params.get("aggWrappedWethL2") || "",
  aggWrappedUsdcL2: params.get("aggWrappedUsdcL2") || "",
};

/** Mutable — set after loading env files */
export function setConfig(updates: Partial<typeof config>) {
  Object.assign(config, updates);
  if (updates.l1ProxyRpc) L1_CHAIN.rpcUrls = [updates.l1ProxyRpc];
  if (updates.l2ProxyRpc) L2_CHAIN.rpcUrls = [updates.l2ProxyRpc];
}

/** L1 chain definition for wallet_addEthereumChain — populated at runtime */
export const L1_CHAIN = {
  chainId: "0x539", // default 1337, auto-detected on init
  chainName: "Based Rollup L1",
  rpcUrls: [config.l1ProxyRpc],
  nativeCurrency: { name: "Ether", symbol: "ETH", decimals: 18 },
};

/** L2 chain definition for wallet_addEthereumChain — populated at runtime.
 * The composer classifies signed transactions, forwarding ordinary work to the
 * mempool and holding only L2→L1 cross-chain calls. */
export const L2_CHAIN = {
  chainId: "0xa455", // default 42069, auto-detected on init
  chainName: "Based Rollup L2",
  rpcUrls: [config.l2ProxyRpc],
  nativeCurrency: { name: "Ether", symbol: "ETH", decimals: 18 },
};

/** Counter contract bytecode (SimpleCounter: count, increment, getCount) */
export const COUNTER_BYTECODE =
  "0x6080604052348015600e575f5ffd5b506101778061001c5f395ff3fe608060405234801561000f575f5ffd5b506004361061003f575f3560e01c806306661abd14610043578063a87d942c14610061578063d09de08a1461007f575b5f5ffd5b61004b610089565b60405161005891906100c8565b60405180910390f35b61006961008e565b60405161007691906100c8565b60405180910390f35b610087610096565b005b5f5481565b5f5f54905090565b60015f5f8282546100a7919061010e565b92505081905550565b5f819050919050565b6100c2816100b0565b82525050565b5f6020820190506100db5f8301846100b9565b92915050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52601160045260245ffd5b5f610118826100b0565b9150610123836100b0565b925082820190508082111561013b5761013a6100e1565b5b9291505056fea2646970667358221220928ed30d80bb25597bae15bbba9d2ddff597e73d3b457921d19fb425f63c421464736f6c63430008210033";

/** Counter ABI selectors */
export const COUNTER_ABI = {
  increment: "0xd09de08a",
  getCount: "0xa87d942c",
} as const;

/** Well-known anvil/reth dev account #4 — used as `from` in read-only eth_call estimation (gas, balances).
 *  Never signs transactions; only provides a funded address for simulation. */
export const ESTIMATION_SENDER = "0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65";
