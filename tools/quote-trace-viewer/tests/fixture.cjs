// Synthetic topology for validation/UI tests. Process tests export real quotes.
const address = digit => "0x" + digit.repeat(40);
const bytes = hex => ({ hex, length: (hex.length - 2) / 2, truncated: false });
const word = value => BigInt(value).toString(16).padStart(64, "0");
const revert = text => {
  const encoded = Buffer.from(text).toString("hex");
  return "0x08c379a0" + word(32) + word(encoded.length / 2) + encoded.padEnd(Math.ceil(encoded.length / 64) * 64, "0");
};
function fixture() {
  const frame = (id, parentId, chainId, status, output = "0x") => ({
    id, parentId, chainId, status, kind: "CALL", executionRoot: true,
    crossChain: parentId !== null, from: address("1"), to: address(String(id + 2)),
    codeAddress: address(String(id + 2)), value: "0x0", gas: "0x10000", gasUsed: "0x100",
    isStatic: false, input: bytes("0x12345678"), output: bytes(output),
    result: status === "revert" ? "Revert" : "Return",
  });
  const frames = [
    frame(0, null, "0x1", "success"),
    frame(1, 0, "0x2a", "revert", revert("Dependency rejected")),
    frame(2, 1, "0x1", "success"),
  ];
  return {
    jsonrpc: "2.0", id: 1,
    result: {
      options: [{ type: "0x2", gas: "0x10000" }],
      diagnostics: {
        trace: {
          format: "eez-quote-call-trace", version: 1,
          request: { from: address("1"), to: address("2"), data: "0x12345678" },
          context: { l1ChainId: "0x1", l2ChainId: "0x2a", simulation: true },
          truncated: false, omittedFrames: 0, omittedRuns: 0,
          runs: [{ id: 0, type: "0x2", phase: "final_validation", pass: 1, purpose: "composition",
            chainId: "0x1", transaction: { gas: "0x10000", input: bytes("0x12345678") },
            frames, complete: true }],
        },
      },
    },
  };
}
module.exports = { fixture, bytes, revert };
