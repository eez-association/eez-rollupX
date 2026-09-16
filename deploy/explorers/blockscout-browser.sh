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
  NEXT_PUBLIC_API_WEBSOCKET_PROTOCOL: window.location.protocol === "https:" ? "wss" : "ws",
  NEXT_PUBLIC_USE_NEXT_JS_PROXY: "false"
});
JS
exec node server.js
