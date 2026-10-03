"""Verify multi-source workspace, persisted bindings and cached DBC results.

Original samples are read-only. Python/cantools are verification dependencies.
"""
import argparse
import hashlib
import json
import math
import re
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def rows(path):
    return [json.loads(line) for line in path.read_text(encoding='utf-8').splitlines() if line.strip()]


def seconds(ns):
    return f'{ns // 1_000_000_000}.{ns % 1_000_000_000:09}'


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--out', required=True, type=Path)
    parser.add_argument('--exe', type=Path, default=ROOT/'dist/canlog.exe')
    parser.add_argument('--samples', type=Path, default=ROOT/'can_example/web_downloads/2026-10-03')
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    exe, web = args.exe.resolve(), args.samples.resolve()
    workspace = out/'workspace'
    cases = [
        ('motorola', web/'atemall_motorola/motorola_matrix.asc', [(1, web/'atemall_motorola/motorola_matrix.dbc')]),
        ('model3', web/'model3_can/CAN.asc', [(1, web/f'model3_can/Model3_{p}.dbc') for p in ('Battery', 'Drive', 'Thermal')]),
        ('easy', web/'py_canoe_demo/demo_log.blf', [(c, web/'py_canoe_demo/easy.dbc') for c in (1, 2)]),
    ]
    sources = {p: sha(p) for _, source, assignments in cases for p in [source, *(p for _, p in assignments)]}
    commands, results = [], []
    common = ['--unsupported', 'skip', '--recover']

    def invoke(label, *argv):
        prefix = f'{len(commands)+1:03}_{label}'
        stdout, stderr = out/(prefix+'.stdout.txt'), out/(prefix+'.stderr.txt')
        started = time.perf_counter()
        with stdout.open('wb') as so, stderr.open('wb') as se:
            process = subprocess.run([str(exe), *map(str, argv)], stdout=so, stderr=se, timeout=120)
        elapsed = time.perf_counter()-started
        commands.append({'label': label, 'argv': [str(exe), *map(str, argv)], 'exit_code': process.returncode,
                         'elapsed_s': elapsed, 'stdout': stdout.name, 'stderr': stderr.name})
        assert process.returncode in (0, 3), stderr.read_text(encoding='utf-8')[-2000:]
        return stdout, elapsed, process.returncode

    invoke('create', 'workspace', 'create', workspace, '--name', 'Downloaded CAN samples')
    for name, source, assignments in cases:
        invoke(name+'_add', 'workspace', 'add', workspace, source, '--name', name)
        flags = [v for channel, path in assignments for v in ('--dbc', f'{channel}={path}')]
        invoke(name+'_bind', 'workspace', 'bind', workspace, name, *flags)
        invoke(name+'_index', 'workspace', 'index', workspace, name, '--stride', '128', *common)
    manifest_sha = sha(workspace/'workspace.json')
    registered_paths = {log['name']: log['path'] for log in json.loads((workspace/'workspace.json').read_text(encoding='utf-8'))['logs']}

    import cantools
    for name, source, assignments in cases:
        # Use the identical registered spelling (Windows canonical \\?\ path)
        # so provenance is compared exactly, rather than stripped from results.
        source = registered_paths[name]
        flags = [v for channel, path in assignments for v in ('--dbc', f'{channel}={path}')]
        outputs, reports, timings = {}, {}, {}
        for mode in ('direct', 'cold', 'warm', 'bypass'):
            report = out/f'{name}-{mode}-report.json'
            argv = ['decode', source, *flags] if mode == 'direct' else ['workspace', 'decode', workspace, name]
            if mode == 'bypass':
                argv += ['--no-cache']
            so, elapsed, code = invoke(name+'_'+mode, *argv, *common, '--report', report)
            outputs[mode] = rows(so)
            reports[mode] = json.loads(report.read_text(encoding='utf-8'))
            timings[mode] = elapsed
            assert code == (3 if reports[mode]['status'] == 'partial' else 0)
        expected = outputs['direct']
        for mode in ('cold', 'warm', 'bypass'):
            assert outputs[mode] == expected, (name, mode, 'decoded rows differ')
            for field in ('status', 'issues_selected', 'issue_counts', 'decode_counts', 'selection_complete'):
                assert reports[mode][field] == reports['direct'][field], (name, mode, field)
        assert reports['cold']['cache']['misses'] == len(expected)
        assert reports['cold']['cache']['hits'] == 0
        assert reports['warm']['cache']['hits'] == len(expected)
        assert reports['warm']['cache']['misses'] == 0
        assert reports['bypass']['cache'] is None

        # A range query must reuse rows yet recompute its selected issue quality.
        times = sorted(r['record']['frame']['timestamp_ns'] for r in expected)
        selection = ['--start', seconds(times[len(times)//2]), '--end', seconds(times[-1]+1)]
        window_outputs, window_reports = [], []
        for mode in ('direct', 'cached'):
            report = out/f'{name}-window-{mode}-report.json'
            argv = ['decode', source, *flags] if mode == 'direct' else ['workspace', 'decode', workspace, name]
            so, _, _ = invoke(name+'_window_'+mode, *argv, *common, *selection, '--report', report)
            window_outputs.append(rows(so))
            window_reports.append(json.loads(report.read_text(encoding='utf-8')))
        assert window_outputs[0] == window_outputs[1]
        for field in ('status', 'issues_selected', 'issue_counts', 'decode_counts', 'selection_complete'):
            assert window_reports[0][field] == window_reports[1][field], (name, 'window', field)
        assert window_reports[1]['cache']['hits'] == len(window_outputs[1])
        assert window_reports[1]['cache']['misses'] == 0

        oracles, diagnostics = {}, []
        for channel, path in assignments:
            try:
                oracle = cantools.database.load_file(str(path), strict=True)
            except Exception:
                assert name == 'easy'
                text = re.sub(r'^ENVVAR_DATA_\s+[^;]+;\s*$', '', path.read_text(encoding='cp1252'), flags=re.MULTILINE)
                oracle = cantools.database.load_string(text, database_format='dbc', strict=True)
                diagnostics.append('cantools only: ignored ENVVAR_DATA_ in memory; original preserved')
            for message in oracle.messages:
                key = (channel, message.frame_id, message.is_extended_frame)
                assert key not in oracles
                oracles[key] = message
        signal_count = 0
        for record in outputs['warm']:
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
        result = {'name': name, 'frames': len(expected), 'signals_compared': signal_count,
                  'issues_selected': reports['warm']['issues_selected'], 'status': reports['warm']['status'],
                  'cold_misses': reports['cold']['cache']['misses'], 'warm_hits': reports['warm']['cache']['hits'],
                  'window_hits': window_reports[1]['cache']['hits'], 'elapsed_s': timings,
                  'oracle': f'cantools {cantools.__version__}', 'diagnostics': sorted(set(diagnostics))}
        results.append(result)
        print(json.dumps(result), flush=True)

    so, _, _ = invoke('cache_info', 'workspace', 'cache', workspace, 'info')
    before_clear = json.loads(so.read_text(encoding='utf-8'))
    assert before_clear['rows'] == sum(r['frames'] for r in results)
    assert before_clear['building_generations'] == 0
    so, _, _ = invoke('cache_clear', 'workspace', 'cache', workspace, 'clear')
    after_clear = json.loads(so.read_text(encoding='utf-8'))
    assert after_clear['rows'] == after_clear['building_generations'] == 0
    assert sha(workspace/'workspace.json') == manifest_sha
    assert all(sha(p) == expected for p, expected in sources.items())
    summary = {'verified_at_utc': datetime.now(timezone.utc).isoformat(), 'exe_sha256': sha(exe),
               'sources_unchanged': True, 'manifest_unchanged_by_decode_and_clear': True,
               'source_hashes': {str(p): s for p, s in sources.items()}, 'results': results,
               'cache_before_clear': before_clear, 'cache_after_clear': after_clear, 'commands': commands}
    (out/'results.json').write_text(json.dumps(summary, indent=2, ensure_ascii=False)+'\n', encoding='utf-8')


if __name__ == '__main__':
    main()
