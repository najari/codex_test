"""Verify replay against decode and cantools, without editing original samples."""
import argparse
import copy
import hashlib
import json
import math
import re
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path

import cantools

ROOT = Path(__file__).resolve().parent.parent


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def rows(path):
    return [json.loads(line) for line in path.read_text(encoding='utf-8').splitlines() if line.strip()]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--out', required=True, type=Path)
    parser.add_argument('--exe', type=Path, default=ROOT/'dist/canlog.exe')
    parser.add_argument('--samples', type=Path, default=ROOT/'can_example/web_downloads/2026-10-03')
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    exe, web = args.exe.resolve(), args.samples.resolve()
    exe_hash = sha(exe)
    cases = [
        ('motorola', web/'atemall_motorola/motorola_matrix.asc', [(1, web/'atemall_motorola/motorola_matrix.dbc')]),
        ('model3', web/'model3_can/CAN.asc', [(1, web/f'model3_can/Model3_{p}.dbc') for p in ('Battery', 'Drive', 'Thermal')]),
        ('easy', web/'py_canoe_demo/demo_log.blf', [(c, web/'py_canoe_demo/easy.dbc') for c in (1, 2)]),
    ]
    originals = {p: sha(p) for _, source, bindings in cases for p in [source, *(p for _, p in bindings)]}
    commands, results = [], []
    common = ['--unsupported', 'skip', '--recover']
    replay_flags = ['--no-wait', '--on-regression', 'immediate']

    def invoke(label, *argv):
        prefix = f'{len(commands)+1:03}_{label}'
        stdout, stderr = out/(prefix+'.stdout.txt'), out/(prefix+'.stderr.txt')
        started = time.perf_counter()
        with stdout.open('wb') as so, stderr.open('wb') as se:
            process = subprocess.run([str(exe), *map(str, argv)], stdout=so, stderr=se, timeout=120)
        commands.append({'label': label, 'argv': [str(exe), *map(str, argv)], 'exit_code': process.returncode,
                         'elapsed_s': time.perf_counter()-started, 'stdout': stdout.name, 'stderr': stderr.name})
        assert process.returncode in (0, 3), stderr.read_text(encoding='utf-8')[-2000:]
        return stdout, process.returncode

    for name, source, bindings in cases:
        flags = [arg for channel, p in bindings for arg in ('--dbc', f'{channel}={p}')]
        decoded, decode_code = invoke(name+'_decode', 'decode', source, *flags, *common,
                                      '--report', out/f'{name}-decode-report.json')
        replayed, replay_code = invoke(name+'_replay', 'replay', source, *flags, *common, *replay_flags,
                                       '--report', out/f'{name}-replay-report.json')
        expected, actual = rows(decoded), rows(replayed)
        assert actual == expected, f'decode/replay mismatch: {name}'
        assert decode_code == replay_code
        decode_report = json.loads((out/f'{name}-decode-report.json').read_text(encoding='utf-8'))
        replay_report = json.loads((out/f'{name}-replay-report.json').read_text(encoding='utf-8'))
        for field in ('status', 'frames_selected', 'decode_counts', 'engine_revision'):
            assert decode_report[field] == replay_report[field], (name, field)
        assert decode_report['issues_selected'] == replay_report['issues']-replay_report['issues_outside_selection']
        output = out/f'{name}-signals.jsonl'
        invoke(name+'_file', 'replay', source, *flags, *common, *replay_flags, '-o', output, '--sync',
               '--allow-loss', 'unsupported-record', '--allow-loss', 'corrupted-region',
               '--allow-loss', 'field:source-metadata', '--allow-loss', 'field:format-metadata',
               '--report', out/f'{name}-file-report.json')
        assert rows(output) == expected
        file_report = json.loads((out/f'{name}-file-report.json').read_text(encoding='utf-8'))
        assert file_report['published'] and file_report['finalized'] and file_report['durable']
        assert file_report['frames_written'] == len(expected)
        selection = ['--channel', str(bindings[0][0]), '--limit', '10']
        selected, selected_code = invoke(name+'_selected_decode', 'decode', source, *flags, *common, *selection)
        limited, limited_code = invoke(name+'_selected_replay', 'replay', source, *flags, *common, *replay_flags, *selection)
        assert rows(selected) == rows(limited) and selected_code == limited_code
        oracles, diagnostics = {}, []
        for channel, db in bindings:
            try:
                oracle = cantools.database.load_file(str(db), strict=True)
            except Exception:
                assert name == 'easy'
                # Only the in-memory oracle removes the unrelated CANoe declaration.
                text = re.sub(r'^ENVVAR_DATA_\s+[^;]+;\s*$', '', db.read_text(encoding='cp1252'), flags=re.MULTILINE)
                oracle = cantools.database.load_string(text, database_format='dbc', strict=True)
                diagnostics.append('cantools only: ignored ENVVAR_DATA_ in memory')
            for message in oracle.messages:
                key = (channel, message.frame_id, message.is_extended_frame)
                assert key not in oracles
                oracles[key] = message
        signal_count = 0
        for record in actual:
            assert record['status'] == 'decoded'
            frame = record['record']['frame']
            message = oracles[(frame['channel'], frame['id'], frame['extended'])]
            physical = message.decode(bytes(frame['data']), decode_choices=False, scaling=True)
            raw = message.decode(bytes(frame['data']), decode_choices=False, scaling=False)
            valid = {s['name']: s for s in record['signals'] if s['status'] == 'valid'}
            assert set(valid) == set(physical)
            for signal_name, sample in valid.items():
                assert math.isclose(sample['physical'], physical[signal_name], rel_tol=1e-10, abs_tol=1e-9)
                if sample['raw']['type'] != 'float_bits':
                    assert sample['raw']['value'] == raw[signal_name]
                signal_count += 1
        if name == 'motorola':
            repeat, _ = invoke(name+'_repeat', 'replay', source, *flags, *common, *replay_flags,
                               '--speed', '2', '--repeat', '2', '--repeat-gap', '0.1')
            repeated = rows(repeat)
            times = [r['record']['frame']['timestamp_ns'] for r in expected]
            offset = max(times)-times[0]+100_000_000
            second = copy.deepcopy(expected)
            for r in second:
                r['record']['frame']['timestamp_ns'] += offset
            assert repeated == expected+second
            console, _ = invoke(name+'_console', 'replay', source, *flags, *replay_flags, '--limit', '2', '--sink', 'console')
            assert '[decoded]' in console.read_text(encoding='utf-8')
        result = {'name': name, 'frames': len(actual), 'signals_compared': signal_count,
                  'issues_selected': decode_report['issues_selected'], 'status': replay_report['status'],
                  'replay_matches_decode': True, 'file_matches_stdout': True,
                  'oracle': f'cantools {cantools.__version__}', 'diagnostics': sorted(set(diagnostics))}
        results.append(result)
        print(json.dumps(result), flush=True)
    assert all(sha(p) == expected for p, expected in originals.items())
    assert sha(exe) == exe_hash, 'executable changed during verification'
    summary = {'verified_at_utc': datetime.now(timezone.utc).isoformat(), 'exe_sha256': exe_hash,
               'originals_unchanged': True, 'original_hashes': {str(p): s for p, s in originals.items()},
               'results': results, 'commands': commands}
    (out/'results.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2)+'\n', encoding='utf-8')


if __name__ == '__main__':
    main()
