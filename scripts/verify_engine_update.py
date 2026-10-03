"""Compare old/new canlog binaries against the same persistent DBC cache."""
import argparse
import hashlib
import json
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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--before', required=True, type=Path)
    parser.add_argument('--exe', type=Path, default=ROOT/'dist/canlog.exe')
    parser.add_argument('--out', required=True, type=Path)
    parser.add_argument('--samples', type=Path, default=ROOT/'can_example/web_downloads/2026-10-03')
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    before, current, web = args.before.resolve(), args.exe.resolve(), args.samples.resolve()
    workspace = out/'workspace'
    cases = [
        ('motorola', web/'atemall_motorola/motorola_matrix.asc', [(1, web/'atemall_motorola/motorola_matrix.dbc')]),
        ('model3', web/'model3_can/CAN.asc', [(1, web/f'model3_can/Model3_{p}.dbc') for p in ('Battery', 'Drive', 'Thermal')]),
        ('easy', web/'py_canoe_demo/demo_log.blf', [(c, web/'py_canoe_demo/easy.dbc') for c in (1, 2)]),
    ]
    originals = {p: sha(p) for _, source, bindings in cases for p in [source, *(p for _, p in bindings)]}
    commands, results = [], []
    common = ['--unsupported', 'skip', '--recover']

    def invoke(exe, label, *argv):
        prefix = f'{len(commands)+1:03}_{label}'
        stdout, stderr = out/(prefix+'.stdout.txt'), out/(prefix+'.stderr.txt')
        started = time.perf_counter()
        with stdout.open('wb') as so, stderr.open('wb') as se:
            process = subprocess.run([str(exe), *map(str, argv)], stdout=so, stderr=se, timeout=120)
        commands.append({'label': label, 'argv': [str(exe), *map(str, argv)], 'exit_code': process.returncode,
                         'elapsed_s': time.perf_counter()-started, 'stdout': stdout.name, 'stderr': stderr.name})
        assert process.returncode in (0, 3), stderr.read_text(encoding='utf-8')[-2000:]
        return stdout

    def decode(exe, name, mode):
        report = out/f'{name}-{mode}-report.json'
        stdout = invoke(exe, name+'_'+mode, 'workspace', 'decode', workspace, name, *common, '--report', report)
        return rows(stdout), json.loads(report.read_text(encoding='utf-8'))

    invoke(before, 'old_create', 'workspace', 'create', workspace)
    for name, source, bindings in cases:
        invoke(before, name+'_add', 'workspace', 'add', workspace, source, '--name', name)
        flags = [v for channel, path in bindings for v in ('--dbc', f'{channel}={path}')]
        invoke(before, name+'_bind', 'workspace', 'bind', workspace, name, *flags)
        invoke(before, name+'_index', 'workspace', 'index', workspace, name, '--stride', '128', *common)
        old_rows, old_report = decode(before, name, 'old')
        manifest_hash = sha(workspace/'workspace.json')
        new_rows, new_report = decode(current, name, 'new_cold')
        warm_rows, warm_report = decode(current, name, 'new_warm')
        assert new_rows == warm_rows == old_rows
        assert new_report['engine_revision'] != old_report['engine_revision']
        assert new_report['cache']['key'] != old_report['cache']['key']
        assert new_report['cache']['hits'] == 0
        assert new_report['cache']['misses'] == len(new_rows)
        assert warm_report['cache']['hits'] == len(new_rows)
        assert warm_report['cache']['misses'] == 0
        assert new_report['index'] == old_report['index']
        assert sha(workspace/'workspace.json') == manifest_hash
        for field in ('status', 'issues_selected', 'issue_counts', 'decode_counts', 'selection_complete', 'source_sha256'):
            assert new_report[field] == warm_report[field] == old_report[field]
        result = {'name': name, 'frames': len(new_rows),
                  'valid_signals': sum(s['status'] == 'valid' for row in new_rows for s in row['signals']),
                  'before_revision': old_report['engine_revision'], 'after_revision': new_report['engine_revision'],
                  'old_cache_ignored': True, 'new_cold_misses': new_report['cache']['misses'],
                  'new_warm_hits': warm_report['cache']['hits'], 'raw_index_reused': True,
                  'status': new_report['status'], 'issues_selected': new_report['issues_selected']}
        results.append(result)
        print(json.dumps(result), flush=True)
    cache = invoke(current, 'new_cache_info', 'workspace', 'cache', workspace, 'info')
    info = json.loads(cache.read_text(encoding='utf-8'))
    assert info['rows'] == 2*sum(r['frames'] for r in results)
    assert info['building_generations'] == 0
    assert all(sha(p) == expected for p, expected in originals.items())
    summary = {'verified_at_utc': datetime.now(timezone.utc).isoformat(), 'before_exe_sha256': sha(before),
               'after_exe_sha256': sha(current), 'originals_unchanged': True,
               'original_hashes': {str(p): s for p, s in originals.items()},
               'results': results, 'cache': info, 'commands': commands}
    (out/'results.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2)+'\n', encoding='utf-8')


if __name__ == '__main__':
    main()
