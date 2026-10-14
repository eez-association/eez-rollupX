#!/bin/bash
set -euo pipefail
# Keep SSR on the internal backend; let browsers use the opened origin for
# HTTP and WebSocket requests. This also works through an SSH port forward.
cat >> public/assets/envs.js <<'JS'
Object.assign(window.__envs, {
  NEXT_PUBLIC_APP_PROTOCOL: window.location.protocol.slice(0, -1),
  NEXT_PUBLIC_APP_HOST: window.location.hostname,
  NEXT_PUBLIC_APP_PORT: window.location.port,
  NEXT_PUBLIC_API_PROTOCOL: window.location.protocol.slice(0, -1),
  NEXT_PUBLIC_API_HOST: window.location.hostname,
  NEXT_PUBLIC_API_PORT: window.location.port,
  // The path image keeps SSR on the root-path backend; only browser API and
  // WebSocket calls use the public /explorer prefix.
  NEXT_PUBLIC_API_BASE_PATH: window.location.pathname === "/explorer" || window.location.pathname.startsWith("/explorer/") ? "/explorer" : "",
  NEXT_PUBLIC_API_WEBSOCKET_PROTOCOL: window.location.protocol === "https:" ? "wss" : "ws",
  NEXT_PUBLIC_USE_NEXT_JS_PROXY: "false"
});
// Blockscout's implicit contract-read RPC URL ignores API_BASE_PATH.
window.__envs.NEXT_PUBLIC_NETWORK_RPC_URL ||= new URL(
  window.__envs.NEXT_PUBLIC_API_BASE_PATH + "/api/eth-rpc",
  window.location.origin
).href;
JS
exec node server.js
