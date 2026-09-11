#!/usr/bin/env bash
# Read-only ARC/Docker diagnostics. No installs, restarts, scaling, or cleanup.
# Usage: bash diagnose-arc.sh [values-file] [namespace] [release] [kubeconfig]
# Output is a shareable report, but review logs for sensitive information first.
set -uo pipefail

if [[ ${1:-} == --help || $# -gt 4 ]]; then
    echo "Usage: bash $0 [values-file] [namespace] [release] [kubeconfig]"
    echo "Defaults: /home/edu/arc-rust-values.yaml arc-runners homelab-rust /etc/rancher/k3s/k3s.yaml"
    exit 0
fi

arc_values=${1:-/home/edu/arc-rust-values.yaml}
arc_namespace=${2:-arc-runners}
arc_release=${3:-homelab-rust}
arc_kubeconfig=${4:-/etc/rancher/k3s/k3s.yaml}

for arc_tool in kubectl jq timeout; do
    if ! command -v "$arc_tool" >/dev/null 2>&1; then
        echo "Missing required command: $arc_tool" >&2
        exit 1
    fi
done
if [[ ! -r "$arc_kubeconfig" ]]; then
    echo "Cannot read $arc_kubeconfig; run with sudo or pass a readable kubeconfig as argument 4." >&2
    exit 1
fi

# Avoid dumping full environments, kubeconfigs, Secret contents, or runner job
# logs. Scrub common credentials from diagnostic errors and Docker daemon logs;
# this cannot guarantee removal of arbitrary secrets embedded in free text.
redact() {
    sed -E \
        -e '/-----BEGIN .*PRIVATE KEY-----/,/-----END .*PRIVATE KEY-----/c\[private key redacted]' \
        -e 's#([[:alpha:]][[:alnum:]+.-]*://)[^/@[:space:]]+@#\1[redacted]@#g' \
        -e 's/github_pat_[[:alnum:]_]+/[GitHub token redacted]/g' \
        -e 's/gh[pousr]_[[:alnum:]_]+/[GitHub token redacted]/g' \
        -e 's/((token|password|passwd|secret|authorization)[[:space:]"=:]+)[^[:space:],]+/\1[redacted]/Ig'
}

section() { printf '\n### %s\n' "$*"; }
k() { timeout 30s kubectl --kubeconfig "$arc_kubeconfig" --request-timeout=15s "$@" 2> >(redact >&2); }
h() { timeout 30s helm --kubeconfig "$arc_kubeconfig" "$@" 2> >(redact >&2); }
probe() {
    "$@" 2>&1 | redact
    local arc_rc=${PIPESTATUS[0]}
    if (( arc_rc != 0 )); then echo "[check unavailable or failed: exit $arc_rc; continuing]"; fi
    return 0
}

# Only these configuration fields reach stdout. In particular, Helm's inline
# githubConfigSecret and arbitrary env values/command arguments are excluded.
arc_filters='
def container:
  {name, image, imagePullPolicy, restartPolicy, securityContext, resources,
   volumeMounts, startupProbe: (.startupProbe | if . == null then null else
     {initialDelaySeconds, periodSeconds, timeoutSeconds, failureThreshold} end),
   env: [(.env // [])[] | select(.name | IN(
     "DOCKER_HOST", "DOCKER_CONTEXT", "DOCKER_TLS_VERIFY", "DOCKER_CERT_PATH",
     "DOCKER_TLS_CERTDIR", "DOCKER_GROUP_GID", "RUNNER_WAIT_FOR_DOCKER_IN_SECONDS",
     "CARGO_HOME", "CARGO_TARGET_DIR", "CARGO_INCREMENTAL", "RUSTC_WRAPPER")) |
     {name, value: (if has("valueFrom") then "[valueFrom reference]" else .value end)}]};
def podspec:
  {hostNetwork, hostPID, dnsPolicy, securityContext, nodeSelector,
   initContainers: [(.initContainers // [])[] | container],
   containers: [(.containers // [])[] | container],
   volumes: [(.volumes // [])[] |
     {name, hostPath, emptyDir, persistentVolumeClaim,
      secretVolume: has("secret"), projectedVolume: has("projected")}]};
def values:
  {runnerScaleSetName, minRunners, maxRunners,
   containerMode: (.containerMode.type // "[not set]"),
   template: ((.template.spec // {}) | podspec)};
def containerstatus:
  {name, ready, started, restartCount, image, imageID,
   state: (.state | if . == null then null else
     {running, waiting, terminated: (.terminated | if . == null then null else
       {reason, exitCode, signal, startedAt, finishedAt} end)} end),
   previousTermination: (.lastState.terminated | if . == null then null else
     {reason, exitCode, signal, finishedAt} end)};
def dockercontainers:
  [(.initContainers // [])[], (.containers // [])[]] |
  map(select(.name == "dind" or ((.image // "") | contains("dind")) or
    any(((.command // []) + (.args // []))[]; . == "dockerd")));
'

section "Report metadata"
date -u '+UTC: %Y-%m-%dT%H:%M:%SZ'
printf 'Host: %s\nValues: %s\nNamespace: %s\nRelease: %s\nKubeconfig: %s\n' \
    "$(hostname)" "$arc_values" "$arc_namespace" "$arc_release" "$arc_kubeconfig"
echo "Read-only diagnostics. No runner job logs or Secret contents are requested. Review before sharing."

section "Local values file (selected fields only)"
if [[ ! -r "$arc_values" ]]; then
    echo "Values file is missing or unreadable. Live cluster checks will still run."
elif command -v python3 >/dev/null 2>&1 && python3 -c 'import yaml' 2>/dev/null; then
    stat -c 'Modified: %y; size: %s bytes; permissions: %A' "$arc_values"
    # YAML exception messages may contain source lines/secrets: do not print them.
    python3 - "$arc_values" <<'PY' | jq "$arc_filters values" | redact
import json
import sys
import yaml

try:
    with open(sys.argv[1]) as source:
        config = yaml.safe_load(source)
    if not isinstance(config, dict):
        raise ValueError("values must be a mapping")
    print(json.dumps(config))
except Exception:
    print("Could not parse the values file (details suppressed to protect secrets).", file=sys.stderr)
    sys.exit(1)
PY
else
    echo "Skipping local YAML parsing: optional python3-yaml is not installed. Live Helm values are collected below."
fi

section "Kubernetes version and node health"
probe k version -o json
if arc_nodes=$(k get nodes -o json); then
    jq '[.items[] | {name: .metadata.name, capacity: .status.capacity,
      allocatable: .status.allocatable, conditions: [.status.conditions[]? |
      select(.type | IN("Ready", "DiskPressure", "MemoryPressure", "PIDPressure")) |
      {type, status, reason}]}]' <<< "$arc_nodes" | redact
fi

section "Helm releases and deployed values"
if command -v helm >/dev/null 2>&1; then
    probe h list -n "$arc_namespace"
    if arc_helm_values=$(h get values "$arc_release" -n "$arc_namespace" -o json); then
        jq "$arc_filters values" <<< "$arc_helm_values" | redact
    fi
else
    echo "Helm is unavailable; generated runner template checks will still run."
fi

section "Generated ARC template (authoritative for new runner pods)"
if arc_set=$(k -n "$arc_namespace" get autoscalingrunnerset "$arc_release" -o json); then
    jq "$arc_filters "'{name: .metadata.name, generation: .metadata.generation,
      minRunners: .spec.minRunners, maxRunners: .spec.maxRunners, status,
      pod: (.spec.template.spec | podspec),
      dockerContainers: [.spec.template.spec | dockercontainers[] | .name]}' \
      <<< "$arc_set" | redact
    if jq -e "$arc_filters .spec.template.spec | dockercontainers | length == 0" \
        <<< "$arc_set" >/dev/null; then
        echo "NOTE: No recognizable DinD container is declared. Docker would need another endpoint/socket mount."
    fi
fi

section "Current namespace pods"
probe k -n "$arc_namespace" get pods -o wide
if ! arc_pods=$(k -n "$arc_namespace" get pods -o json); then
    echo "Cannot list pods; check the namespace, kubeconfig and Kubernetes access." >&2
    exit 1
fi

arc_runner_names=$(jq -r --arg release "$arc_release" '
  [.items[] | select(any(.spec.containers[]?; .name == "runner")) |
    select(.metadata.labels["actions.github.com/scale-set-name"] == $release or
      (.metadata.name | startswith($release + "-")))] |
  sort_by(.metadata.creationTimestamp) | reverse | .[:4][] | .metadata.name' <<< "$arc_pods")

if [[ -z "$arc_runner_names" ]]; then
    echo "No matching runner pods exist. This is normal when minRunners=0 and CI is idle."
    echo "The generated template above still shows whether DinD/socket wiring is configured."
    echo "For runtime checks, run this script again while a CI job is using this scale set."
fi

# All commands executed in containers are bounded, read-only diagnostics. Avoid
# full docker info (it can expose proxy credentials) and process command lines.
arc_runtime_probe='
id
for name in DOCKER_HOST DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH RUNNER_WAIT_FOR_DOCKER_IN_SECONDS CARGO_HOME CARGO_TARGET_DIR CARGO_INCREMENTAL RUSTC_WRAPPER; do
  printf "%s=" "$name"
  printenv "$name" || printf "[unset]\n"
done
ls -ld /var/run /run 2>/dev/null
stat -Lc "%A uid=%u gid=%g device=%d inode=%i %n" /var/run/docker.sock /run/docker.sock 2>&1
case "${DOCKER_HOST:-}" in
  unix://*) stat -Lc "%A uid=%u gid=%g device=%d inode=%i %n" "${DOCKER_HOST#unix://}" 2>&1 ;;
esac
df -h /home/runner/_work /ci/cargo /var/run 2>&1
if ! command -v docker >/dev/null 2>&1; then echo "Docker CLI is missing"; exit 0; fi
if ! command -v timeout >/dev/null 2>&1; then echo "No timeout command; skipping Docker API calls"; exit 0; fi
timeout 5s docker context show
timeout 5s docker context inspect --format "{{.Name}} endpoint={{.Endpoints.docker.Host}}"
timeout 5s docker version
timeout 5s docker info --format "server={{.ServerVersion}} name={{.Name}} root={{.DockerRootDir}} driver={{.Driver}} cpus={{.NCPU}} memory={{.MemTotal}} containers={{.Containers}} images={{.Images}}"
'

while IFS= read -r arc_pod_name; do
    [[ -n "$arc_pod_name" ]] || continue
    section "Runner pod: $arc_pod_name"
    if ! arc_pod=$(k -n "$arc_namespace" get pod "$arc_pod_name" -o json); then
        echo "Pod may have finished and been removed; continuing."
        continue
    fi
    jq "$arc_filters "'{name: .metadata.name, node: .spec.nodeName,
      created: .metadata.creationTimestamp, deleting: .metadata.deletionTimestamp,
      phase: .status.phase, pod: (.spec | podspec),
      initStatus: [.status.initContainerStatuses[]? | containerstatus],
      containerStatus: [.status.containerStatuses[]? | containerstatus]}' <<< "$arc_pod" | redact

    if jq -e 'any(.status.containerStatuses[]?; .name == "runner" and .state.running != null)' \
        <<< "$arc_pod" >/dev/null; then
        section "Docker as the actual runner user: $arc_pod_name"
        probe k -n "$arc_namespace" exec "$arc_pod_name" -c runner -- sh -c "$arc_runtime_probe"
    else
        echo "Runner container is not running; runtime checks skipped."
    fi

    while IFS= read -r arc_dind; do
        [[ -n "$arc_dind" ]] || continue
        section "Docker container $arc_dind: status/permissions/API"
        if jq -e --arg name "$arc_dind" '
          any([.status.initContainerStatuses[]?, .status.containerStatuses[]?][];
            .name == $name and .state.running != null)' <<< "$arc_pod" >/dev/null; then
            probe k -n "$arc_namespace" exec "$arc_pod_name" -c "$arc_dind" -- sh -c "$arc_runtime_probe"
        fi
        section "Docker container $arc_dind: last 80 log lines"
        probe k -n "$arc_namespace" logs "$arc_pod_name" -c "$arc_dind" --tail=80 --timestamps
        if jq -e --arg name "$arc_dind" '
          any([.status.initContainerStatuses[]?, .status.containerStatuses[]?][];
            .name == $name and .restartCount > 0)' <<< "$arc_pod" >/dev/null; then
            section "Docker container $arc_dind: previous instance"
            probe k -n "$arc_namespace" logs "$arc_pod_name" -c "$arc_dind" --previous --tail=80 --timestamps
        fi
    done < <(jq -r "$arc_filters .spec | dockercontainers[] | .name" <<< "$arc_pod")
done <<< "$arc_runner_names"

section "Recent events for this runner scale set"
if arc_events=$(k -n "$arc_namespace" get events -o json); then
    jq --arg release "$arc_release" '[.items[] |
      select(.involvedObject.name == $release or (.involvedObject.name | startswith($release + "-"))) |
      {time: (.lastTimestamp // .eventTime // .metadata.creationTimestamp),
       type, reason, object: .involvedObject.name, message, count}] |
      sort_by(.time) | .[-30:]' <<< "$arc_events" | redact
fi

section "End of report"
echo "No cluster settings, workloads, caches or credentials were changed."
echo "Review this report before sharing; hostnames, paths, IPs and diagnostic log text may be present."
