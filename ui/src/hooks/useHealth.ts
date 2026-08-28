import { useCallback, useEffect, useState } from "react";
import { config } from "../config";
import { rpcCall } from "../rpc";

export interface HealthData {
  healthy: boolean;
  mode: string;
  l2_head: number;
  l1_derivation_head: number;
  pending_submissions: number;
  consecutive_rewind_cycles: number;
  commit?: string;
}

const HOST = window.location.hostname;
const PROTO = window.location.protocol;
const params = new URLSearchParams(window.location.search);
const IS_PROXIED =
  PROTO === "https:" ||
  (HOST !== "localhost" && HOST !== "127.0.0.1" && !window.location.port);
const HEALTH_URL = params.get("health") ||
  (IS_PROXIED ? `${PROTO}//${window.location.host}/health` : `http://${HOST}:9560/health`);

export function useHealth() {
  const [health, setHealth] = useState<HealthData | null>(null);

  const refresh = useCallback(async () => {
    try {
      const res = await fetch(HEALTH_URL);
      if (res.ok) {
        const data = (await res.json()) as HealthData;
        setHealth(data);
        return;
      }
    } catch {
      // The current node does not expose the legacy POC health schema.
    }

    try {
      const [l1Head, l2Head] = await Promise.all([
        rpcCall(config.l1Rpc, "eth_blockNumber") as Promise<string>,
        rpcCall(config.l2Rpc, "eth_blockNumber") as Promise<string>,
      ]);
      setHealth({
        healthy: true,
        mode: "eez",
        l2_head: parseInt(l2Head, 16),
        l1_derivation_head: parseInt(l1Head, 16),
        pending_submissions: 0,
        consecutive_rewind_cycles: 0,
      });
    } catch {
      setHealth(null);
    }
  }, []);

  useEffect(() => {
    refresh();
    const interval = setInterval(refresh, 3000);
    return () => clearInterval(interval);
  }, [refresh]);

  return health;
}
