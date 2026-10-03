"""Local sample audit + independent python-can comparison. Does not modify samples.

python -m pip install python-can==4.6.1
python scripts/verify_samples.py --exe target/release/canlog.exe
"""
import argparse
import hashlib
import json
import platform
import struct
import subprocess
import time
from pathlib import Path

import can

ROOT = Path(__file__).resolve().parent.parent
LOSSES = ["unsupported-record", "corrupted-region", "field:format-metadata", "field:source-metadata"]


def run(exe, *args, accepted=(0, 3)):
    process = subprocess.run([str(exe), *map(str, args)], capture_output=True, text=True, encoding="utf-8")
    if process.returncode not in accepted:
        raise RuntimeError(f"{args}: exit {process.returncode}: {process.stderr[-2000:]}")
    return process


def engine_frames(exe, source):
    process = run(exe, "view", source, "--unlimited", "--unsupported", "skip", "--recover")
    return [json.loads(line)["frame"] for line in process.stdout.splitlines() if line.strip()]


def key(frame):
    return (frame["channel"], frame["id"], frame["extended"], frame["direction"], frame["remote"],
            frame["fd"], frame["raw_dlc"], tuple(frame["data"]), frame["brs"], frame["esi"], frame["timestamp_ns"])


def reference_compare(source, frames, file_format):
    reader = can.BLFReader(source) if file_format == "blf" else can.ASCReader(source, relative_timestamp=True)
    origin = reader.start_timestamp if file_format == "blf" else 0
    messages = [message for message in reader if not message.is_error_frame]
    if len(messages) != len(frames):
        raise AssertionError(f"reference frame count {len(messages)} != engine {len(frames)}")
    max_time_error = 0
    reference_differences = []
    for i, (message, frame) in enumerate(zip(messages, frames)):
        expected = (message.channel + 1, message.arbitration_id, message.is_extended_id,
                    "rx" if message.is_rx else "tx", message.is_remote_frame, message.is_fd,
                    bytes(message.data), message.bitrate_switch, message.error_state_indicator)
        actual = (frame["channel"], frame["id"], frame["extended"], frame["direction"], frame["remote"],
                  frame["fd"], bytes(frame["data"]), frame["brs"], frame["esi"])
        if file_format == "asc" and frame["fd"] and frame["raw_dlc"] == 0 and message.is_remote_frame:
            reference_differences.append({"frame": i, "reason": "python-can 4.6.1 ASC reader labels zero-length CAN FD as remote; core rejects FD remote"})
            expected = expected[:4] + (False,) + expected[5:]
        if expected != actual:
            raise AssertionError(f"reference mismatch at frame {i}: {expected} != {actual}")
        error = abs(round((message.timestamp - origin) * 1e9) - frame["timestamp_ns"])
        max_time_error = max(max_time_error, error)
        # Epoch float64 quantization is a limitation of the reference, not a Rust tolerance.
        if error > (500 if file_format == "blf" else 1):
            raise AssertionError(f"timestamp difference at frame {i}: {error}ns")
    return {"frames_compared": len(messages), "max_reference_time_error_ns": max_time_error,
            "raw_dlc_checked_by_engine_fixture": True, "reference_differences": reference_differences}


def audit(exe, directory):
    rows = []
    for source in sorted((ROOT / "can_example").rglob("*")):
        if not source.is_file() or source.suffix.lower() not in (".asc", ".blf"):
            continue
        row = {"path": source.relative_to(ROOT).as_posix(), "bytes": source.stat().st_size,
               "sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
               "source_license": "user-provided local sample; redistribution unverified"}
        try:
            p = run(exe, "stats", source, "--unsupported", "skip", "--recover")
            report = json.loads(p.stdout)
            row.update({"status": report["status"], "frames": report["frames_selected"],
                        "issues": report["issues"], "issue_counts": report["issue_counts"],
                        "metadata": report["metadata"], "first_timestamp_ns": report["first_timestamp_ns"],
                        "last_timestamp_ns": report["last_timestamp_ns"], "regressions": report["regressions"],
                        "issue_examples": report["issue_examples"][:5]})
            original = engine_frames(exe, source)
            stem = source.relative_to(ROOT / "can_example").as_posix().replace("/", "_")
            blf = directory / f"{stem}.blf"
            asc = directory / f"{stem}.asc"
            options = ["--unsupported", "skip", "--recover"]
            for loss in LOSSES:
                options.extend(["--allow-loss", loss])
            record_report = directory / f"{stem}.record-report.json"
            run(exe, "record", "--input", source, "-o", blf, "--report", record_report, *options)
            row["actual_record_losses"] = json.loads(record_report.read_text(encoding="utf-8"))["losses"]
            normalized = engine_frames(exe, blf)
            if list(map(key, original)) != list(map(key, normalized)):
                raise AssertionError("source -> recorded BLF semantic mismatch")
            run(exe, "replay", blf, "--no-wait", "--on-regression", "immediate", "-o", asc)
            if list(map(key, original)) != list(map(key, engine_frames(exe, asc))):
                raise AssertionError("recorded BLF -> replayed ASC semantic mismatch")
            row["roundtrip"] = "pass (CAN frames; declared losses separate)"
            row["external_generated_blf"] = reference_compare(blf, original, "blf")
            row["external_generated_asc"] = reference_compare(asc, original, "asc")
            if source.suffix.lower() == ".blf":
                row["external_original"] = reference_compare(source, original, "blf")
            else:
                # python-can cannot resolve CANoe symbolic rows and does not accumulate relative delta rows.
                text = source.read_text(encoding="utf-8", errors="replace")
                if "timestamps relative" in text or " ID = " in text or "UnresolvedId" in row["issue_counts"]:
                    row["external_original"] = "reference dialect differs; numeric canonical output compared"
                else:
                    row["external_original"] = reference_compare(source, original, "asc")
        except Exception as exc:
            row["validation_error"] = str(exc)
        rows.append(row)
    return rows


def synthetic_profiles(exe, directory):
    lengths = [0, 1, 2, 3, 4, 5, 6, 7, 8, 12, 16, 20, 24, 32, 48, 64]
    frames = []
    for dlc, length in enumerate(lengths):
        frames.append(dict(timestamp_ns=dlc * 100000001, channel=65535, id=0x1fffffff,
                           extended=True, direction="tx", remote=False, fd=True, raw_dlc=dlc,
                           data=list(range(length)), brs=True, esi=True))
    for dlc in range(16):
        frames.append(dict(timestamp_ns=(16 + dlc) * 100000001, channel=1, id=0x7ff,
                           extended=False, direction="rx", remote=False, fd=False, raw_dlc=dlc,
                           data=list(range(min(dlc, 8))), brs=False, esi=False))
    frames.append(dict(timestamp_ns=32 * 100000001, channel=2, id=0x7ff, extended=True,
                       direction="tx", remote=True, fd=False, raw_dlc=15, data=[], brs=False, esi=False))
    source = directory / "synthetic.jsonl"
    source.write_text("".join(json.dumps({"schema_version": 1, "frame": f}) + "\n" for f in frames), encoding="ascii")
    evidence = {}
    for fmt in ("asc", "blf"):
        output = directory / f"synthetic.{fmt}"
        run(exe, "record", "--input", source, "-o", output)
        assert list(map(key, engine_frames(exe, output))) == list(map(key, frames))
        evidence[fmt] = reference_compare(output, frames, fmt)
    # Independently generated v2 header + FD64 body, including BRS/ESI/extended/Tx.
    payload = bytes(range(12))
    obj = struct.pack("<4sHHII", b"LOBJ", 40, 2, 40 + 40 + 12, 101)
    obj += struct.pack("<IBxHQ8x", 2, 0, 0, 1234)
    obj += struct.pack("<BBBBIIIIIIIHBBI", 7, 9, 12, 0, 0x9fffffff, 0, 0x7000, 0, 0, 0, 0, 0, 1, 0, 0) + payload
    container = struct.pack("<4sHHIIH6xI4x", b"LOBJ", 16, 1, 32 + len(obj), 10, 0, len(obj)) + obj
    header = bytearray(144); header[:4] = b"LOGG"
    struct.pack_into("<I", header, 4, 144); struct.pack_into("<Q", header, 16, 144 + len(container)); struct.pack_into("<I", header, 32, 1)
    fd64 = directory / "independent-fd64-v2.blf"; fd64.write_bytes(header + container)
    expected = dict(timestamp_ns=1234, channel=7, id=0x1fffffff, extended=True, direction="tx",
                    remote=False, fd=True, raw_dlc=9, data=list(payload), brs=True, esi=True)
    assert list(map(key, engine_frames(exe, fd64))) == [key(expected)]
    evidence["independent_fd64_v2"] = reference_compare(fd64, [expected], "blf")
    return evidence


def synthetic_and_timing(exe, directory):
    fixture = directory / "timing.asc"
    fixture.write_text("base hex timestamps absolute\n0.100000001 1 100 Rx d 1 AA\n0.300000001 1 100x Tx d 0\n0.500000001 CANFD 1 Rx 200 1 1 9 12 00 01 02 03 04 05 06 07 08 09 0A 0B\n", encoding="ascii")
    evidence = []
    for speed, expected in [(1, .4), (2, .2)]:
        start = time.perf_counter()
        p = run(exe, "replay", fixture, "--speed", speed)
        elapsed = time.perf_counter() - start
        frames = [json.loads(line)["frame"] for line in p.stdout.splitlines()]
        assert [f["timestamp_ns"] for f in frames] == [100000001, 300000001, 500000001]
        assert expected - .02 <= elapsed <= expected + .35, (speed, elapsed)
        evidence.append({"speed": speed, "expected_interval_s": expected, "elapsed_s": elapsed, "frames": len(frames)})
    start = time.perf_counter()
    run(exe, "replay", fixture, "--no-wait")
    evidence.append({"mode": "no-wait", "elapsed_s": time.perf_counter() - start})
    # Exercise the real control path, not just the fake clock.
    child = subprocess.Popen([str(exe), "replay", str(fixture), "--control-stdin"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    first = child.stdout.readline()
    assert json.loads(first)["frame"]["timestamp_ns"] == 100000001
    start = time.perf_counter()
    child.stdin.write("pause\n"); child.stdin.flush()
    time.sleep(.15)
    child.stdin.write("resume\n"); child.stdin.flush()
    rest, errors = child.communicate(timeout=3)
    elapsed = time.perf_counter() - start
    assert child.returncode == 0, errors
    assert len(rest.splitlines()) == 2 and elapsed >= .50
    evidence.append({"mode": "pause-resume", "pause_s": .15, "elapsed_s_after_first_frame": elapsed})
    return evidence


def benchmark(exe, directory):
    import ctypes
    from ctypes import wintypes
    class Counters(ctypes.Structure):
        _fields_ = [("cb", wintypes.DWORD), ("PageFaultCount", wintypes.DWORD)] + [(name, ctypes.c_size_t) for name in
                   ["PeakWorkingSetSize", "WorkingSetSize", "QuotaPeakPagedPoolUsage", "QuotaPagedPoolUsage", "QuotaPeakNonPagedPoolUsage", "QuotaNonPagedPoolUsage", "PagefileUsage", "PeakPagefileUsage"]]
    get_info = ctypes.WinDLL("psapi").GetProcessMemoryInfo
    get_info.argtypes = [wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD]
    rows = []
    for frames in (100_000, 1_000_000):
        source = directory / f"bench-{frames}.asc"
        with source.open("w", encoding="ascii", newline="\n") as output:
            output.write("base hex timestamps absolute\n")
            for i in range(frames):
                output.write(f"{i//1000}.{i%1000:03d} 1 100 Rx d 8 01 02 03 04 05 06 07 08\n")
        blf = directory / f"bench-{frames}.blf"
        run(exe, "record", "--input", source, "-o", blf)
        for input_file in (source, blf):
            start = time.perf_counter()
            process = subprocess.Popen([str(exe), "stats", str(input_file)], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            peak = 0
            while process.poll() is None:
                counters = Counters(); counters.cb = ctypes.sizeof(counters)
                if get_info(int(process._handle), ctypes.byref(counters), counters.cb):
                    peak = max(peak, counters.PeakWorkingSetSize)
                time.sleep(.01)
            out, err = process.communicate()
            assert process.returncode == 0, err.decode()
            result = json.loads(out)
            elapsed = time.perf_counter() - start
            assert result["frames_selected"] == frames
            assert 0 < peak < 64 * 1024 * 1024, peak
            start = time.perf_counter(); run(exe, "view", input_file, "--limit", 1)
            first_latency = time.perf_counter() - start
            rows.append({"format": input_file.suffix[1:], "frames": frames, "bytes": input_file.stat().st_size, "elapsed_s": elapsed,
                         "frames_per_s": frames / elapsed, "peak_working_set_bytes": peak, "first_frame_process_latency_s": first_latency,
                         "sha256": hashlib.sha256(input_file.read_bytes()).hexdigest()})
    for fmt in ("asc", "blf"):
        pair = [row for row in rows if row["format"] == fmt]
        assert pair[1]["peak_working_set_bytes"] <= pair[0]["peak_working_set_bytes"] + 16 * 1024 * 1024
    return rows


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", type=Path, default=ROOT / "target/release/canlog.exe")
    parser.add_argument("--skip-benchmark", action="store_true")
    args = parser.parse_args()
    assert can.__version__ == "4.6.1"
    directory = ROOT / "artifacts" / f"verification-{time.time_ns()}"
    directory.mkdir(parents=True)
    samples = audit(args.exe.resolve(), directory)
    result = {"platform": platform.platform(), "python_can": can.__version__, "cli": str(args.exe.resolve()),
              "samples": samples, "synthetic_profiles": synthetic_profiles(args.exe.resolve(), directory),
              "timing": synthetic_and_timing(args.exe.resolve(), directory),
              "benchmark": [] if args.skip_benchmark else benchmark(args.exe.resolve(), directory)}
    report = directory / "verification.json"
    report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    failures = [row for row in samples if "validation_error" in row]
    print(json.dumps({"report": str(report), "samples": len(samples), "frames": sum(row.get("frames", 0) for row in samples),
                      "failures": [{"path": row["path"], "error": row["validation_error"]} for row in failures],
                      "timing": result["timing"], "benchmark": result["benchmark"]}, ensure_ascii=False, indent=2))
    raise SystemExit(bool(failures))


if __name__ == "__main__":
    main()
