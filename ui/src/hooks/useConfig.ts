import { useEffect, useState } from "react";
import { config, setConfig, L1_CHAIN, L2_CHAIN } from "../config";
import { rpcCall } from "../rpc";
import { registerContractsFromEnv } from "../lib/addressBook";

type RuntimeConfig = {
  l1RpcUrl?: string;
  l2RpcUrl?: string;
  l1FrontUrl?: string;
  l2FrontUrl?: string;
  l1ExplorerUrl?: string;
  l2ExplorerUrl?: string;
  l2ExplorerApiUrl?: string;
  l1ContractAddress?: string;
  l2ContractAddress?: string;
  rollupId?: string;
  demoBridgeAddress?: string;
  demoPrivateKey?: string;
  demoTokenAddress?: string;
  demoPoolAddress?: string;
  demoExecutorL1?: string;
  demoExecutorL2?: string;
  demoWrappedTokenL2?: string;
  demoNftL2?: string;
};

function absoluteUrl(value: string | undefined): string | undefined {
  return value ? new URL(value, window.location.origin).toString() : undefined;
}

function parseEnv(text: string): Record<string, string> {
  const result: Record<string, string> = {};
  for (const line of text.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const eq = trimmed.indexOf("=");
    if (eq < 0) continue;
    result[trimmed.slice(0, eq).trim()] = trimmed.slice(eq + 1).trim();
  }
  return result;
}

/** Loads config from /shared/ env files and auto-detects chain IDs from RPCs */
export function useConfigLoader() {
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    (async () => {
      // Current EEZ/Kurtosis runtime configuration. The mapping below is the
      // compatibility layer between the POC UI names and the current network.
      try {
        const response = await fetch("/config.json");
        if (response.ok) {
          const payload = (await response.json()) as RuntimeConfig & { browser?: RuntimeConfig };
          const runtime: RuntimeConfig = payload.browser ?? payload;
          const l1Rpc = absoluteUrl(runtime.l1RpcUrl);
          const l2Rpc = absoluteUrl(runtime.l2RpcUrl);
          const l1ProxyRpc = absoluteUrl(runtime.l1FrontUrl);
          const l2ProxyRpc = absoluteUrl(runtime.l2FrontUrl);
          setConfig({
            ...(l1Rpc ? { l1Rpc } : {}),
            ...(l2Rpc ? { l2Rpc } : {}),
            ...(l1ProxyRpc ? { l1ProxyRpc } : {}),
            ...(l2ProxyRpc ? { l2ProxyRpc } : {}),
            ...(runtime.l1ExplorerUrl ? { l1Explorer: runtime.l1ExplorerUrl } : {}),
            ...(runtime.l2ExplorerUrl ? { l2Explorer: runtime.l2ExplorerUrl } : {}),
            ...(runtime.l2ExplorerApiUrl ? { l2ExplorerApi: runtime.l2ExplorerApiUrl } : {}),
            ...(runtime.l1ContractAddress ? { rollupsAddress: runtime.l1ContractAddress } : {}),
            ...(runtime.rollupId ? { rollupId: runtime.rollupId } : {}),
            ...(runtime.demoBridgeAddress ? {
              l1Bridge: runtime.demoBridgeAddress,
              l2Bridge: runtime.demoBridgeAddress,
            } : {}),
            ...(runtime.demoPrivateKey ? { demoPrivateKey: runtime.demoPrivateKey } : {}),
            ...(runtime.demoExecutorL1 ? { flashExecutorL1: runtime.demoExecutorL1 } : {}),
            ...(runtime.demoTokenAddress ? { flashTokenAddress: runtime.demoTokenAddress } : {}),
            ...(runtime.demoPoolAddress ? { flashPoolAddress: runtime.demoPoolAddress } : {}),
            ...(runtime.demoNftL2 ? { flashNftAddress: runtime.demoNftL2 } : {}),
            ...(runtime.demoExecutorL2 ? { flashExecutorL2: runtime.demoExecutorL2 } : {}),
            ...(runtime.demoWrappedTokenL2 ? { flashWrappedTokenL2: runtime.demoWrappedTokenL2 } : {}),
            ...(runtime.l2ContractAddress ? { ccmL2Address: runtime.l2ContractAddress } : {}),
          });
        }
      } catch {
        /* runtime config not present; URL parameters and legacy env remain valid */
      }

      // Load env file (unified rollup.env written by deploy.sh)
      try {
        const resp = await fetch("/shared/rollup.env");
        if (resp.ok) {
          const env = parseEnv(await resp.text());
          if (!config.rollupsAddress && env["ROLLUPS_ADDRESS"]) {
            setConfig({ rollupsAddress: env["ROLLUPS_ADDRESS"] });
          }
          if (env["ROLLUP_ID"]) {
            setConfig({ rollupId: env["ROLLUP_ID"] });
          }
          if (!config.l1Bridge && env["BRIDGE_L1_ADDRESS"])
            setConfig({ l1Bridge: env["BRIDGE_L1_ADDRESS"] });
          if (!config.l2Bridge && env["BRIDGE_L2_ADDRESS"])
            setConfig({ l2Bridge: env["BRIDGE_L2_ADDRESS"] });
          if (!config.flashExecutorL1 && env["FLASH_EXECUTOR_L1_ADDRESS"])
            setConfig({ flashExecutorL1: env["FLASH_EXECUTOR_L1_ADDRESS"] });
          if (!config.flashTokenAddress && env["FLASH_TOKEN_ADDRESS"])
            setConfig({ flashTokenAddress: env["FLASH_TOKEN_ADDRESS"] });
          if (!config.flashPoolAddress && env["FLASH_POOL_ADDRESS"])
            setConfig({ flashPoolAddress: env["FLASH_POOL_ADDRESS"] });
          if (!config.flashNftAddress && env["FLASH_NFT_ADDRESS"])
            setConfig({ flashNftAddress: env["FLASH_NFT_ADDRESS"] });
          if (!config.flashExecutorL2 && env["FLASH_EXECUTOR_L2_ADDRESS"])
            setConfig({ flashExecutorL2: env["FLASH_EXECUTOR_L2_ADDRESS"] });
          if (!config.flashWrappedTokenL2 && env["WRAPPED_TOKEN_L2"])
            setConfig({ flashWrappedTokenL2: env["WRAPPED_TOKEN_L2"] });
          if (!config.reverseExecutorL2 && env["REVERSE_EXECUTOR_L2"])
            setConfig({ reverseExecutorL2: env["REVERSE_EXECUTOR_L2"] });
          if (!config.reverseNftL1 && env["REVERSE_NFT_L1"])
            setConfig({ reverseNftL1: env["REVERSE_NFT_L1"] });
          if (!config.reverseExecutorL1 && env["REVERSE_EXECUTOR_L1"])
            setConfig({ reverseExecutorL1: env["REVERSE_EXECUTOR_L1"] });
          if (!config.faucetAddress && env["FAUCET_ADDRESS"])
            setConfig({ faucetAddress: env["FAUCET_ADDRESS"] });
          if (!config.ccmL2Address && env["CROSS_CHAIN_MANAGER_ADDRESS"])
            setConfig({ ccmL2Address: env["CROSS_CHAIN_MANAGER_ADDRESS"] });
          // Aggregator addresses
          if (!config.aggWeth && env["AGG_WETH_ADDRESS"])
            setConfig({ aggWeth: env["AGG_WETH_ADDRESS"] });
          if (!config.aggUsdc && env["AGG_USDC_ADDRESS"])
            setConfig({ aggUsdc: env["AGG_USDC_ADDRESS"] });
          if (!config.aggL1Amm && env["AGG_L1_AMM_ADDRESS"])
            setConfig({ aggL1Amm: env["AGG_L1_AMM_ADDRESS"] });
          if (!config.aggAggregator && env["AGG_AGGREGATOR_ADDRESS"])
            setConfig({ aggAggregator: env["AGG_AGGREGATOR_ADDRESS"] });
          if (!config.aggL2Executor && env["AGG_L2_EXECUTOR_ADDRESS"])
            setConfig({ aggL2Executor: env["AGG_L2_EXECUTOR_ADDRESS"] });
          if (!config.aggL2Amm && env["AGG_L2_AMM_ADDRESS"])
            setConfig({ aggL2Amm: env["AGG_L2_AMM_ADDRESS"] });
          if (!config.aggL2ExecutorProxy && env["AGG_L2_EXECUTOR_PROXY_ADDRESS"])
            setConfig({ aggL2ExecutorProxy: env["AGG_L2_EXECUTOR_PROXY_ADDRESS"] });
          if (!config.aggWrappedWethL2 && env["AGG_WRAPPED_WETH_L2"])
            setConfig({ aggWrappedWethL2: env["AGG_WRAPPED_WETH_L2"] });
          if (!config.aggWrappedUsdcL2 && env["AGG_WRAPPED_USDC_L2"])
            setConfig({ aggWrappedUsdcL2: env["AGG_WRAPPED_USDC_L2"] });
          registerContractsFromEnv(env);
        }
      } catch {
        /* shared not mounted */
      }

      // Auto-detect chain IDs from RPCs
      try {
        const l1ChainId = (await rpcCall(
          config.l1Rpc,
          "eth_chainId",
        )) as string;
        L1_CHAIN.chainId = l1ChainId;
        const dec = parseInt(l1ChainId, 16);
        L1_CHAIN.chainName = `Based Rollup L1 (${dec})`;
      } catch {
        /* keep defaults */
      }

      try {
        const l2ChainId = (await rpcCall(
          config.l2Rpc,
          "eth_chainId",
        )) as string;
        L2_CHAIN.chainId = l2ChainId;
        const dec = parseInt(l2ChainId, 16);
        L2_CHAIN.chainName = `Based Rollup L2 (${dec})`;
      } catch {
        /* keep defaults */
      }

      setLoaded(true);
    })();
  }, []);

  return loaded;
}
