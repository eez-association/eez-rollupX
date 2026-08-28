import { useCallback, useEffect, useRef, useState } from "react";
import {
  createWalletClient,
  defineChain,
  http,
  type Address,
  type Hex,
} from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { config, L1_CHAIN, L2_CHAIN } from "../config";
import { rpcCall } from "../rpc";
import type { WalletState } from "../types";

type Logger = (msg: string, type?: "ok" | "err" | "info") => void;

type LocalAccount = ReturnType<typeof privateKeyToAccount>;

export function useWallet(log: Logger, configLoaded = false) {
  const [state, setState] = useState<WalletState>({
    address: null,
    chainId: null,
    l1Balance: null,
    l2Balance: null,
    isConnected: false,
  });

  const stateRef = useRef(state);
  stateRef.current = state;
  const localAccountRef = useRef<LocalAccount | null>(null);

  const hasProvider = typeof window.ethereum !== "undefined";

  /**
   * The disposable local signer remains a fallback for browsers without an
   * injected wallet. Rabby/MetaMask use the composer-backed chain definitions;
   * the composer now forwards ordinary transactions automatically.
   */
  const sendLocalTx = useCallback(
    async (
      rpcUrl: string,
      chainDef: typeof L1_CHAIN | typeof L2_CHAIN,
      txParams: Record<string, string>,
    ): Promise<string> => {
      const account = localAccountRef.current;
      if (!account) throw new Error("Local demo signer is not configured");

      const chain = defineChain({
        id: Number(BigInt(chainDef.chainId)),
        name: chainDef.chainName,
        nativeCurrency: chainDef.nativeCurrency,
        rpcUrls: { default: { http: [rpcUrl] } },
      });
      const client = createWalletClient({
        account,
        chain,
        transport: http(rpcUrl),
      });
      const gas = txParams.gas ?? txParams.gasLimit;
      const request = {
        account,
        chain,
        ...(txParams.to ? { to: txParams.to as Address } : {}),
        ...(txParams.data ? { data: txParams.data as Hex } : {}),
        ...(txParams.value ? { value: BigInt(txParams.value) } : {}),
        ...(gas ? { gas: BigInt(gas) } : {}),
      };
      return client.sendTransaction(request);
    },
    [],
  );

  const refreshBalance = useCallback(async (address: string) => {
    // Fetch L1 and L2 balances in parallel
    const [l1Result, l2Result] = await Promise.allSettled([
      rpcCall(config.l1Rpc, "eth_getBalance", [address, "latest"]),
      rpcCall(config.l2Rpc, "eth_getBalance", [address, "latest"]),
    ]);

    // Use BigInt for precision — parseInt + /1e18 loses precision for
    // values above ~2^53 wei (which includes the CCM's 1M ETH genesis pre-mint).
    const formatEth = (hex: string): string => {
      try {
        const wei = BigInt(hex);
        const ONE_ETH = 10n ** 18n;
        const whole = wei / ONE_ETH;
        // Cap unreasonable balances (e.g. CCM with 1M ETH pre-mint)
        if (whole > 10n ** 9n) return "∞";
        const frac = wei % ONE_ETH;
        const fracStr = frac.toString().padStart(18, "0").slice(0, 4);
        return `${whole.toString()}.${fracStr}`;
      } catch {
        return "—";
      }
    };

    const l1Bal = l1Result.status === "fulfilled" ? formatEth(l1Result.value as string) : null;
    const l2Bal = l2Result.status === "fulfilled" ? formatEth(l2Result.value as string) : null;

    setState((s) => ({ ...s, l1Balance: l1Bal, l2Balance: l2Bal }));
  }, []);

  const connect = useCallback(async () => {
    if (!hasProvider) {
      if (config.demoPrivateKey) {
        try {
          const account = privateKeyToAccount(config.demoPrivateKey as Hex);
          localAccountRef.current = account;
          setState({
            address: account.address,
            chainId: L1_CHAIN.chainId,
            l1Balance: null,
            l2Balance: null,
            isConnected: true,
          });
          refreshBalance(account.address);
          log(`Local demo signer connected: ${account.address.slice(0, 8)}...${account.address.slice(-6)}`, "info");
          return;
        } catch (e) {
          log(`Local demo signer failed: ${(e as Error).message}`, "err");
          return;
        }
      }
      log("No wallet detected — install Rabby or MetaMask to connect", "err");
      return;
    }
    try {
      localAccountRef.current = null;
      const accounts = (await window.ethereum!.request({
        method: "eth_requestAccounts",
      })) as string[];
      const addr = accounts[0];
      if (!addr) return;

      const chainId = (await window.ethereum!.request({
        method: "eth_chainId",
      })) as string;

      setState({
        address: addr,
        chainId,
        l1Balance: null,
        l2Balance: null,
        isConnected: true,
      });
      localStorage.setItem("walletConnected", "true");
      log(
        `Wallet connected: ${addr.slice(0, 8)}...${addr.slice(-6)}`,
        "info",
      );
      refreshBalance(addr);
    } catch (e) {
      log(`Wallet connect failed: ${(e as Error).message}`, "err");
    }
  }, [hasProvider, log, refreshBalance]);

  const disconnect = useCallback(() => {
    localAccountRef.current = null;
    setState({
      address: null,
      chainId: null,
      l1Balance: null,
      l2Balance: null,
      isConnected: false,
    });
    localStorage.removeItem("walletConnected");
    log("Wallet disconnected", "info");
  }, [log]);

  const switchChain = useCallback(
    async (chainId: string, chainDef: typeof L1_CHAIN | typeof L2_CHAIN) => {
      if (!state.isConnected) {
        log("Connect wallet first", "err");
        return;
      }
      if (localAccountRef.current) {
        setState((current) => ({ ...current, chainId }));
        return;
      }
      try {
        // Try adding the chain first (works with Rabby, MetaMask, and others).
        // If the chain already exists, most wallets silently ignore this.
        await window.ethereum!.request({
          method: "wallet_addEthereumChain",
          params: [chainDef],
        });
      } catch {
        // Some wallets reject addEthereumChain for already-known chains — ignore
      }
      try {
        await window.ethereum!.request({
          method: "wallet_switchEthereumChain",
          params: [{ chainId }],
        });
      } catch (e) {
        log(`Switch chain failed: ${(e as Error).message}`, "err");
      }
    },
    [state.isConnected, log],
  );

  const switchToL1 = useCallback(
    () => switchChain(L1_CHAIN.chainId, L1_CHAIN),
    [switchChain],
  );
  const switchToL2 = useCallback(
    () => switchChain(L2_CHAIN.chainId, L2_CHAIN),
    [switchChain],
  );

  /**
   * Ensure the wallet is on the right chain, auto-switching if needed.
   * Rabby and MetaMask both support wallet_addEthereumChain + wallet_switchEthereumChain.
   */
  const ensureChain = useCallback(
    async (chainDef: typeof L1_CHAIN | typeof L2_CHAIN) => {
      if (localAccountRef.current) {
        setState((current) => ({ ...current, chainId: chainDef.chainId }));
        return;
      }
      if (stateRef.current.chainId === chainDef.chainId) return;
      try {
        await window.ethereum!.request({
          method: "wallet_addEthereumChain",
          params: [chainDef],
        });
      } catch {
        // Chain may already exist — ignore
      }
      await window.ethereum!.request({
        method: "wallet_switchEthereumChain",
        params: [{ chainId: chainDef.chainId }],
      });
    },
    [],
  );

  /**
   * Prepare tx params for wallet submission.
   * Ensures gas is passed as both `gas` and `gasLimit` for maximum wallet compatibility
   * (MetaMask uses `gas`, some wallets use `gasLimit`).
   */
  function prepareWalletParams(
    txParams: Record<string, string>,
    from: string,
  ): Record<string, string> {
    const params: Record<string, string> = { ...txParams, from };
    // Wallets vary on whether they read `gas` or `gasLimit` — set both
    if (params.gas && !params.gasLimit) {
      params.gasLimit = params.gas;
    }
    return params;
  }

  /**
   * Send a tx to L2.
   *
   * Requires a connected wallet — auto-switches to L2 chain and routes through wallet.
   */
  const sendTx = useCallback(
    async (txParams: Record<string, string>): Promise<string> => {
      if (!stateRef.current.isConnected) {
        throw new Error("Connect wallet to send transactions");
      }
      if (localAccountRef.current) {
        return sendLocalTx(config.l2Rpc, L2_CHAIN, txParams);
      }
      await ensureChain(L2_CHAIN);
      return (await window.ethereum!.request({
        method: "eth_sendTransaction",
        params: [prepareWalletParams(txParams, stateRef.current.address!)],
      })) as string;
    },
    [ensureChain, sendLocalTx],
  );

  /**
   * Send a tx to L1.
   *
   * Requires a connected wallet — auto-switches to L1. Injected wallets use
   * the classifying front; the fallback signer sends ordinary work directly.
   */
  const sendL1Tx = useCallback(
    async (txParams: Record<string, string>): Promise<string> => {
      if (!stateRef.current.isConnected) {
        throw new Error("Connect wallet to send transactions");
      }
      if (localAccountRef.current) {
        return sendLocalTx(config.l1Rpc, L1_CHAIN, txParams);
      }
      await ensureChain(L1_CHAIN);
      return (await window.ethereum!.request({
        method: "eth_sendTransaction",
        params: [prepareWalletParams(txParams, stateRef.current.address!)],
      })) as string;
    },
    [ensureChain, sendLocalTx],
  );

  /**
   * Send a tx to L1 via the L1 RPC proxy (port 9556).
   *
   * MUST be used for cross-chain calls — the proxy traces the tx,
   * detects executeCrossChainCall, populates the L2 execution table,
   * then forwards to L1. Without the proxy, the execution table is
   * empty and the tx reverts with ExecutionNotFound.
   *
   * Injected wallets already use the classifying front for this chain. The
   * fallback signer chooses the composer endpoint for this transaction only.
   */
  const sendL1ProxyTx = useCallback(
    async (txParams: Record<string, string>): Promise<string> => {
      if (!stateRef.current.isConnected) {
        throw new Error("Connect wallet to send transactions");
      }
      if (localAccountRef.current) {
        return sendLocalTx(config.l1ProxyRpc, L1_CHAIN, txParams);
      }
      await ensureChain(L1_CHAIN);
      return (await window.ethereum!.request({
        method: "eth_sendTransaction",
        params: [prepareWalletParams(txParams, stateRef.current.address!)],
      })) as string;
    },
    [ensureChain, sendLocalTx],
  );

  /**
   * Send a tx to L2 via the L2 RPC proxy (port 9548).
   *
   * MUST be used for L2→L1 cross-chain calls — the composer detects
   * executeCrossChainCall via trace, queues entries BEFORE forwarding
   * the tx to the builder (hold-then-forward pattern).
   */
  const sendL2ProxyTx = useCallback(
    async (txParams: Record<string, string>): Promise<string> => {
      if (!stateRef.current.isConnected) {
        throw new Error("Connect wallet to send transactions");
      }
      if (localAccountRef.current) {
        return sendLocalTx(config.l2ProxyRpc, L2_CHAIN, txParams);
      }
      await ensureChain(L2_CHAIN);
      return (await window.ethereum!.request({
        method: "eth_sendTransaction",
        params: [prepareWalletParams(txParams, stateRef.current.address!)],
      })) as string;
    },
    [ensureChain, sendLocalTx],
  );

  // Browsers without Rabby/MetaMask fall back to the disposable Kurtosis key.
  // An injected wallet always remains the primary signer.
  useEffect(() => {
    if (
      !configLoaded ||
      hasProvider ||
      !config.demoPrivateKey ||
      localAccountRef.current
    ) {
      return;
    }
    try {
      const account = privateKeyToAccount(config.demoPrivateKey as Hex);
      localAccountRef.current = account;
      setState({
        address: account.address,
        chainId: L1_CHAIN.chainId,
        l1Balance: null,
        l2Balance: null,
        isConnected: true,
      });
      refreshBalance(account.address);
      log(
        `Local Kurtosis signer ready: ${account.address.slice(0, 8)}...${account.address.slice(-6)}`,
        "info",
      );
    } catch (e) {
      log(`Invalid local demo signer: ${(e as Error).message}`, "err");
    }
  }, [configLoaded, hasProvider, log, refreshBalance]);

  // Auto-reconnect on mount
  useEffect(() => {
    if (
      hasProvider &&
      localStorage.getItem("walletConnected") === "true"
    ) {
      (async () => {
        try {
          const accounts = (await window.ethereum!.request({
            method: "eth_accounts",
          })) as string[];
          const addr = accounts[0];
          if (!addr) return;

          const chainId = (await window.ethereum!.request({
            method: "eth_chainId",
          })) as string;

          setState({
            address: addr,
            chainId,
            l1Balance: null,
            l2Balance: null,
            isConnected: true,
          });
          refreshBalance(addr);
        } catch {
          /* silent */
        }
      })();
    }
  }, [hasProvider, refreshBalance]);

  // Listen for account/chain changes
  useEffect(() => {
    if (!hasProvider) return;
    const eth = window.ethereum!;

    const onAccountsChanged = ((...args: unknown[]) => {
      const accounts = args[0] as string[];
      if (accounts.length === 0) {
        disconnect();
      } else {
        setState((s) => ({ ...s, address: accounts[0]! }));
        refreshBalance(accounts[0]!);
      }
    }) as (...args: unknown[]) => void;

    const onChainChanged = ((...args: unknown[]) => {
      const chainId = args[0] as string;
      setState((s) => ({ ...s, chainId }));
    }) as (...args: unknown[]) => void;

    eth.on("accountsChanged", onAccountsChanged);
    eth.on("chainChanged", onChainChanged);
    return () => {
      eth.removeListener("accountsChanged", onAccountsChanged);
      eth.removeListener("chainChanged", onChainChanged);
    };
  }, [hasProvider, disconnect, refreshBalance]);

  // Periodic balance refresh
  useEffect(() => {
    if (!state.isConnected || !state.address) return;
    const interval = setInterval(() => refreshBalance(state.address!), 10000);
    return () => clearInterval(interval);
  }, [state.isConnected, state.address, refreshBalance]);

  return {
    ...state,
    hasProvider: hasProvider || Boolean(localAccountRef.current),
    connect,
    disconnect,
    switchToL1,
    switchToL2,
    sendTx,
    sendL2ProxyTx,
    sendL1Tx,
    sendL1ProxyTx,
  };
}
