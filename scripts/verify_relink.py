"""Verify ASC/BLF relocation using disposable copies, preserving original samples."""
import argparse
import copy
import hashlib
import json
import shutil
import subprocess
import time
from collections import Counter
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
    originals = {p: sha(p) for _, source, assignments in cases for p in [source, *(p for _, p in assignments)]}
    commands, results = [], []
    common = ['--unsupported', 'skip', '--recover']

    def invoke(label, *argv, expected=(0, 3)):
        prefix = f'{len(commands)+1:03}_{label}'
        stdout, stderr = out/(prefix+'.stdout.txt'), out/(prefix+'.stderr.txt')
        started = time.perf_counter()
        with stdout.open('wb') as so, stderr.open('wb') as se:
            process = subprocess.run([str(exe), *map(str, argv)], stdout=so, stderr=se, timeout=120)
        commands.append({'label': label, 'argv': [str(exe), *map(str, argv)], 'exit_code': process.returncode,
                         'elapsed_s': time.perf_counter()-started, 'stdout': stdout.name, 'stderr': stderr.name})
        assert process.returncode in expected, stderr.read_text(encoding='utf-8')[-2000:]
        return stdout

    def manifest():
        return json.loads((workspace/'workspace.json').read_text(encoding='utf-8'))

    def decode(name, label):
        report = out/(label+'-report.json')
        stdout = invoke(label, 'workspace', 'decode', workspace, name, *common, '--report', report)
        return rows(stdout), json.loads(report.read_text(encoding='utf-8'))

    invoke('create', 'workspace', 'create', workspace)
    for name, original, assignments in cases:
        old_folder, new_folder = out/('old_'+name), out/('new_'+name)
        old_folder.mkdir()
        new_folder.mkdir()
        source, moved = old_folder/original.name, new_folder/original.name
        shutil.copyfile(original, source)
        invoke(name+'_add', 'workspace', 'add', workspace, source, '--name', name)
        flags = [v for channel, path in assignments for v in ('--dbc', f'{channel}={path}')]
        invoke(name+'_bind', 'workspace', 'bind', workspace, name, *flags)
        invoke(name+'_old_index', 'workspace', 'index', workspace, name, '--stride', '128', *common)
        before_rows, before_report = decode(name, name+'_old_decode')
        before_manifest = manifest()
        old_log = next(log for log in before_manifest['logs'] if log['name'] == name)
        # These are single disposable files underneath the new evidence directory.
        assert source.resolve().is_relative_to(out) and moved.resolve().is_relative_to(out)
        source.replace(moved)
        old_folder.rmdir()
        assert not source.exists()

        wrong = out/('wrong_'+name+original.suffix)
        changed = bytearray(moved.read_bytes())
        changed[-1] ^= 1
        wrong.write_bytes(changed)
        manifest_hash = sha(workspace/'workspace.json')
        invoke(name+'_mismatch', 'workspace', 'relink', workspace, name, wrong, expected=(1,))
        assert sha(workspace/'workspace.json') == manifest_hash
        invoke(name+'_relink', 'workspace', 'relink', workspace, name, moved)
        new_manifest = manifest()
        new_log = next(log for log in new_manifest['logs'] if log['name'] == name)
        assert new_manifest['revision'] == before_manifest['revision']+1
        assert new_log['bindings'] == old_log['bindings']
        assert new_log['source_identity'] == old_log['source_identity']
        moved_rows, moved_report = decode(name, name+'_moved_cold')
        expected_rows = copy.deepcopy(before_rows)
        for row in expected_rows:
            row['record']['location']['source'] = new_log['path']
        assert moved_rows == expected_rows
        assert moved_report['cache']['hits'] == 0
        assert moved_report['cache']['misses'] == len(moved_rows)
        assert moved_report['index'] is None
        for field in ('status', 'issues_selected', 'issue_counts', 'decode_counts', 'selection_complete'):
            assert moved_report[field] == before_report[field]
        invoke(name+'_new_index', 'workspace', 'index', workspace, name, '--stride', '128', *common)
        warm_rows, warm_report = decode(name, name+'_moved_warm')
        assert warm_rows == moved_rows
        assert warm_report['cache']['hits'] == len(warm_rows)
        assert warm_report['cache']['misses'] == 0
        assert warm_report['index'] is not None
        query = invoke(name+'_query', 'workspace', 'query', workspace, name, *common)
        full = invoke(name+'_direct_view', 'view', new_log['path'], '--unlimited', *common)
        assert rows(query) == rows(full)
        assert sha(moved) == originals[original]
        result = {'name': name, 'frames': len(warm_rows), 'signals': sum(len(r['signals']) for r in warm_rows),
                  'signal_states': dict(Counter(s['status'] for r in warm_rows for s in r['signals'])),
                  'old_directory_removed': not old_folder.exists(), 'content_mismatch_rejected': True,
                  'moved_cold_misses': moved_report['cache']['misses'], 'moved_warm_hits': warm_report['cache']['hits'],
                  'query_matches_full_scan': True, 'issues_selected': warm_report['issues_selected'],
                  'status': warm_report['status']}
        results.append(result)
        print(json.dumps(result), flush=True)
    cache = invoke('cache_info', 'workspace', 'cache', workspace, 'info')
    info = json.loads(cache.read_text(encoding='utf-8'))
    assert info['building_generations'] == 0
    assert info['rows'] == 2*sum(r['frames'] for r in results)
    assert all(sha(p) == expected for p, expected in originals.items())
    summary = {'verified_at_utc': datetime.now(timezone.utc).isoformat(), 'exe_sha256': sha(exe),
               'originals_unchanged': True, 'original_hashes': {str(p): s for p, s in originals.items()},
               'results': results, 'cache': info, 'commands': commands}
    (out/'results.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2)+'\n', encoding='utf-8')


if __name__ == '__main__':
    main()
