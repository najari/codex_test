"""Compare sparse indexes against full scans and DBC values against cantools.

The Python dependencies are verification oracles, never canlog runtime dependencies.
"""
import argparse
import hashlib
import json
import math
import subprocess
import time
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def seconds(ns):
    return f'{ns // 1_000_000_000}.{ns % 1_000_000_000:09}'


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--out', required=True, type=Path)
    parser.add_argument('--manifest', action='append', required=True, type=Path)
    parser.add_argument('--exe', type=Path, default=ROOT/'dist/canlog.exe')
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    exe = args.exe.resolve()
    commands = []
    results = []
    decode_results = []
    serial = 0
    common = ['--unsupported', 'skip', '--recover']

    def invoke(label, *argv):
        nonlocal serial
        serial += 1
        prefix = f'{serial:03}_{label}'
        started = time.perf_counter()
        stdout = out/(prefix+'.stdout.txt')
        stderr = out/(prefix+'.stderr.txt')
        with stdout.open('wb') as so, stderr.open('wb') as se:
            process = subprocess.run([str(exe), *map(str, argv)], stdout=so, stderr=se, timeout=120)
        commands.append({'label': label, 'argv': [str(exe), *map(str, argv)],
                         'exit_code': process.returncode, 'elapsed_s': time.perf_counter()-started,
                         'stdout': stdout.name, 'stderr': stderr.name})
        return process.returncode, stdout, stderr

    def require(run):
        code, so, se = run
        assert code in (0, 3), f'exit {code}: {se.read_text(encoding="utf-8")[-2000:]}'
        return code, so, se

    def rows(p):
        return [json.loads(l) for l in p.read_text(encoding='utf-8').splitlines() if l.strip()]

    serial_log = 0
    inputs = []
    for manifest in args.manifest:
        manifest = manifest.resolve()
        document = json.loads(manifest.read_text(encoding='utf-8'))
        entries = document.get('files')
        if entries is None:
            entries = [f for g in document['groups'] for f in g['files']]
        for entry in entries:
            path = manifest.parent/entry['local_path']
            assert sha(path) == entry['sha256'], f'source hash changed: {path}'
            if path.suffix.lower() in ('.asc', '.blf') and entry.get('role') in ('primary', 'log'):
                inputs.append((path, entry['sha256']))
    for source, expected_hash in inputs:
        serial_log += 1
        prefix = f'log{serial_log:02}'
        row = {'source': str(source), 'sha256': expected_hash}
        db = out/(prefix+'.sqlite')
        build = invoke(prefix+'_build', 'index', 'build', source, '-o', db, '--stride', '128', *common)
        full_report = out/(prefix+'_full-report.json')
        baseline = invoke(prefix+'_full', 'view', source, '--unlimited', '--report', full_report, *common)
        if baseline[0] not in (0, 3):
            assert build[0] not in (0, 3) and not db.exists(), 'invalid source published an index'
            row.update(status='source_rejected', diagnostic=baseline[2].read_text(encoding='utf-8')[-2000:])
            results.append(row)
            print(json.dumps({'source': str(source), 'status': row['status']}), flush=True)
            continue
        require(build)
        frames = rows(baseline[1])
        row['frames'] = len(frames)
        times = sorted(f['frame']['timestamp_ns'] for f in frames)
        windows = [('all', [])]
        if times:
            middle = times[len(times)//2]
            last = times[-1]
            windows += [('middle', ['--start', seconds(middle), '--end', seconds(min(last+1, middle+10_000_000))]),
                        ('last', ['--start', seconds(last), '--end', seconds(last+1)])]
        else:
            windows += [('empty', ['--start', '1', '--end', '2'])]
        row['windows'] = []
        for label, selection in windows:
            expected_report = full_report if not selection else out/(prefix+'_'+label+'-full-report.json')
            full = baseline if not selection else require(invoke(prefix+'_'+label+'_full', 'view', source,
                        '--unlimited', '--report', expected_report, *common, *selection))
            query_report = out/(prefix+'_'+label+'-query-report.json')
            query = require(invoke(prefix+'_'+label+'_query', 'index', 'query', source, '--index', db,
                        '--report', query_report, *common, *selection))
            assert rows(query[1]) == rows(full[1]), f'frame/location mismatch: {source} {label}'
            expected = json.loads(expected_report.read_text(encoding='utf-8'))
            actual = json.loads(query_report.read_text(encoding='utf-8'))
            assert actual['issues_selected'] == expected['issues']-expected['issues_outside_selection'], f'quality mismatch: {source} {label}'
            assert query[0] == full[0], f'status mismatch: {source} {label}'
            assert actual['selection_complete']
            row['windows'].append({'label': label, 'status': 'pass', 'frames': actual['frames_selected'],
                'issues_selected': actual['issues_selected'], 'frames_examined': actual['frames_examined'], 'chunks_read': actual['chunks_read']})
        row['status'] = 'pass'
        assert sha(source) == expected_hash
        results.append(row)
        print(json.dumps({'source': str(source), 'status': 'pass', 'frames': len(frames)}), flush=True)

    import cantools
    web = ROOT/'can_example/web_downloads/2026-10-03'
    cases = [
        ('motorola', web/'atemall_motorola/motorola_matrix.asc', [(1, web/'atemall_motorola/motorola_matrix.dbc')]),
        ('model3', web/'model3_can/CAN.asc', [(1, web/f'model3_can/Model3_{part}.dbc') for part in ('Battery', 'Drive', 'Thermal')]),
        ('easy', web/'py_canoe_demo/demo_log.blf', [(c, web/'py_canoe_demo/easy.dbc') for c in (1, 2)]),
    ]
    for name, source, assignments in cases:
        db = out/(name+'.sqlite')
        require(invoke(name+'_build', 'index', 'build', source, '-o', db, '--stride', '128', *common))
        dbc_flags = [arg for channel, p in assignments for arg in ('--dbc', f'{channel}={p}')]
        decoded = require(invoke(name+'_decode', 'decode', source, *dbc_flags, *common, '--report', out/(name+'-decode-report.json')))
        indexed = require(invoke(name+'_indexed_decode', 'decode', source, *dbc_flags, *common, '--index', db,
                            '--report', out/(name+'-indexed-decode-report.json')))
        actual = rows(decoded[1])
        assert actual == rows(indexed[1]), f'index decode mismatch: {name}'
        oracles = {}
        diagnostics = []
        for channel, dbc_path in assignments:
            try:
                oracle = cantools.database.load_file(str(dbc_path), strict=True)
            except Exception:
                # cantools 43 rejects this CANoe environment declaration; preserve
                # the original file and filter only the unrelated in-memory statement.
                assert name == 'easy'
                import re
                text = dbc_path.read_text(encoding='cp1252')
                text = re.sub(r'^ENVVAR_DATA_\s+[^;]+;\s*$', '', text, flags=re.MULTILINE)
                oracle = cantools.database.load_string(text, database_format='dbc', strict=True)
                diagnostics.append('cantools oracle only: ignored ENVVAR_DATA_ declaration in memory')
            for message in oracle.messages:
                key = (channel, message.frame_id, message.is_extended_frame)
                assert key not in oracles
                oracles[key] = message
        signal_count = 0
        for record in actual:
            assert record['status'] == 'decoded', record
            frame = record['record']['frame']
            message = oracles[(frame['channel'], frame['id'], frame['extended'])]
            physical = message.decode(bytes(frame['data']), decode_choices=False, scaling=True)
            raw = message.decode(bytes(frame['data']), decode_choices=False, scaling=False)
            valid = {s['name']: s for s in record['signals'] if s['status'] == 'valid'}
            assert set(valid) == set(physical), f'active signal set: {name} {message.name}'
            for signal_name, sample in valid.items():
                assert math.isclose(sample['physical'], physical[signal_name], rel_tol=1e-10, abs_tol=1e-9), (name, signal_name, sample, physical[signal_name])
                if sample['raw']['type'] != 'float_bits':
                    assert sample['raw']['value'] == raw[signal_name], (name, signal_name)
                signal_count += 1
        decode_results.append({'name': name, 'status': 'pass', 'frames': len(actual), 'signals_compared': signal_count,
                               'oracle': f'cantools {cantools.__version__}', 'diagnostics': sorted(set(diagnostics))})
        print(json.dumps(decode_results[-1]), flush=True)

    for source, expected_hash in inputs:
        assert sha(source) == expected_hash
    summary = {'verified_at_utc': datetime.now(timezone.utc).isoformat(), 'exe_sha256': sha(exe),
        'sources_unchanged': True, 'index_counts': dict(Counter(r['status'] for r in results)),
        'index_results': results, 'decode_results': decode_results, 'commands': commands}
    (out/'results.json').write_text(json.dumps(summary, indent=2, ensure_ascii=False)+'\n', encoding='utf-8')
    (out/'README.md').write_text('# Sparse index and DBC validation\n\n'+
        f"Index results: {summary['index_counts']}.\n\n"+
        '\n'.join(f"- {r['name']}: {r['frames']} frames, {r['signals_compared']} signals compared with {r['oracle']}." for r in decode_results)+
        f'\n\n{len(commands)} commands; complete evidence in results.json and individual stdout/stderr files.\n'+
        'All original sample hashes unchanged.\n', encoding='utf-8')


if __name__ == '__main__':
    main()
