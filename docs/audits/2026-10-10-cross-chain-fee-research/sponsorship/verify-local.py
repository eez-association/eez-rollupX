#!/usr/bin/env python3
"""Isolated native-blob, operator-value and sponsored-tip experiments; no public RPC.

Uses the pinned EEZ bytecode, a hash-checking mock proof system, and synthetic
L1 companions. This does not run Composer, an L2 prover, or a production builder.
Retained to reproduce the funding/fee boundary without operator wallets/state.
"""

import hashlib
import json
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.request


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
BASE_FEE = 1_000_000_000
GAS_LIMIT = 500_000
MAX_FEE = 2_000_000_000
BLOB_CAP = 1_000_000_000
RESERVE = GAS_LIMIT * MAX_FEE + 131_072 * BLOB_CAP
# Public disposable fixture keys, used only on a newly created local chain.
KEYS = [f"0x{i:064x}" for i in range(1, 9)]


def command(args, *, stdin=None, timeout=90, cwd=ROOT):
    result = subprocess.run(args, input=stdin, text=True, capture_output=True,
                            cwd=cwd, timeout=timeout)
    if result.returncode:
        raise RuntimeError(f"{args[0:2]} failed: {result.stderr[-4000:]}")
    return result.stdout.strip()


def main():
    work = Path(tempfile.mkdtemp(prefix="rollupx-sponsorship-"))
    print(f"Local evidence directory: {work}", flush=True)
    record = {"scope": __doc__, "cases": [], "local_evidence_directory": str(work)}
    process = None
    try:
        record["source"] = command(["git", "rev-parse", "HEAD"])
        record["protocol"] = command(["git", "-C", "eez-core-protocol", "rev-parse", "HEAD"])
        record["fixture_sha256"] = {
            name: hashlib.sha256((HERE / name).read_bytes()).hexdigest()
            for name in ["Probe.sol", "verify-local.py"]
        }
        assert record["source"] == "fa916b13254a36423cad130080dde367bb18a7d6"
        assert record["protocol"] == "855fe0602484750861b4c31502a1a91355d33ed6"
        record["executables"] = {}
        for tool in ["forge", "cast", "anvil"]:
            path = Path(shutil.which(tool))
            record["executables"][tool] = {
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                "version": command([str(path), "--version"]),
            }
            assert "Version: 1.7.1" in record["executables"][tool]["version"]
        artifact_path = ROOT / "eez-core-protocol/out/EEZ.sol/EEZ.json"
        artifact = json.loads(artifact_path.read_text())
        metadata = artifact["metadata"]
        if isinstance(metadata, str):
            metadata = json.loads(metadata)
        assert metadata["compiler"]["version"].startswith("0.8.34+")
        assert metadata["settings"]["optimizer"] == {"enabled": True, "runs": 1}
        assert metadata["settings"]["viaIR"] is True
        for name, expected in metadata["sources"].items():
            source = ROOT / "eez-core-protocol" / name
            actual = command(["cast", "keccak"], stdin="0x" + source.read_bytes().hex())
            assert actual == expected["keccak256"], f"Stale compiled dependency: {name}"
        record["eez_artifact_sha256"] = hashlib.sha256(artifact_path.read_bytes()).hexdigest()
        record["eez_metadata_source_files_verified"] = len(metadata["sources"])
        (work / "src").mkdir()
        shutil.copy(HERE / "Probe.sol", work / "src/Probe.sol")
        shutil.copy(ROOT / "eez-core-protocol/src/interfaces/IEEZ.sol", work / "src/IEEZ.sol")
        (work / "foundry.toml").write_text(
            '[profile.default]\nsrc="src"\nsolc="0.8.34"\noptimizer=true\n'
            'optimizer_runs=1\nvia_ir=true\nevm_version="osaka"\n')
        build = command(["forge", "build", "--root", str(work), "--offline"], timeout=180)
        (work / "build.log").write_text(build + "\n")
        probe_artifact = json.loads((work / "out/Probe.sol/Probe.json").read_text())
        record["probe_bytecode_sha256"] = hashlib.sha256(
            bytes.fromhex(probe_artifact["bytecode"]["object"].removeprefix("0x"))).hexdigest()
        with socket.socket() as selector:
            selector.bind(("127.0.0.1", 0))
            port = selector.getsockname()[1]
        rpc_url = f"http://127.0.0.1:{port}"
        log = (work / "anvil.log").open("w")
        process = subprocess.Popen([
            "anvil", "--host", "127.0.0.1", "--port", str(port), "--chain-id", "31337",
            "--hardfork", "osaka", "--accounts", "0", "--no-mining", "--order", "fifo",
            "--gas-limit", "30000000", "--base-fee", str(BASE_FEE), "--silent",
        ], stdout=log, stderr=subprocess.STDOUT)

        def rpc(method, params, *, error_ok=False):
            body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
            request = urllib.request.Request(rpc_url, body, {"Content-Type": "application/json"})
            with urllib.request.urlopen(request, timeout=20) as response:
                data = json.load(response)
            if "error" in data:
                if error_ok:
                    return {"error": data["error"]}
                raise RuntimeError(f"{method}: {data['error']}")
            return data["result"]

        for _ in range(100):
            try:
                assert rpc("eth_chainId", []) == "0x7a69"
                break
            except (OSError, AssertionError):
                if process.poll() is not None:
                    raise RuntimeError("Owned Anvil exited during startup")
                time.sleep(0.1)
        else:
            raise RuntimeError("Owned Anvil did not start")

        addresses = [command(["cast", "wallet", "address", "--private-key", key]) for key in KEYS]
        builder, poster, *rest = addresses
        users, deployer = rest[:3], rest[3]
        rpc("anvil_setCoinbase", [builder])
        for address in [builder, *users, deployer]:
            rpc("anvil_setBalance", [address, hex(100 * 10**18)])

        def balance(address):
            return int(rpc("eth_getBalance", [address, "latest"]), 16)

        def sign(key, to, data="0x", *, value=0, gas=GAS_LIMIT, tip=0, blob=False, nonce=None):
            sender = addresses[KEYS.index(key)]
            if nonce is None:
                nonce = int(rpc("eth_getTransactionCount", [sender, "latest"]), 16)
            args = ["cast", "mktx", "--rpc-url", rpc_url, "--chain", "31337",
                    "--private-key", key, "--nonce", str(nonce),
                    "--gas-limit", str(gas), "--gas-price", str(max(MAX_FEE, tip + BASE_FEE)),
                    "--priority-gas-price", str(tip), "--value", str(value)]
            if blob:
                args += ["--blob", "--blob-gas-price", str(BLOB_CAP), "--path", str(work / "blob.bin")]
            args += [to, data] if to else ["--create", data]
            return command(args)

        def mine(timestamp=None):
            rpc("anvil_setNextBlockBaseFeePerGas", [hex(BASE_FEE)])
            if timestamp is not None:
                rpc("evm_setNextBlockTimestamp", [timestamp])
            rpc("evm_mine", [])

        def deploy(bytecode, argument):
            encoded = command(["cast", "abi-encode", "f(address)", argument])
            raw = sign(KEYS[5], None, bytecode + encoded[2:], gas=8_000_000)
            tx_hash = rpc("eth_sendRawTransaction", [raw])
            mine()
            receipt = rpc("eth_getTransactionReceipt", [tx_hash])
            assert receipt["status"] == "0x1"
            return receipt["contractAddress"]

        registry = deploy(artifact["bytecode"]["object"], deployer)
        probe = deploy(probe_artifact["bytecode"]["object"], registry)
        data = command(["cast", "calldata", "registerRollup(address,bytes32)", probe, "0x" + "00" * 32])
        registration = rpc("eth_sendRawTransaction", [sign(KEYS[5], registry, data)])
        mine()
        assert rpc("eth_getTransactionReceipt", [registration])["status"] == "0x1"
        (work / "blob.bin").write_bytes(b"local sponsorship fee-mechanics probe")
        dummy = sign(KEYS[1], registry, blob=True)
        decoded = json.loads(command(["cast", "decode-transaction"], stdin=dummy))
        if isinstance(decoded, str):
            decoded = json.loads(decoded)
        blob_hash = decoded["blobVersionedHashes"][0]
        record["blob_hash"] = blob_hash
        record["reserve_wei"] = RESERVE
        record["registry"] = registry
        record["probe"] = probe
        consume = command(["cast", "calldata", "consume()"])

        def prepare_post(*, value=0, tip=0, gas=GAS_LIMIT):
            parent = rpc("eth_getBlockByNumber", ["latest", False])
            timestamp = int(parent["timestamp"], 16) + 12
            call = command(["cast", "calldata", "postData(bytes32,uint64,bytes32)", blob_hash,
                            str(timestamp), parent["hash"]])
            output = bytes.fromhex(rpc("eth_call", [{"to": probe, "data": call}, "latest"])[2:])
            offset = int.from_bytes(output[:32])
            size = int.from_bytes(output[offset:offset+32])
            calldata = "0x" + output[offset+32:offset+32+size].hex()
            return sign(KEYS[1], registry, calldata, value=value, tip=tip, gas=gas, blob=True), timestamp

        raw, _ = prepare_post()
        unfunded = rpc("eth_sendRawTransaction", [raw], error_ok=True)
        assert isinstance(unfunded, dict) and "error" in unfunded, unfunded
        assert "fund" in unfunded["error"]["message"].lower(), unfunded
        record["cases"].append({"name": "zero_balance_without_funding", "result": unfunded})
        print("PASS: unfunded native blob transaction rejected", flush=True)
        for name, amount in [("execution_cap_only", GAS_LIMIT * MAX_FEE),
                             ("one_wei_below_full_reserve", RESERVE - 1)]:
            rpc("anvil_setBalance", [poster, hex(amount)])
            rejected = rpc("eth_sendRawTransaction", [raw], error_ok=True)
            assert isinstance(rejected, dict) and "error" in rejected, rejected
            assert "fund" in rejected["error"]["message"].lower(), rejected
            record["cases"].append({"name": name, "balance_wei": amount, "result": rejected})
            print(f"PASS: {name} rejected", flush=True)
        rpc("anvil_setBalance", [poster, "0x0"])

        # Ensure the mock checks its exact context, rather than accepting any
        # proof. A funding prefix must not move the post into another slot.
        snapshot = rpc("evm_snapshot", [])
        rpc("anvil_setBalance", [poster, hex(RESERVE)])
        wrong_slot_raw, expected_timestamp = prepare_post()
        wrong_slot = rpc("eth_sendRawTransaction", [wrong_slot_raw])
        mine(expected_timestamp + 1)
        wrong_receipt = rpc("eth_getTransactionReceipt", [wrong_slot])
        assert wrong_receipt and wrong_receipt["status"] == "0x0", wrong_receipt
        assert int(wrong_receipt["blobGasUsed"], 16) == 131_072
        record["cases"].append({"name": "wrong_slot_reverts", "status": "0x0",
                                "blob_gas_used": 131_072})
        assert rpc("evm_revert", [snapshot])
        assert balance(poster) == 0
        print("PASS: wrong-slot post reverted and charged blob gas", flush=True)

        def batch(name, *, sponsored):
            before_poster, before_builder = balance(poster), balance(builder)
            raw_post, timestamp = prepare_post()
            funding = max(0, RESERVE - before_poster) if sponsored else 0
            txs = []
            if funding:
                txs.append(rpc("eth_sendRawTransaction", [sign(KEYS[0], poster, value=funding, gas=21_000)]))
            response = rpc("eth_sendRawTransaction", [raw_post], error_ok=True)
            queue_override = isinstance(response, dict)
            if queue_override:
                # Txpool admission may ignore pending incoming transfers. This
                # temporary fixture balance is restored BEFORE block execution.
                # The actual block must execute the real funding transaction.
                rpc("anvil_setBalance", [poster, hex(RESERVE)])
                response = rpc("eth_sendRawTransaction", [raw_post])
                rpc("anvil_setBalance", [poster, hex(before_poster)])
            post_hash = response
            txs.append(post_hash)
            for key in KEYS[2:5]:
                txs.append(rpc("eth_sendRawTransaction", [sign(key, probe, consume, gas=100_000, tip=50_000_000_000)]))
            assert balance(poster) == before_poster
            mine(timestamp)
            receipts = [rpc("eth_getTransactionReceipt", [tx]) for tx in txs]
            assert all(receipt and receipt["status"] == "0x1" for receipt in receipts), receipts
            block = rpc("eth_getBlockByNumber", [receipts[0]["blockNumber"], False])
            assert block["transactions"] == txs
            post = next(receipt for receipt in receipts if receipt["transactionHash"] == post_hash)
            assert post["type"] == "0x3"
            blob_cost = int(post["blobGasUsed"], 16) * int(post["blobGasPrice"], 16)
            post_cost = int(post["gasUsed"], 16) * int(post["effectiveGasPrice"], 16) + blob_cost
            tips = sum(int(r["gasUsed"], 16) * (int(r["effectiveGasPrice"], 16) - BASE_FEE) for r in receipts[-3:])
            funding_cost = 21_000 * BASE_FEE if funding else 0
            assert balance(poster) == before_poster + funding - post_cost
            assert balance(builder) - before_builder == tips - funding - funding_cost
            assert tips - funding - funding_cost > 0
            row = {"name": name, "funding_wei": funding, "poster_balance_before_wei": before_poster,
                   "poster_balance_after_wei": balance(poster), "post_cost_wei": post_cost,
                   "post_gas_used": int(post["gasUsed"], 16), "blob_cost_wei": blob_cost,
                   "customer_tips_wei": tips, "builder_net_wei": balance(builder)-before_builder,
                   "txpool_only_balance_override": queue_override, "transaction_count": len(txs),
                   "transaction_hashes": txs}
            record["cases"].append(row)
            (work / f"{name}-receipts.json").write_text(json.dumps(receipts, indent=2) + "\n")
            print(f"PASS: {name}: {len(txs)} txs; blob fee {blob_cost}; builder net {row['builder_net_wei']} wei", flush=True)

        batch("sponsored_first_batch", sponsored=True)
        batch("sponsored_residual_balance", sponsored=True)
        rpc("anvil_setBalance", [poster, hex(RESERVE * 2)])
        batch("funded_poster_no_topup", sponsored=False)

        # This extension measures explicit operator value, preserving the gas-only
        # controls above. The funding transaction is still supplied by this harness,
        # so none of these cases establishes automatic funding by a real builder.
        payout_sender, treasury = addresses[6:8]
        payout_gas = 21_000
        payout_value = 2_400_000_000_000_000
        payout_reserve = payout_value + payout_gas * MAX_FEE
        assumed_non_gas_cost = 200_000_000_000_000
        record["value_payout_assumptions"] = {
            "requested_value_wei": payout_value,
            "payout_gas_limit": payout_gas,
            "assumed_non_gas_cost_wei": assumed_non_gas_cost,
            "non_gas_cost_note": "Illustrative arithmetic input, not measured proving or operating expense.",
            "funding_policy_note": "Harness uses full fee-cap shortfall; a live builder may use another policy.",
        }
        raw_control = sign(KEYS[6], treasury, value=payout_value, gas=payout_gas)
        for name, amount in [
            ("payout_zero_balance_rejected", 0),
            ("payout_gas_only_rejected", payout_gas * MAX_FEE),
            ("payout_one_wei_below_value_and_gas_rejected", payout_reserve - 1),
        ]:
            rpc("anvil_setBalance", [payout_sender, hex(amount)])
            rejected = rpc("eth_sendRawTransaction", [raw_control], error_ok=True)
            assert isinstance(rejected, dict) and "error" in rejected, rejected
            assert "fund" in rejected["error"]["message"].lower(), rejected
            record["cases"].append({"name": name, "balance_wei": amount, "result": rejected})
            print(f"PASS: {name}", flush=True)
        rpc("anvil_setBalance", [payout_sender, "0x0"])

        # Run the same pinned call/parent twice, varying only transaction value.
        # Snapshots restore state after each control; neither receipt is revenue.
        paired = []
        control_parent = rpc("eth_getBlockByNumber", ["latest", False])["hash"]
        for value, status in [(payout_value, "0x0"), (0, "0x1")]:
            snapshot = rpc("evm_snapshot", [])
            rpc("anvil_setBalance", [poster, hex(RESERVE + payout_value)])
            raw_post, timestamp = prepare_post(value=value)
            tx = rpc("eth_sendRawTransaction", [raw_post])
            mine(timestamp)
            receipt = rpc("eth_getTransactionReceipt", [tx])
            assert receipt and receipt["status"] == status, receipt
            assert int(receipt["blobGasUsed"], 16) == 131_072
            paired.append({"value_wei": value, "status": status, "transaction_hash": tx})
            (work / f"post-value-{value}-receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
            assert rpc("evm_revert", [snapshot])
            assert rpc("eth_getBlockByNumber", ["latest", False])["hash"] == control_parent
        record["cases"].append({"name": "post_with_service_value_reverts", "paired_controls": paired,
                                "parent_hash": control_parent, "blob_gas_charged_each": 131_072})
        print("PASS: nonzero-value post reverts; zero-value post succeeds at identical parent", flush=True)

        def value_batch(name, *, amount, sponsored, expect_builder_profit):
            owners = [poster, payout_sender, treasury]
            before = {address: balance(address) for address in [builder, *owners, *users]}
            required = amount + payout_gas * MAX_FEE
            funding = max(0, required - before[payout_sender]) if sponsored else 0
            txs = []
            funding_hash = None
            if funding:
                funding_hash = rpc("eth_sendRawTransaction", [sign(KEYS[0], payout_sender,
                                                                   value=funding, gas=21_000)])
                txs.append(funding_hash)
            raw_post, timestamp = prepare_post()
            post_hash = rpc("eth_sendRawTransaction", [raw_post])
            txs.append(post_hash)
            user_hashes = [rpc("eth_sendRawTransaction", [sign(key, probe, consume, gas=100_000,
                                                               tip=50_000_000_000)]) for key in KEYS[2:5]]
            txs.extend(user_hashes)
            raw_payout = sign(KEYS[6], treasury, value=amount, gas=payout_gas)
            response = rpc("eth_sendRawTransaction", [raw_payout], error_ok=True)
            queue_override = isinstance(response, dict)
            if queue_override:
                assert funding > 0 and "fund" in response["error"]["message"].lower(), response
                rpc("anvil_setBalance", [payout_sender, hex(required)])
                response = rpc("eth_sendRawTransaction", [raw_payout])
                rpc("anvil_setBalance", [payout_sender, hex(before[payout_sender])])
            payout_hash = response
            txs.append(payout_hash)
            assert all(balance(address) == before[address] for address in before)
            mine(timestamp)
            receipts = [rpc("eth_getTransactionReceipt", [tx]) for tx in txs]
            assert all(r and r["status"] == "0x1" for r in receipts), receipts
            block = rpc("eth_getBlockByNumber", [receipts[0]["blockNumber"], False])
            assert block["transactions"] == txs
            by_hash = {r["transactionHash"]: r for r in receipts}

            def cost(receipt):
                return (int(receipt["gasUsed"], 16) * int(receipt["effectiveGasPrice"], 16)
                        + int(receipt.get("blobGasUsed", "0x0"), 16)
                        * int(receipt.get("blobGasPrice", "0x0"), 16))

            post_cost = cost(by_hash[post_hash])
            assert by_hash[post_hash]["type"] == "0x3"
            assert int(by_hash[post_hash]["blobGasUsed"], 16) == 131_072
            payout_cost = cost(by_hash[payout_hash])
            assert int(by_hash[payout_hash]["gasUsed"], 16) == payout_gas
            funding_cost = cost(by_hash[funding_hash]) if funding_hash else 0
            tips = sum(int(by_hash[h]["gasUsed"], 16)
                       * (int(by_hash[h]["effectiveGasPrice"], 16) - BASE_FEE) for h in user_hashes)
            after = {address: balance(address) for address in before}
            assert after[poster] == before[poster] - post_cost
            assert after[payout_sender] == before[payout_sender] + funding - amount - payout_cost
            assert after[treasury] == before[treasury] + amount
            assert all(after[address] == before[address] - cost(by_hash[h])
                       for address, h in zip(users, user_hashes))
            operator_delta = sum(after[a] - before[a] for a in owners)
            builder_delta = after[builder] - before[builder]
            assert operator_delta == funding - post_cost - payout_cost
            assert builder_delta == tips - funding - funding_cost
            assert (builder_delta > 0) == expect_builder_profit
            burned = sum(cost(r) for r in receipts) - tips
            assert sum(after[a] - before[a] for a in before) == -burned
            if sponsored:
                assert operator_delta > assumed_non_gas_cost
            else:
                assert operator_delta < 0 and funding == 0
            row = {
                "name": name, "requested_value_wei": amount, "builder_funding_wei": funding,
                "treasury_receipt_wei": after[treasury] - before[treasury],
                "post_cost_wei": post_cost, "payout_cost_wei": payout_cost,
                "funding_tx_cost_wei": funding_cost, "user_tips_wei": tips,
                "operator_combined_balance_change_wei": operator_delta,
                "contribution_after_assumed_non_gas_cost_wei": operator_delta - assumed_non_gas_cost,
                "builder_balance_change_wei": builder_delta,
                "builder_economically_positive": expect_builder_profit,
                "local_only_negative_control": not expect_builder_profit,
                "payout_sender_balance_before_wei": before[payout_sender],
                "payout_sender_balance_after_wei": after[payout_sender],
                "balances_before_wei": before, "balances_after_wei": after,
                "txpool_only_balance_override": queue_override, "transaction_hashes": txs,
                "post_hash": post_hash, "payout_hash": payout_hash, "funding_hash": funding_hash,
                "block_hash": block["hash"], "target_timestamp": timestamp,
            }
            record["cases"].append(row)
            (work / f"{name}-receipts.json").write_text(json.dumps(receipts, indent=2) + "\n")
            print(f"PASS: {name}: operator delta {operator_delta}; builder delta {builder_delta} wei", flush=True)

        value_batch("value_sponsored_first_batch", amount=payout_value, sponsored=True,
                    expect_builder_profit=True)
        value_batch("value_sponsored_residual_balance", amount=payout_value, sponsored=True,
                    expect_builder_profit=True)
        rpc("anvil_setBalance", [payout_sender, hex(payout_reserve)])
        value_batch("value_prefunded_self_payment", amount=payout_value, sponsored=False,
                    expect_builder_profit=True)
        value_batch("value_oversized_payout_builder_loss", amount=10_000_000_000_000_000,
                    sponsored=True, expect_builder_profit=False)

        # Both builder transfers below are manually supplied by this harness.
        # Testing their composition cannot establish a provider's refund policy.
        # A separate treasury keeps a received refund out of the next funding
        # shortfall; the same-poster controls exercise the opposite cash flow.
        post_tip = 20_000_000_000
        tip_gas_limit = 100_000
        refund_percent = 99
        refund_transfer_cost = 21_000 * BASE_FEE
        tip_reserve = tip_gas_limit * (post_tip + BASE_FEE) + 131_072 * BLOB_CAP
        record["sponsored_tip_assumptions"] = {
            "post_priority_fee_wei": post_tip,
            "post_gas_limit": tip_gas_limit,
            "refund_percent": refund_percent,
            "refund_formula": "floor(max(0, refund_base - refund_transfer_cost) * percent / 100)",
            "refund_formula_note": "Titan's displayed deduction order, not a verified combined sponsorship policy.",
            "funding_policy": "Full signed execution/blob fee-cap shortfall; manually funded by the harness.",
            "assumed_non_gas_cost_wei": assumed_non_gas_cost,
            "unverified": "Automatic sponsorship, selected-post eligibility, and refund accounting at any live builder.",
        }

        def sponsored_tip_batch(name, *, sponsored, recipient, expected_builder_profit,
                                gas_limit=tip_gas_limit, net_sponsorship_from_source=False):
            accounts = [builder, poster, treasury, *users]
            before = {address: balance(address) for address in accounts}
            reserve = gas_limit * (post_tip + BASE_FEE) + 131_072 * BLOB_CAP
            funding = max(0, reserve - before[poster]) if sponsored else 0
            raw_post, timestamp = prepare_post(tip=post_tip, gas=gas_limit)
            raw_users = [sign(key, probe, consume, gas=100_000, tip=50_000_000_000)
                         for key in KEYS[2:5]]
            parent_hash = rpc("eth_getBlockByNumber", ["latest", False])["hash"]

            # Determine the refund from actual post gas, then revert the entire
            # provisional block. Its balance override supplies no mined funds.
            snapshot = rpc("evm_snapshot", [])
            rpc("anvil_setBalance", [poster, hex(max(reserve, before[poster]))])
            simulation_hash = rpc("eth_sendRawTransaction", [raw_post])
            for raw_user in raw_users:
                rpc("eth_sendRawTransaction", [raw_user])
            mine(timestamp)
            simulated_post = rpc("eth_getTransactionReceipt", [simulation_hash])
            assert simulated_post and simulated_post["status"] == "0x1", simulated_post
            simulated_tip = (int(simulated_post["gasUsed"], 16)
                             * (int(simulated_post["effectiveGasPrice"], 16) - BASE_FEE))
            assert rpc("evm_revert", [snapshot])
            assert rpc("eth_getBlockByNumber", ["latest", False])["hash"] == parent_hash
            assert all(balance(address) == before[address] for address in accounts)
            refund_base = max(0, simulated_tip - funding) if net_sponsorship_from_source else simulated_tip
            refund = max(0, refund_base - refund_transfer_cost) * refund_percent // 100
            builder_nonce = int(rpc("eth_getTransactionCount", [builder, "latest"]), 16)
            txs = []
            funding_hash = None
            if funding:
                funding_hash = rpc("eth_sendRawTransaction", [sign(
                    KEYS[0], poster, value=funding, gas=21_000, nonce=builder_nonce)])
                txs.append(funding_hash)
            response = rpc("eth_sendRawTransaction", [raw_post], error_ok=True)
            queue_override = isinstance(response, dict)
            if queue_override:
                assert funding > 0 and "fund" in response["error"]["message"].lower(), response
                rpc("anvil_setBalance", [poster, hex(reserve)])
                response = rpc("eth_sendRawTransaction", [raw_post])
                rpc("anvil_setBalance", [poster, hex(before[poster])])
            post_hash = response
            assert post_hash == simulation_hash
            txs.append(post_hash)
            user_hashes = [rpc("eth_sendRawTransaction", [raw_user]) for raw_user in raw_users]
            txs.extend(user_hashes)
            refund_hash = None
            if refund:
                refund_hash = rpc("eth_sendRawTransaction", [sign(
                    KEYS[0], recipient, value=refund, gas=21_000,
                    nonce=builder_nonce + int(funding > 0))])
                txs.append(refund_hash)
            assert all(balance(address) == before[address] for address in accounts)
            mine(timestamp)
            receipts = [rpc("eth_getTransactionReceipt", [tx]) for tx in txs]
            assert all(r and r["status"] == "0x1" for r in receipts), receipts
            block = rpc("eth_getBlockByNumber", [receipts[0]["blockNumber"], False])
            assert block["transactions"] == txs
            by_hash = {r["transactionHash"]: r for r in receipts}

            def cost(receipt):
                return (int(receipt["gasUsed"], 16) * int(receipt["effectiveGasPrice"], 16)
                        + int(receipt.get("blobGasUsed", "0x0"), 16)
                        * int(receipt.get("blobGasPrice", "0x0"), 16))

            post = by_hash[post_hash]
            assert post["type"] == "0x3" and int(post["blobGasUsed"], 16) == 131_072
            actual_tip = int(post["gasUsed"], 16) * (int(post["effectiveGasPrice"], 16) - BASE_FEE)
            assert actual_tip == simulated_tip
            post_cost = cost(post)
            tips = sum(int(by_hash[h]["gasUsed"], 16)
                       * (int(by_hash[h]["effectiveGasPrice"], 16) - BASE_FEE) for h in user_hashes)
            funding_cost = cost(by_hash[funding_hash]) if funding_hash else 0
            refund_cost = cost(by_hash[refund_hash]) if refund_hash else 0
            after = {address: balance(address) for address in accounts}
            assert after[poster] - before[poster] == funding - post_cost + (refund if recipient == poster else 0)
            assert after[treasury] - before[treasury] == (refund if recipient == treasury else 0)
            for address, user_hash in zip(users, user_hashes):
                assert after[address] - before[address] == -cost(by_hash[user_hash])
            operator_delta = sum(after[a] - before[a] for a in [poster, treasury])
            builder_delta = after[builder] - before[builder]
            assert operator_delta == funding + refund - post_cost
            assert builder_delta == tips + actual_tip - funding - refund - funding_cost - refund_cost
            assert (builder_delta > 0) == expected_builder_profit
            burned = sum(cost(r) for r in receipts) - tips - actual_tip
            assert sum(after[a] - before[a] for a in accounts) == -burned
            if not sponsored:
                assert funding == 0 and operator_delta < 0
            if net_sponsorship_from_source:
                assert funding >= actual_tip and refund == 0
            elif sponsored and recipient == treasury:
                assert refund > assumed_non_gas_cost
            row = {
                "name": name, "manual_builder_funding_and_refund": True,
                "gross_post_tip_wei": actual_tip, "refund_basis_wei": refund_base,
                "net_sponsorship_from_source_assumption": net_sponsorship_from_source,
                "builder_funding_wei": funding, "refund_wei": refund,
                "refund_recipient": recipient, "poster_fee_cap_reserve_wei": reserve,
                "post_cost_wei": post_cost, "funding_cost_wei": funding_cost,
                "refund_transfer_cost_wei": refund_cost, "external_user_tips_wei": tips,
                "operator_combined_balance_change_wei": operator_delta,
                "contribution_after_assumed_non_gas_cost_wei": operator_delta - assumed_non_gas_cost,
                "treasury_receipt_wei": after[treasury] - before[treasury],
                "poster_balance_change_wei": after[poster] - before[poster],
                "builder_balance_change_wei": builder_delta,
                "builder_economically_positive": expected_builder_profit,
                "local_only_negative_control": not expected_builder_profit,
                "balances_before_wei": before, "balances_after_wei": after,
                "txpool_only_balance_override": queue_override,
                "transaction_hashes": txs, "post_hash": post_hash,
                "funding_hash": funding_hash, "refund_hash": refund_hash,
                "block_hash": block["hash"], "target_timestamp": timestamp,
            }
            record["cases"].append(row)
            (work / f"{name}-receipts.json").write_text(json.dumps(receipts, indent=2) + "\n")
            print(f"PASS: {name}: operator delta {operator_delta}; builder delta {builder_delta} wei", flush=True)
            return row

        rpc("anvil_setBalance", [poster, "0x0"])
        first_tip = sponsored_tip_batch("tip_refund_sponsored_first", sponsored=True, recipient=treasury,
                                        expected_builder_profit=True)
        recurring = sponsored_tip_batch("tip_refund_sponsored_repeat", sponsored=True, recipient=treasury,
                                        expected_builder_profit=True)
        reserve_shift = first_tip["post_cost_wei"] - recurring["post_cost_wei"]
        assert recurring["poster_balance_change_wei"] == reserve_shift
        assert recurring["operator_combined_balance_change_wei"] == recurring["refund_wei"] + reserve_shift
        net_control = sponsored_tip_batch("tip_refund_sponsorship_deducted_from_source", sponsored=True,
                                         recipient=treasury, expected_builder_profit=True,
                                         net_sponsorship_from_source=True)
        assert net_control["treasury_receipt_wei"] == 0
        rpc("anvil_setBalance", [poster, hex(tip_reserve * 2)])
        sponsored_tip_batch("tip_refund_prefunded_no_sponsor", sponsored=False, recipient=treasury,
                            expected_builder_profit=True)
        rpc("anvil_setBalance", [poster, "0x0"])
        same_first = sponsored_tip_batch("tip_refund_same_poster_first", sponsored=True, recipient=poster,
                                         expected_builder_profit=True)
        same_repeat = sponsored_tip_batch("tip_refund_same_poster_repeat", sponsored=True, recipient=poster,
                                          expected_builder_profit=True)
        assert same_repeat["treasury_receipt_wei"] == 0
        # The standing reserve may shift with intrinsic calldata gas between
        # targets. That is not the full refund becoming recurring service income.
        expected_shift = ((same_first["post_cost_wei"] - same_first["refund_wei"])
                          - (same_repeat["post_cost_wei"] - same_repeat["refund_wei"]))
        assert same_repeat["operator_combined_balance_change_wei"] == expected_shift
        rpc("anvil_setBalance", [poster, "0x0"])
        sponsored_tip_batch("tip_refund_oversized_reserve_builder_loss", sponsored=True, recipient=treasury,
                            expected_builder_profit=False, gas_limit=GAS_LIMIT)
        record["status"] = "passed"
    except Exception as error:
        record["status"] = "failed"
        record["error"] = repr(error)
        raise
    finally:
        (work / "results.json").write_text(json.dumps(record, indent=2) + "\n")
        if process is not None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=10)
        print(f"Retained results: {work / 'results.json'}", flush=True)


if __name__ == "__main__":
    main()
