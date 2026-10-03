"""Independent passive payload oracle. Test-only python-can 4.6.1/can-isotp 2.0.7.

Generates traffic with the external ISO-TP transmitter, never with canlog.
Checks complete payloads using both the external passive receiver and canlog CLI.
Artifacts include exact commands, input/binary identities and assertion results.
"""
import argparse
import collections
import hashlib
import importlib.metadata
import json
from pathlib import Path
import subprocess

import can
import isotp


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def address(route, direction):
    endpoint = route[direction]
    peer = route["response" if direction == "request" else "request"]
    bits = "29bits" if endpoint["extended"] else "11bits"
    assert endpoint["extended"] == peer["extended"]
    mode = getattr(isotp.AddressingMode, route["addressing"].title() + "_" + bits)
    kwargs = dict(txid=peer["id"], rxid=endpoint["id"])
    if route["addressing"] == "extended":
        kwargs.update(source_address=endpoint["address"], target_address=peer["address"])
    elif route["addressing"] == "mixed":
        kwargs["address_extension"] = endpoint["address"]
        if endpoint["extended"]:
            # Mixed_29bits has embedded source/target CAN ID fields; the
            # library computes 0x18CE{target}{source} rather than using txid/rxid.
            kwargs = {"source_address": (endpoint["id"] >> 8) & 255,
                      "target_address": endpoint["id"] & 255,
                      "address_extension": endpoint["address"]}
    return isotp.Address(mode, **kwargs)


def receiver(route, direction):
    queue = collections.deque()
    errors = []
    transmissions = []
    stack = isotp.TransportLayer(
        rxfn=lambda _timeout: queue.popleft() if queue else None,
        txfn=transmissions.append,
        address=address(route, direction),
        error_handler=lambda e: errors.append(str(e)),
        params={"listen_mode": True, "max_frame_size": 4095},
    )
    return stack, queue, errors, transmissions


def oracle(messages, routes):
    stacks = {(r["name"], d): (r, receiver(r, d)) for r in routes for d in ("request", "response")}
    result = collections.defaultdict(list)
    for message in messages:
        for key, (route, (stack, queue, errors, transmissions)) in stacks.items():
            endpoint = route[key[1]]
            if message.channel != route["channel"] or message.arbitration_id != endpoint["id"] or message.is_extended_id != endpoint["extended"]:
                continue
            if endpoint.get("address") is not None and (not message.data or message.data[0] != endpoint["address"]):
                continue
            # FC is an observation for canlog; the independent listen-only receiver
            # checks the corresponding incoming data direction only.
            offset = int(route["addressing"] != "normal")
            if message.data[offset] >> 4 == 3:
                continue
            queue.append(isotp.CanMessage(arbitration_id=message.arbitration_id, data=message.data, dlc=len(message.data), extended_id=message.is_extended_id))
            stack.process()
            while stack.available():
                result[key].append(stack.recv().hex().upper())
            assert not errors, (key, errors)
            assert not transmissions, "passive oracle attempted transmission"
    return dict(result)


def generate(route, payload, padding):
    output = []
    queue = collections.deque()
    errors = []
    # Sender sends in request direction, so invert the receiver address.
    stack = isotp.TransportLayer(
        rxfn=lambda _timeout: queue.popleft() if queue else None,
        txfn=output.append,
        address=address(route, "response"),
        error_handler=lambda e: errors.append(str(e)),
        params={"tx_data_length": 8, "tx_padding": padding, "override_receiver_stmin": 0},
    )
    stack.send(payload)
    stack.process()
    if stack.transmitting():
        response = route["response"]
        data = bytes(([response["address"]] if route["addressing"] != "normal" else []) + [0x30, 0, 0])
        queue.append(isotp.CanMessage(arbitration_id=response["id"], data=data, dlc=len(data), extended_id=response["extended"]))
        stack.process()
    assert not errors, errors
    assert not stack.transmitting()
    return output


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", type=Path, default=Path("dist/canlog.exe"))
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--samples", type=Path, default=Path("can_example/asf"))
    args = parser.parse_args()
    assert importlib.metadata.version("can-isotp") == "2.0.7"
    assert can.__version__ == "4.6.1"
    args.artifacts.mkdir(parents=True, exist_ok=False)
    executable = args.exe.resolve()
    binary_hash = sha(executable)
    evidence = {"binary": str(executable), "binary_sha256": binary_hash, "python_can": can.__version__, "can_isotp": "2.0.7", "commands": [], "cases": []}

    def run(command, expected=0):
        proc = subprocess.run([str(executable), *map(str, command)], capture_output=True)
        evidence["commands"].append({"argv": command, "exit": proc.returncode, "stderr": proc.stderr.decode("utf-8", errors="replace")})
        assert proc.returncode == expected, evidence["commands"][-1]
        return proc

    def verify(name, source, config_path, known=None):
        routes = json.loads(config_path.read_text(encoding="utf-8"))["routes"]
        before = {str(p): sha(p) for p in (source, config_path)}
        if source.suffix == ".asc":
            with can.ASCReader(source, relative_timestamp=True) as reader:
                messages = list(reader)
        else:
            with can.BLFReader(source) as reader:
                messages = list(reader)
        # python-can uses zero-based channels; canlog and routes are one-based.
        for message in messages:
            message.channel += 1
        reference = oracle(messages, routes)
        if known is not None:
            assert reference == known
        dest = args.artifacts / (name + ".payloads.jsonl")
        report = args.artifacts / (name + ".report.json")
        run(["isotp", str(source), "--routes", str(config_path), "-o", str(dest), "--report", str(report)])
        records = [json.loads(line) for line in dest.read_text(encoding="utf-8").splitlines()]
        actual = collections.defaultdict(list)
        for record in records:
            if record["kind"] == "payload":
                assert record["status"] == "complete"
                assert record["declared_length"] == record["observed_length"]
                assert len(record["data_hex"]) == 2 * record["declared_length"]
                assert record["protocol_compliance"] == "unknown"
                actual[(record["route"], record["direction"])].append(record["data_hex"])
        assert dict(actual) == reference
        assert len(set(r["result_key"] for r in records)) == len(records)
        metadata = json.loads(report.read_text(encoding="utf-8"))
        assert metadata["scan_complete"] and metadata["published"] and metadata["status"] == "complete"
        assert metadata["frames_examined"] == len(messages)
        assert all(sha(Path(path)) == identity for path, identity in before.items())
        evidence["cases"].append({"name": name, "input_identities": before, "frames": len(messages), "payloads": sum(map(len, actual.values())), "rows": len(records), "matched_frames": metadata["frames_matched"], "verified": True})
        return records

    try:
        demo_config = Path("examples/isotp-physical.routes.json")
        for name in ("ComfortDiagData", "EngineDiagData"):
            source = args.samples / (name + ".asc")
            verify(name, source, demo_config)
            blf = args.artifacts / (name + ".blf")
            run(["convert", str(source), str(blf)])
            verify(name + "_blf", blf, demo_config)
        total_generated = 0
        for mode in ("normal", "extended", "mixed"):
            for extended in (False, True):
                route = {"name": mode + ("29" if extended else "11"), "channel": 1, "kind": "physical", "profile": "classic", "addressing": mode,
                         "request": {"id": 0x18DA10F1 if extended else 0x700, "extended": extended},
                         "response": {"id": 0x18DAF110 if extended else 0x600, "extended": extended}, "timeout_ns": 1_000_000_000}
                if mode != "normal":
                    route["request"]["address"] = 0x11
                    route["response"]["address"] = 0x11 if mode == "mixed" else 0x22
                if mode == "mixed" and extended:
                    route["request"]["id"] = 0x18CE2211
                    route["response"]["id"] = 0x18CE1122
                config = args.artifacts / (route["name"] + ".routes.json")
                config.write_text(json.dumps({"schema_version": 1, "routes": [route]}), encoding="utf-8")
                source = args.artifacts / (route["name"] + ".asc")
                # python-can's ASC header reader consumes the first non-header
                # line unless the events header explicitly terminates the header.
                lines = ["base hex timestamps absolute", "no internal events logged"]
                expected = []
                ordinal = 0
                for padding in (None, 0xaa):
                    for length in (1, 6, 7, 8, 12, 100, 120, 255, 4095):
                        payload = bytes((i * 17 + length) % 256 for i in range(length))
                        expected.append(payload.hex().upper())
                        total_generated += 1
                        for message in generate(route, payload, padding):
                            ordinal += 1
                            can_id = f"{message.arbitration_id:X}" + ("x" if message.is_extended_id else "")
                            lines.append(f"{ordinal / 10000:.4f} 1 {can_id} Rx d {len(message.data)} " + " ".join(f"{b:02X}" for b in message.data))
                source.write_text("\n".join(lines) + "\n", encoding="utf-8")
                verify(route["name"], source, config, {(route["name"], "request"): expected})
        evidence["generated_payloads"] = total_generated
        evidence["payloads_verified"] = sum(case["payloads"] for case in evidence["cases"])
        assert sha(executable) == binary_hash
        evidence["passed"] = True
    finally:
        (args.artifacts / "results.json").write_text(json.dumps(evidence, indent=2, ensure_ascii=False), encoding="utf-8")
    print(json.dumps({k: evidence[k] for k in ("generated_payloads", "payloads_verified", "passed")}))


if __name__ == "__main__":
    main()
