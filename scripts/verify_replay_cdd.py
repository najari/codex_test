"""Replay equivalence and live console pacing against independently checked KWP results.

The baseline is produced by verify_kwp_cdd.py (can-isotp and cantools oracles).
All original ASC/DBC/CDD/configuration files remain read-only.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import time


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--exe', type=Path, default=Path('dist/canlog.exe'))
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--uds-baseline', type=Path)
    parser.add_argument('--artifacts', type=Path, required=True)
    args = parser.parse_args()
    baseline = json.loads(args.baseline.read_text(encoding='utf-8'))
    assert baseline['assertions_passed'] and len(baseline['combined']) == 2
    exe = args.exe.resolve()
    for source, fingerprint in baseline['originals'].items():
        assert sha(Path(source)) == fingerprint, f'baseline original changed: {source}'
    args.artifacts.mkdir(parents=True, exist_ok=False)
    evidence = {'binary': str(exe), 'binary_sha256': sha(exe),
                'baseline': str(args.baseline), 'baseline_sha256': sha(args.baseline),
                'oracle_versions': baseline['versions'], 'originals': baseline['originals'],
                'commands': [], 'cases': []}
    for case in baseline['combined']:
        original = next(command['argv'] for command in baseline['commands']
                        if case['output'] in command['argv'])
        original_rows = [json.loads(line) for line in Path(case['output']).read_text().splitlines()]
        original_report = json.loads(Path(case['report']).read_text())
        replay_command = list(original)
        replay_command[0] = 'replay'
        replay_command += ['--protocol', 'kwp2000-vector', '--speed', '2']
        output = args.artifacts / f"{case['name']}.replay.jsonl"
        report = args.artifacts / f"{case['name']}.replay.report.json"
        replay_command[replay_command.index('-o') + 1] = str(output)
        replay_command[replay_command.index('--report') + 1] = str(report)
        instant = subprocess.run([str(exe), *replay_command, '--no-wait'], capture_output=True)
        evidence['commands'].append({'argv': replay_command + ['--no-wait'], 'exit': instant.returncode,
                                     'stderr': instant.stderr.decode('utf-8', errors='replace')})
        assert instant.returncode == 3
        actual = [json.loads(line) for line in output.read_text().splitlines()]
        assert actual == original_rows, case['name']
        replay_report = json.loads(report.read_text())
        for key in ('dbc_counts', 'kwp_counts', 'cdd_counts', 'timeline_key', 'output_rows'):
            assert replay_report[key] == original_report[key], (case['name'], key)
        assert replay_report['scan_complete'] and replay_report['published']
        assert replay_report['replay'] == {'speed': 2.0, 'no_wait': True,
                                           'on_regression': 'error', 'sink': 'jsonl'}
        evidence['cases'].append({'name': case['name'], 'rows': len(actual), 'equivalent': True})
        if case['name'] == 'ComfortDiagData':
            console_command = list(replay_command)
            index = console_command.index('-o')
            del console_command[index:index + 2]
            console_report = args.artifacts / 'ComfortDiagData.console.report.json'
            console_command[console_command.index('--report') + 1] = str(console_report)
            console_command += ['--sink', 'console']
            proc = subprocess.Popen([str(exe), *console_command], stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, text=True, encoding='utf-8')
            arrivals, lines = [], []
            for line in proc.stdout:
                lines.append(line)
                if re.match(r'^-?\d+\.\d+ ch\d+ ', line):
                    arrivals.append(time.monotonic())
            stderr = proc.stderr.read()
            code = proc.wait()
            evidence['commands'].append({'argv': console_command, 'exit': code, 'stderr': stderr})
            assert code == 3
            text = ''.join(lines)
            frames = [r for r in original_rows if r['kind'] == 'decoded_frame']
            assert len(arrivals) == len(frames) == 20
            expected_span = (frames[-1]['record']['frame']['timestamp_ns']
                             - frames[0]['record']['frame']['timestamp_ns']) / 1e9 / 2
            observed_span = arrivals[-1] - arrivals[0]
            assert observed_span >= expected_span * 0.9, (observed_span, expected_span)
            for value in ('9877', '5433', '2000', '8888'):
                assert f' = {value}\n' in text, value
            assert 'CDD [no_match]' in text and 'CDD [decoded_with_diagnostics]' in text
            assert 'no text table entry for raw value 0' in text
            (args.artifacts / 'ComfortDiagData.console.txt').write_text(text, encoding='utf-8')
            console_summary = json.loads(console_report.read_text())
            assert console_summary['replay']['sink'] == 'console'
            assert console_summary['scan_complete'] and not console_summary['published']
            assert console_summary['cdd_counts'] == original_report['cdd_counts']
            evidence['cases'].append({'name': 'ComfortDiagData.console', 'frames': len(arrivals),
                                      'speed': 2, 'expected_span_s': expected_span,
                                      'observed_span_s': observed_span, 'values_verified': True})
    if args.uds_baseline:
        uds = json.loads(args.uds_baseline.read_text())
        assert uds['passed']
        evidence['uds_baseline'] = str(args.uds_baseline)
        evidence['uds_baseline_sha256'] = sha(args.uds_baseline)
        for source, fingerprint in uds['originals'].items():
            assert sha(Path(source)) == fingerprint
            evidence['originals'][source] = fingerprint
        for index, command in enumerate(uds['commands']):
            original = command['argv']
            if original[0] != 'uds' or '-o' not in original or command['exit'] not in (0, 3):
                continue
            source_output = Path(original[original.index('-o') + 1])
            replay_command = list(original)
            replay_command[0] = 'replay'
            replay_command += ['--protocol', 'uds2013', '--no-wait']
            output = args.artifacts / f'uds_{index}.replay.jsonl'
            report = args.artifacts / f'uds_{index}.replay.report.json'
            replay_command[replay_command.index('-o') + 1] = str(output)
            if '--report' in replay_command:
                replay_command[replay_command.index('--report') + 1] = str(report)
            else:
                replay_command += ['--report', str(report)]
            proc = subprocess.run([str(exe), *replay_command], capture_output=True)
            evidence['commands'].append({'argv': replay_command, 'exit': proc.returncode,
                                         'stderr': proc.stderr.decode('utf-8', errors='replace')})
            assert proc.returncode == command['exit']
            assert output.read_bytes() == source_output.read_bytes(), str(source_output)
            evidence['cases'].append({'name': f'uds_{index}', 'equivalent': True})
            source_rows = [json.loads(line) for line in source_output.read_text().splitlines()]
            negatives = [r for r in source_rows
                         if r.get('cdd', {}).get('response', {}) is not None
                         and (r.get('cdd', {}).get('response') or {}).get('message') == 'NEG']
            if negatives:
                console_command = list(replay_command)
                output_index = console_command.index('-o')
                del console_command[output_index:output_index + 2]
                console_report = args.artifacts / f'uds_{index}.console.report.json'
                console_command[console_command.index('--report') + 1] = str(console_report)
                console_command += ['--sink', 'console']
                console = subprocess.run([str(exe), *console_command], capture_output=True)
                evidence['commands'].append({'argv': console_command, 'exit': console.returncode,
                                             'stderr': console.stderr.decode('utf-8', errors='replace')})
                assert console.returncode == command['exit']
                text = console.stdout.decode('utf-8')
                assert text.count('    NEG ') == len(negatives)
                (args.artifacts / f'uds_{index}.console.txt').write_text(text, encoding='utf-8')
                evidence['cases'].append({'name': f'uds_{index}.console',
                                          'negative_messages': len(negatives), 'labels_verified': True})
    for source, fingerprint in evidence['originals'].items():
        assert sha(Path(source)) == fingerprint, f'original changed: {source}'
    assert sha(exe) == evidence['binary_sha256']
    evidence['passed'] = True
    (args.artifacts / 'results.json').write_text(json.dumps(evidence, indent=2), encoding='utf-8')
    print(json.dumps({'passed': True, 'cases': len(evidence['cases']),
                      'commands': len(evidence['commands'])}, indent=2))


if __name__ == '__main__':
    main()
