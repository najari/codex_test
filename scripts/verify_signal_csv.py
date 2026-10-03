"""Compare signal CSV with typed JSONL using Python's independent csv reader."""
import argparse
import csv
import hashlib
import itertools
import json
import struct
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def nullable(text):
    if text == '\\N':
        return None
    return text[1:] if text.startswith('\\\\') else text


def scalar(value):
    if value is None:
        return None
    if isinstance(value, bool):
        return str(value).lower()
    return str(value)


def expected_rows(jsonl, revision):
    with jsonl.open(encoding='utf-8') as stream:
        for line in stream:
            r = json.loads(line)
            frame, location = r['record']['frame'], r['record']['location']
            base = {'schema_version': '1', 'row_kind': 'signal' if r['signals'] else 'frame_status',
                    'data_hex': bytes(frame['data']).hex().upper(), 'engine_revision': revision,
                    'frame_status': r['status'], 'database_sha256': r['database_sha256'],
                    'message': r['message'], 'error': r['error']}
            for field in ('timestamp_ns', 'channel', 'id', 'extended', 'direction', 'remote', 'fd', 'raw_dlc', 'brs', 'esi'):
                base[field] = scalar(frame[field])
            for field in ('source', 'ordinal', 'line', 'container', 'object_offset'):
                base[field] = scalar(location[field])
            for signal in r['signals'] or [None]:
                row = dict(base)
                raw = signal['raw'] if signal else None
                row.update(signal_key=signal['key'] if signal else None,
                           definition_ordinal=scalar(signal['definition_ordinal']) if signal else None,
                           signal_name=signal['name'] if signal else None,
                           signal_status=signal['status'] if signal else None,
                           raw_type=raw['type'] if raw else None, raw_value=scalar(raw['value']) if raw else None,
                           raw_text=signal['raw_text'] if signal else None,
                           physical=signal['physical'] if signal else None,
                           unit=signal['unit'] if signal else None, description=signal['description'] if signal else None)
                yield row


def compare(csv_path, jsonl_path, revision, allow_source_alias=False):
    count, valid, inactive, status_rows = 0, 0, 0, 0
    with csv_path.open(encoding='utf-8', newline='') as stream:
        reader = csv.DictReader(stream, strict=True)
        assert reader.fieldnames and len(reader.fieldnames) == 33
        for observed, expected in itertools.zip_longest(reader, expected_rows(jsonl_path, revision)):
            assert observed is not None and expected is not None, 'row count mismatch'
            actual = {k: nullable(v) for k, v in observed.items()}
            assert set(actual) == set(expected)
            if allow_source_alias and actual['source'] != expected['source']:
                # Workspace readers retain their canonical source path. Windows
                # may add a verbatim prefix; prove it is the same file explicitly.
                assert Path(actual['source']).samefile(expected['source'])
                actual['source'] = expected['source']
            physical = actual.pop('physical')
            wanted = expected.pop('physical')
            if wanted is None:
                assert physical is None
            else:
                assert struct.pack('!d', float(physical)) == struct.pack('!d', wanted), (physical, wanted)
            assert actual == expected, (count, actual, expected)
            count += 1
            valid += actual['signal_status'] == 'valid'
            inactive += actual['signal_status'] == 'inactive'
            status_rows += actual['row_kind'] == 'frame_status'
    return {'rows': count, 'valid_signals': valid, 'inactive_signals': inactive, 'frame_status_rows': status_rows}


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
    common = ['--unsupported', 'skip', '--recover']
    playback = ['--no-wait', '--on-regression', 'immediate']
    commands, results = [], []

    def invoke(label, *argv):
        prefix = f'{len(commands)+1:03}_{label}'
        stdout, stderr = out/(prefix+'.stdout.txt'), out/(prefix+'.stderr.txt')
        started = time.perf_counter()
        with stdout.open('wb') as so, stderr.open('wb') as se:
            proc = subprocess.run([str(exe), *map(str, argv)], stdout=so, stderr=se, timeout=120)
        commands.append({'label': label, 'argv': [str(exe), *map(str, argv)], 'exit_code': proc.returncode,
                         'elapsed_s': time.perf_counter()-started, 'stdout': stdout.name, 'stderr': stderr.name})
        assert proc.returncode in (0, 3), stderr.read_text(encoding='utf-8')[-2000:]
        return stdout, proc.returncode

    for name, source, assignments in cases:
        flags = [a for channel, db in assignments for a in ('--dbc', f'{channel}={db}')]
        decode_report = out/f'{name}-jsonl-report.json'
        jsonl, jsonl_code = invoke(name+'_jsonl', 'decode', source, *flags, *common, '--report', decode_report)
        metadata = json.loads(decode_report.read_text(encoding='utf-8'))
        revision = metadata['engine_revision']
        csv_report = out/f'{name}-csv-report.json'
        csv_path, code = invoke(name+'_csv', 'decode', source, *flags, *common, '--format', 'csv', '--report', csv_report)
        assert code == jsonl_code
        result = compare(csv_path, jsonl, revision)
        csv_metadata = json.loads(csv_report.read_text(encoding='utf-8'))
        assert csv_metadata['output_rows'] == result['rows']
        assert csv_metadata['export_schema']['id'] == 'canlog-signal-csv-v1'
        for field in ('status', 'frames_selected', 'issues_selected', 'decode_counts', 'source_sha256', 'assignments'):
            assert csv_metadata[field] == metadata[field], field
        dest = out/f'{name}-signals.csv'
        invoke(name+'_file', 'decode', source, *flags, *common, '-o', dest)
        assert sha(dest) == sha(csv_path)
        replay, replay_code = invoke(name+'_replay', 'replay', source, *flags, *common, *playback, '--sink', 'csv')
        assert replay_code == code and sha(replay) == sha(csv_path)
        root = out/(name+'-workspace')
        invoke(name+'_create', 'workspace', 'create', root)
        invoke(name+'_add', 'workspace', 'add', root, source, '--name', name)
        invoke(name+'_bind', 'workspace', 'bind', root, name, *flags)
        manifest_hash = sha(root/'workspace.json')
        cold_report, warm_report = out/f'{name}-cold-report.json', out/f'{name}-warm-report.json'
        cold, _ = invoke(name+'_cold', 'workspace', 'decode', root, name, *common, '--report', cold_report)
        warm, _ = invoke(name+'_warm', 'workspace', 'decode', root, name, *common, '--format', 'csv', '--report', warm_report)
        bypass, _ = invoke(name+'_bypass', 'workspace', 'decode', root, name, *common, '--format', 'csv', '--no-cache')
        assert sha(warm) == sha(bypass)
        assert compare(warm, cold, revision) == result
        assert compare(warm, jsonl, revision, allow_source_alias=True) == result
        cold_meta = json.loads(cold_report.read_text(encoding='utf-8'))
        warm_meta = json.loads(warm_report.read_text(encoding='utf-8'))
        assert cold_meta['cache']['key'] == warm_meta['cache']['key']
        assert warm_meta['cache']['hits'] == metadata['frames_selected'] and warm_meta['cache']['misses'] == 0
        assert sha(root/'workspace.json') == manifest_hash
        if name == 'motorola':
            repeat_jsonl, _ = invoke(name+'_repeat_jsonl', 'replay', source, *flags, *playback, '--repeat', '2', '--repeat-gap', '0.1')
            repeat_csv, _ = invoke(name+'_repeat_csv', 'replay', source, *flags, *playback, '--repeat', '2', '--repeat-gap', '0.1', '--sink', 'csv')
            assert compare(repeat_csv, repeat_jsonl, revision)['rows'] == 2*result['rows']
        result.update(name=name, frames=metadata['frames_selected'], issues_selected=metadata['issues_selected'],
                      status=metadata['status'], warm_hits=warm_meta['cache']['hits'], canonical_source_alias_verified=True)
        results.append(result)
        print(json.dumps(result), flush=True)
    # Generated boundary fixture; no downloaded source is modified.
    typed = out/'typed,한국어.jsonl'
    db = out/'typed.dbc'
    db.write_text('VERSION ""\nNS_ :\nBS_:\nBU_: Sender Receiver\n'
                  'BO_ 256 Wide: 8 Sender\n SG_ U : 0|64@1+ (1,0) [0|18446744073709551615] "" Receiver\n'
                  'BO_ 2147483904 Signed: 8 Sender\n SG_ I : 0|64@1- (1,0) [-9223372036854775808|9223372036854775807] "" Receiver\n'
                  'BO_ 259 FloatMsg: 8 Sender\n SG_ F : 0|64@1+ (1,0) [0|1] "" Receiver\nSIG_VALTYPE_ 259 F : 2;\n', encoding='utf-8')
    records = []
    for n, (ident, extended, raw) in enumerate([(256, False, 2**64-1), (256, True, 2**63), (259, False, 0x7ff8000000000021)]):
        records.append({'schema_version': 1, 'frame': {'timestamp_ns': 9007199254740993+n, 'channel': 1,
                        'id': ident, 'extended': extended, 'direction': 'unknown', 'remote': False, 'fd': False,
                        'raw_dlc': 8, 'data': list(raw.to_bytes(8, 'little')), 'brs': False, 'esi': False}})
    typed.write_text(''.join(json.dumps(r)+'\n' for r in records), encoding='utf-8')
    reference, _ = invoke('typed_jsonl', 'decode', typed, '--dbc', f'1={db}')
    exported, _ = invoke('typed_csv', 'decode', typed, '--dbc', f'1={db}', '--format', 'csv')
    assert compare(exported, reference, revision)['rows'] == 3
    with exported.open(encoding='utf-8', newline='') as stream:
        typed_rows = list(csv.DictReader(stream))
    assert typed_rows[0]['raw_value'] == str(2**64-1)
    assert typed_rows[1]['raw_value'] == str(-2**63)
    assert typed_rows[2]['raw_value'] == str(0x7ff8000000000021) and typed_rows[2]['physical'] == '\\N'
    assert all(sha(p) == expected for p, expected in originals.items())
    assert sha(exe) == exe_hash, 'executable changed during verification'
    summary = {'verified_at_utc': datetime.now(timezone.utc).isoformat(), 'exe_sha256': exe_hash,
               'originals_unchanged': True, 'original_hashes': {str(p): s for p, s in originals.items()},
               'typed_boundary_fixture': 'pass', 'results': results, 'commands': commands}
    (out/'results.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2)+'\n', encoding='utf-8')


if __name__ == '__main__':
    main()
